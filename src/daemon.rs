use std::collections::HashMap;

use anyhow::{Context, Result};
use bluer::{Adapter, Address, Session};
use log::debug;
use tokio::sync::mpsc;

use crate::{
	battery::BluezBatteryProvider,
	bluetooth::{
		l2cap::L2CapClient,
		proto::{BatteryComponent, BatteryStatus, L2CapPacket, ProximityPairingPacket},
	},
};

pub enum DaemonEvent {
	BrEdrName(Address, Option<String>),
	L2CapPacket(Address, L2CapPacket),
	BleAdvertisement(ProximityPairingPacket),
}

#[derive(Clone, Debug)]
struct DeviceState {
	name: Option<String>,
	case_battery: BatteryStatus,
	left_battery: BatteryStatus,
	right_battery: BatteryStatus,
}
impl Default for DeviceState {
	fn default() -> Self {
		Self {
			name: None,
			case_battery: BatteryStatus::Unknown,
			left_battery: BatteryStatus::Unknown,
			right_battery: BatteryStatus::Unknown,
		}
	}
}

pub struct Daemon {
	devices: HashMap<Address, DeviceState>,
	adapter: Adapter,
	battery_manager: BluezBatteryProvider,
}
impl Daemon {
	pub async fn new() -> Result<Self> {
		let session = Session::new()
			.await
			.context("failed to connect to bluetoothd")?;

		let adapter = session
			.default_adapter()
			.await
			.context("no bluetooth adapter found")?;

		let battery_manager = BluezBatteryProvider::new(&adapter)
			.await
			.context("failed to register bluez battery provider")?;

		Ok(Self {
			devices: HashMap::new(),
			adapter,
			battery_manager,
		})
	}

	pub async fn monitor(mut self) -> Result<()> {
		let (tx, mut rx) = mpsc::unbounded_channel();

		let mut l2cap = tokio::spawn(L2CapClient::new(self.adapter, tx.clone()).monitor());

		loop {
			tokio::select! {
				x = &mut l2cap => x.context("failed to join l2cap").flatten()?,
				x = rx.recv() => {
					match x {
						Some(DaemonEvent::L2CapPacket(addr, packet)) => {
							let state = self.devices.entry(addr).or_insert_with(Default::default);

							match packet {
								L2CapPacket::Battery(batteries) => {
									for battery in batteries {
										match battery.component {
											BatteryComponent::Case => state.case_battery = battery.status,
											BatteryComponent::Left => state.left_battery = battery.status,
											BatteryComponent::Right => state.right_battery = battery.status,
										}
									}

									self.battery_manager.update_battery(addr, state.case_battery, state.left_battery, state.right_battery).await.context("failed to update battery level")?;
								}
								L2CapPacket::Info { name, .. } => {
									state.name = Some(name);
								}
								L2CapPacket::CloudKeys { irk, enc_key } => {
									debug!("got keys for {addr}: {irk:?} {enc_key:?}");
								}
								_ => {}
							}

							debug!("new state for {addr}: {state:?}");
						}
						Some(DaemonEvent::BrEdrName(addr, name)) => {
							let state = self.devices.entry(addr).or_insert_with(Default::default);
							if let Some(name) = name {
								state.name = Some(name);
								debug!("new state for {addr}: {state:?}");
							}
						}
						Some(DaemonEvent::BleAdvertisement(ad)) => {}
						None => {
							break Ok(());
						}
					}
				}
			}
		}
	}
}
