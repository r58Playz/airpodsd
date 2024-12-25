use anyhow::{Context, Result};
use bluer::{Device, Session};
use bytes::{Buf, Bytes};
use event_listener::Event;
use log::info;
use std::{io::ErrorKind, sync::Arc, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::UnixStream,
};

use crate::daemon::{
	blconn::{self, L2CapAddr},
	packet::BatteryStatus,
};

use super::{
	PodsBattery, PodsInEar, PodsState, PodsStatus,
	blconn::Address,
	packet::{BatteryComponent, EarDetectionStatus, ParsedPacket},
};

async fn handle_stream(
	mut stream: UnixStream,
	status: PodsState,
	notify: Arc<Event>,
) -> Result<()> {
	// handshake
	stream
		.write_all(&[
			0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
			0x00, 0x00,
		])
		.await
		.context("failed to send handshake")?;

	// enable features
	stream
		.write_all(&[
			0x04, 0x00, 0x04, 0x00, 0x4d, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
		])
		.await
		.context("failed to send enable features")?;

	// enable notifications
	stream
		.write_all(&[0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF])
		.await
		.context("failed to send enable notifications")?;

	let mut last_stats: Option<PodsStatus> = None;
	let mut buf = vec![0; 1024];
	loop {
		match stream.read(&mut buf).await {
			Ok(read) => {
				let bytes = Bytes::copy_from_slice(&buf[..read]);

				if bytes.remaining() == 0 {
					break Ok(());
				}

				if let Some(packet) =
					ParsedPacket::decode(bytes).context("failed to decode packet")?
				{
					let mut lock = status.lock().await;
					match packet {
						ParsedPacket::Battery(batteries) => {
							let locked = lock.battery.get_or_insert(PodsBattery {
								case: BatteryStatus::Unknown,
								left: BatteryStatus::Unknown,
								right: BatteryStatus::Unknown,
							});
							for battery in batteries {
								match battery.component {
									BatteryComponent::Case => locked.case = battery.status,
									BatteryComponent::Left => locked.left = battery.status,
									BatteryComponent::Right => locked.right = battery.status,
								}
							}
						}
						ParsedPacket::NoiseControl(status) => {
							lock.noise = Some(status);
						}
						ParsedPacket::EarDetection { primary, secondary } => {
							let locked = lock.ear.get_or_insert(PodsInEar {
								primary: EarDetectionStatus::InCase,
								secondary: EarDetectionStatus::InCase,
							});
							locked.primary = primary;
							locked.secondary = secondary;
						}
					}
					if last_stats.is_some_and(|x| x == *lock) {
						continue;
					} else {
						last_stats.replace(*lock);
					}
					notify.notify(usize::MAX);
				}
			}
			Err(err) => {
				if matches!(err.kind(), ErrorKind::ConnectionReset | ErrorKind::TimedOut) {
					// device probably went to sleep
					break Ok(());
				} else {
					break Err(err).context("failed to read from stream");
				}
			}
		}
	}
}

pub async fn bluetooth_setup(addr: Address) -> Result<(Device, String)> {
	let session = Session::new()
		.await
		.context("failed to connect to bluetoothd")?;
	let adapter = session
		.default_adapter()
		.await
		.context("failed to get default adapter")?;
	let device = adapter
		.device(bluer::Address::new(addr.into_inner()))
		.context("failed to get device")?;

	let name = device.adapter_name().to_string();

	Ok((device, name))
}

pub async fn bluetooth_main(
	addr: Address,
	status: PodsState,
	notify: Arc<Event>,
	device: Device,
) -> Result<()> {
	let mut was_waiting = true;
	loop {
		// so that we don't steal the device from bluetoothd making it impossible to connect for
		// audio
		while device
			.is_connected()
			.await
			.context("failed to get connected status of device")?
		{
			was_waiting = false;
			info!("connecting to {}", addr);
			let stream = blconn::connect(L2CapAddr::new(addr, 0x1001))
				.await
				.context("failed to connect to address")?;
			info!("connected to device over l2cap");

			handle_stream(stream, status.clone(), notify.clone())
				.await
				.context("failed to handle device stream")?;
		}

		if !was_waiting {
			let mut locked = status.lock().await;
			locked.ear.take();
			locked.battery.take();
			locked.noise.take();

			notify.notify(usize::MAX);
			was_waiting = true;
		}

		info!("waiting for device to connect");
		tokio::time::sleep(Duration::from_secs(10)).await;
	}
}
