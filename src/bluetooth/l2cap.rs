use std::collections::HashMap;

use anyhow::{Context, Result};
use bluer::{Adapter, AdapterEvent, Address, Device, DeviceEvent, DeviceProperty, Uuid};
use bytes::Bytes;
use futures::{
	StreamExt,
	future::join_all,
	stream::{self, SelectAll, select},
};
use log::{debug, info, trace, warn};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::UnixStream,
	sync::mpsc,
	task::JoinSet,
};

use crate::{
	bluetooth::{
		l2capconn::{self, L2CapAddr},
		proto::{
			ENABLE_FEATURES, ENABLE_NOTIFICATIONS, GET_CLOUD_KEYS, HANDSHAKE_PACKET, L2CapPacket,
		},
	},
	daemon::DaemonEvent,
};

enum L2CapEvent {
	DeviceAdded(Address),
	DeviceConnected(Device),
	DeviceDisconnected(Device),
	Ignore,
}

pub struct L2CapClient {
	adapter: Adapter,
	set: JoinSet<Result<()>>,
	events: mpsc::UnboundedSender<DaemonEvent>,
}
impl L2CapClient {
	pub fn new(adapter: Adapter, events: mpsc::UnboundedSender<DaemonEvent>) -> Self {
		Self {
			adapter,
			events,
			set: JoinSet::new(),
		}
	}

	async fn is_device_apple(device: &Device) -> Result<bool> {
		let apple_uuid = Uuid::try_parse("74ec2172-0bad-4d01-8f77-997b2be0722a")
			.context("failed to parse apple uuid")?;

		let uuids = device
			.uuids()
			.await
			.context("failed to get device uuids")?
			.unwrap_or_default();

		Ok(uuids.contains(&apple_uuid))
	}

	async fn monitor_one(
		addr: Address,
		mut stream: UnixStream,
		events: mpsc::UnboundedSender<DaemonEvent>,
	) -> Result<()> {
		stream
			.write_all(HANDSHAKE_PACKET)
			.await
			.context("failed to write handshake")?;
		stream
			.write_all(ENABLE_FEATURES)
			.await
			.context("failed to write enable features")?;
		stream
			.write_all(ENABLE_NOTIFICATIONS)
			.await
			.context("failed to write enable notifications")?;

		stream
			.write_all(GET_CLOUD_KEYS)
			.await
			.context("failed to write get cloud keys")?;

		let mut buf = vec![0; 1024];
		loop {
			let read = stream.read(&mut buf).await?;
			let bytes = Bytes::copy_from_slice(&buf[0..read]);

			if let Some(packet) =
				L2CapPacket::decode(bytes).context("failed to parse l2cap packet")?
			{
				events
					.send(DaemonEvent::L2CapPacket(addr, packet))
					.context("failed to send message to daemon")?;
			}
		}
	}

	pub async fn monitor(mut self) -> Result<()> {
		let discovered_devices = self
			.adapter
			.device_addresses()
			.await
			.context("failed to get currently discovered devices")?;
		let discovered_events = join_all(discovered_devices.into_iter().map(async |x| {
			let device = self.adapter.device(x).context("failed to get device")?;

			if Self::is_device_apple(&device)
				.await
				.context("failed to check if device is apple")?
			{
				let connected = device
					.is_connected()
					.await
					.context("failed to check if device is connected")?;

				Ok(Some(if connected {
					L2CapEvent::DeviceConnected(device)
				} else {
					L2CapEvent::DeviceAdded(x)
				}))
			} else {
				Ok(None)
			}
		}))
		.await
		.into_iter()
		.filter_map(|x| x.transpose())
		.collect::<Result<Vec<_>>>()
		.context("failed to probe currently discovered device")?;
		let discovered = stream::iter(discovered_events);

		let events = self
			.adapter
			.events()
			.await
			.context("failed to get adapter event stream")?
			.map(|x| match x {
				AdapterEvent::DeviceAdded(x) => L2CapEvent::DeviceAdded(x),
				_ => L2CapEvent::Ignore,
			});
		let mut stream = select(select(events, discovered), SelectAll::new());

		let mut clients = HashMap::new();

		while let Some(event) = stream.next().await {
			match event {
				L2CapEvent::DeviceAdded(address) => {
					let res = (async || {
						let device = self
							.adapter
							.device(address)
							.context("failed to get device")?;

						if !Self::is_device_apple(&device)
							.await
							.context("failed to check if device is apple-ish")?
						{
							debug!(
								"filtered out device {address} since it doesn't have the apple uuid"
							);
							return Ok(());
						}

						let dev_stream = device
							.events()
							.await
							.context("failed to get device event stream")?
							.map(move |x| match x {
								DeviceEvent::PropertyChanged(DeviceProperty::Connected(
									connected,
								)) => {
									if connected {
										L2CapEvent::DeviceConnected(device.clone())
									} else {
										L2CapEvent::DeviceDisconnected(device.clone())
									}
								}
								_ => L2CapEvent::Ignore,
							});

						stream.get_mut().1.push(dev_stream);

						anyhow::Ok(())
					})()
					.await;

					if let Err(err) = res {
						warn!("unable to monitor device {address}: {err:?}");
					}
				}
				L2CapEvent::DeviceConnected(dev) => {
					let addr = dev.address();
					let res = (async || {
						let remote_addr = dev
							.remote_address()
							.await
							.context("failed to get remote address")?;

						let name = dev.name().await.context("failed to get device name")?;

						debug!("connecting to {name:?} ({addr}, {remote_addr})");

						let stream = l2capconn::connect(L2CapAddr::new(remote_addr, 0x1001))
							.await
							.context("failed to connect via l2cap")?;

						info!("connected to {name:?} ({addr}, {remote_addr})");

						let events = self.events.clone();

						events
							.send(DaemonEvent::BrEdrName(remote_addr, name))
							.context("failed to send message to daemon")?;

						clients.insert(
							addr,
							self.set.spawn(async move {
								let ret = Self::monitor_one(remote_addr, stream, events).await;
								if let Err(err) = ret {
									warn!("device monitoring for {} failed: {err:?}", addr);
									Err(err)
								} else {
									Ok(())
								}
							}),
						);

						anyhow::Ok(())
					})()
					.await;

					if let Err(err) = res {
						warn!("unable to connect to device {}: {:?}", addr, err);
					}
				}
				L2CapEvent::DeviceDisconnected(dev) => {
					if let Some(device) = clients.remove(&dev.address()) {
						device.abort();
					}
				}
				L2CapEvent::Ignore => {}
			}
		}

		Ok(())
	}
}
