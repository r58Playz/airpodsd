use std::sync::Arc;

use anyhow::{Context, Result};
use bluez::bluez_main;
use event_listener::Event;
use log::{LevelFilter, info};
use tokio::{sync::Mutex, task::JoinSet};

mod blconn;
mod bluetooth;
mod bluez;
mod packet;

use blconn::Address;
use bluetooth::{bluetooth_main, bluetooth_setup};
use packet::{BatteryStatus, EarDetectionStatus, NoiseControlStatus};

#[derive(Copy, Clone, Eq, PartialEq)]
struct PodsBattery {
	case: BatteryStatus,
	left: BatteryStatus,
	right: BatteryStatus,
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct PodsInEar {
	primary: EarDetectionStatus,
	secondary: EarDetectionStatus,
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct PodsStatus {
	battery: PodsBattery,
	noise: NoiseControlStatus,
	ear: PodsInEar,
}

pub async fn daemon_main(addr: Address) -> Result<()> {
	env_logger::builder()
		.filter_level(LevelFilter::Debug)
		.parse_default_env()
		.init();

	let status = Arc::new(Mutex::new(PodsStatus {
		battery: PodsBattery {
			case: BatteryStatus::Unknown,
			left: BatteryStatus::Unknown,
			right: BatteryStatus::Unknown,
		},
		noise: NoiseControlStatus::Off,
		ear: PodsInEar {
			primary: EarDetectionStatus::InEar,
			secondary: EarDetectionStatus::InEar,
		},
	}));
	let notify = Arc::new(Event::new());
	let mut set = JoinSet::new();

	let (device, name) = bluetooth_setup(addr)
		.await
		.context("failed to set up bluetooth")?;

	set.spawn(bluetooth_main(addr, status.clone(), notify.clone(), device));
	set.spawn(bluez_main(addr, status.clone(), notify.clone(), name));

	info!("daemon started");

	while let Some(ret) = set.join_next().await {
		ret.context("failed to wait for task")??;
	}

	Ok(())
}
