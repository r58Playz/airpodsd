use std::sync::Arc;

use anyhow::{Context, Result};
use bluez::bluez_main;
use event_listener::Event;
use log::{LevelFilter, info};
use serde::{Deserialize, Serialize};
use tokio::{sync::Mutex, task::JoinSet};

mod blconn;
mod bluetooth;
mod bluez;
pub mod packet;
mod unix;

pub use blconn::Address;
use bluetooth::{bluetooth_main, bluetooth_setup};
use packet::{BatteryStatus, EarDetectionStatus, NoiseControlStatus};
use unix::unix_listener_main;

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
pub struct PodsBattery {
	pub case: BatteryStatus,
	pub left: BatteryStatus,
	pub right: BatteryStatus,
}

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
pub struct PodsInEar {
	pub primary: EarDetectionStatus,
	pub secondary: EarDetectionStatus,
}

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
pub struct PodsStatus {
	pub battery: Option<PodsBattery>,
	pub noise: Option<NoiseControlStatus>,
	pub ear: Option<PodsInEar>,
}

impl PodsStatus {
	pub fn unknown() -> Self {
		Self {
			battery: None,
			noise: None,
			ear: None,
		}
	}
}

pub type PodsState = Arc<Mutex<PodsStatus>>;

pub async fn daemon_main(addr: Address) -> Result<()> {
	env_logger::builder()
		.filter_level(LevelFilter::Debug)
		.parse_default_env()
		.init();

	let status = Arc::new(Mutex::new(PodsStatus::unknown()));
	let notify = Arc::new(Event::new());
	let mut set = JoinSet::new();

	let (device, name) = bluetooth_setup(addr)
		.await
		.context("failed to set up bluetooth")?;

	set.spawn(bluetooth_main(addr, status.clone(), notify.clone(), device));
	set.spawn(bluez_main(addr, status.clone(), notify.clone(), name));
	set.spawn(unix_listener_main(addr, status, notify));

	info!("daemon started");

	while let Some(ret) = set.join_next().await {
		ret.context("failed to wait for task")??;
	}

	Ok(())
}
