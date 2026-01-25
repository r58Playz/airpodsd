use anyhow::{Context, Result};
use bluer::{
	Adapter,
	monitor::{Monitor, MonitorEvent, Pattern, Type},
};
use futures::StreamExt;
use log::trace;

pub async fn monitor_ble(adapter: Adapter) -> Result<()> {
	let monitor_manager = adapter
		.monitor()
		.await
		.context("failed to start monitoring")?;

	let mut monitor = monitor_manager
		.register(Monitor {
			monitor_type: Type::OrPatterns,
			patterns: Some(vec![Pattern::new(0xff, 0, &[0x4C, 0x00, 0x07])]),
			..Default::default()
		})
		.await
		.context("failed to create monitor for apple audio")?;

	while let Some(event) = monitor.next().await {
		match event {
			MonitorEvent::DeviceFound(info) => {
				let device = adapter
					.device(info.device)
					.context("failed to get device")?;
				let name = device
					.name()
					.await
					.context("failed to get device name")?
					.unwrap_or_else(|| "Unknown Name".to_string());
				let mac = device
					.remote_address()
					.await
					.context("failed to get resolved address")?;
				let mac_type = device
					.address_type()
					.await
					.context("failed to get address type")?;
				trace!(
					"found device {:?} {:?} {:?} {:?}",
					info.device, name, mac, mac_type
				);
			}
			_ => {}
		}
	}

	Ok(())
}
