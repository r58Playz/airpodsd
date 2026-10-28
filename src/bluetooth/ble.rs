use std::{collections::HashMap, sync::Arc};

use aes::Aes128;
use anyhow::{Context, Result, bail};
use bluer::{
	Adapter, Address, AddressType, Device, DeviceEvent, DeviceProperty,
	monitor::{
		Monitor, MonitorEvent, Pattern, RssiSamplingPeriod, Type,
		data_type::MANUFACTURER_SPECIFIC_DATA,
	},
};
use bytes::Bytes;
use cbc::Decryptor;
use cipher::{BlockDecryptMut, BlockEncrypt, KeyInit, KeyIvInit, generic_array::GenericArray};
use futures::{
	StreamExt, future,
	stream::{self, SelectAll},
};
use log::{trace, warn};
use tokio::sync::mpsc;

use crate::{bluetooth::proto::ProximityPairingPacket, daemon::DaemonEvent, keystore::KeyStore};

/// Verify a BLE Resolvable Private Address (RPA)
///
/// address layout (little-endian):
/// [ hash(3) | prand(3) ]
pub fn verify_rpa(mut address: [u8; 6], mut irk: Vec<u8>) -> bool {
	address.reverse();
	let mut prand: [u8; 16] = [
		0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, address[5], address[4], address[3],
	];
	irk.reverse();

	let cipher = Aes128::new(GenericArray::from_slice(&irk));
	let mut block = GenericArray::from_mut_slice(&mut prand);
	cipher.encrypt_block(&mut block);

	let correct = [block[15], block[14], block[13]] == [address[0], address[1], address[2]];
	trace!("rpa verify for address {address:?} and irk {irk:?}: {correct}");
	correct
}

enum BleEvent {
	DeviceFound(Address),
	DeviceLost(Address),
	ManufacturerData(Device, HashMap<u16, Vec<u8>>),
}

pub struct BleClient {
	adapter: Adapter,
	keystore: Arc<KeyStore>,
	events: mpsc::UnboundedSender<DaemonEvent>,
}
impl BleClient {
	pub fn new(
		adapter: Adapter,
		keystore: Arc<KeyStore>,
		events: mpsc::UnboundedSender<DaemonEvent>,
	) -> Self {
		Self {
			adapter,
			keystore,
			events,
		}
	}

	fn decrypt_advertisement(advertisement: Bytes, enc_key: Bytes) -> Result<Bytes> {
		if advertisement.len() < 16 {
			bail!("advertisement too small");
		}
		let enc_key = enc_key.as_array::<16>().context("enc_key incorrect size")?;

		let mut block = [0u8; 16];
		block.copy_from_slice(&advertisement[advertisement.len() - 16..]);

		let iv = [0; 16];
		let mut decryptor = Decryptor::<Aes128>::new(enc_key.into(), &iv.into());
		decryptor.decrypt_block_mut((&mut block).into());

		Ok(Bytes::from(block.to_vec()))
	}

	async fn process_advertisement(
		&self,
		device: &Device,
		mut data_map: HashMap<u16, Vec<u8>>,
	) -> Result<()> {
		let Some(data) = data_map.remove(&0x004C) else {
			return Ok(());
		};
		if data.first() != Some(&0x07) {
			return Ok(());
		}
		let data = Bytes::from(data);
		let mac = device
			.remote_address()
			.await
			.context("failed to get resolved address")?;
		let mac_type = device
			.address_type()
			.await
			.context("failed to get address type")?;

		trace!("got advertisement from {mac}");

		if mac_type == AddressType::LeRandom
			&& let Some((addr, keys)) = self
				.keystore
				.find(|keys| verify_rpa(mac.0, keys.irk.to_vec()))
				.await
		{
			let decrypted = Self::decrypt_advertisement(data.clone(), keys.enc_key)
				.context("failed to decrypt advertisement")?;

			self.events
				.send(DaemonEvent::BleAdvertisement(
					addr,
					ProximityPairingPacket::decode(data, decrypted)
						.context("failed to decode advertisement")?,
				))
				.context("failed to send daemon event")?;
		}

		Ok(())
	}

	pub async fn monitor(self) -> Result<()> {
		let monitor_manager = self
			.adapter
			.monitor()
			.await
			.context("failed to get monitor manager")?;

		let mut monitor = monitor_manager
			.register(Monitor {
				monitor_type: Type::OrPatterns,
				rssi_sampling_period: Some(RssiSamplingPeriod::All),
				patterns: Some(vec![Pattern::new(
					MANUFACTURER_SPECIFIC_DATA,
					0,
					&[0x4C, 0x00, 0x07],
				)]),
				..Default::default()
			})
			.await
			.context("failed to create monitor for apple audio")?;

		let mut device_events = SelectAll::new();
		let mut monitors = HashMap::new();

		loop {
			let event = tokio::select! {
				event = monitor.next() => match event {
					Some(MonitorEvent::DeviceFound(info)) => BleEvent::DeviceFound(info.device),
					Some(MonitorEvent::DeviceLost(info)) => BleEvent::DeviceLost(info.device),
					Some(_) => continue,
					None => break,
				},
				Some(event) = device_events.next(), if !device_events.is_empty() => event,
			};

			match event {
				BleEvent::DeviceFound(addr) => {
					let ret = (async || {
						let device = self.adapter.device(addr).context("failed to get device")?;

						// Subscribe before reading the initial data so later broadcasts are queued.
						let events = device
							.events()
							.await
							.context("failed to get device event stream")?;
						let dev = device.clone();
						let events = events
							.filter_map(move |event| {
								future::ready(match event {
									DeviceEvent::PropertyChanged(
										DeviceProperty::ManufacturerData(data),
									) => Some(BleEvent::ManufacturerData(dev.clone(), data)),
									_ => None,
								})
							})
							.chain(stream::once(future::ready(BleEvent::DeviceLost(addr))));
						let (events, handle) = stream::abortable(events);
						device_events.push(events.boxed());
						if let Some(previous) = monitors.insert(addr, handle) {
							previous.abort();
						}

						let data_map = device
							.manufacturer_data()
							.await
							.context("failed to get manufacturer data")?
							.unwrap_or_default();
						self.process_advertisement(&device, data_map).await
					})()
					.await;

					if let Err(err) = ret {
						warn!("failed to monitor advertisements for {addr}: {err:?}");
					}
				}
				BleEvent::ManufacturerData(device, data) => {
					if let Err(err) = self.process_advertisement(&device, data).await {
						warn!(
							"failed to process manufacturer data for {}: {err:?}",
							device.address()
						);
					}
				}
				BleEvent::DeviceLost(addr) => {
					if let Some(handle) = monitors.remove(&addr) {
						handle.abort();
					}
				}
			}
		}

		Ok(())
	}
}
