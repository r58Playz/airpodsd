use std::{
	collections::HashMap,
	pin::Pin,
	time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bluer::{Adapter, Address, Session};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use log::{debug, info, trace};
use tokio::sync::mpsc;

use crate::{
	battery::BluezBatteryProvider,
	bluetooth::{
		l2cap::L2CapClient,
		proto::{BatteryComponent, BatteryStatus, L2CapPacket, ProximityPairingPacket},
	},
};

pub enum DaemonEvent {
	BrEdrConnected(Address, Option<String>),
	BrEdrPacket(Address, L2CapPacket),
	BrEdrDisconnected(Address),
	BleAdvertisement(ProximityPairingPacket),
}

#[derive(Clone, Debug)]
pub struct DeviceBattery {
	pub case: BatteryStatus,
	pub left: BatteryStatus,
	pub right: BatteryStatus,
}
impl Default for DeviceBattery {
	fn default() -> Self {
		Self {
			case: BatteryStatus::Unknown,
			left: BatteryStatus::Unknown,
			right: BatteryStatus::Unknown,
		}
	}
}

#[derive(Clone, Debug)]
struct DeviceState {
	name: Option<String>,
	connected: bool,
	last_ble_advertise: Option<Instant>,
	battery: DeviceBattery,
}
impl Default for DeviceState {
	fn default() -> Self {
		Self {
			name: None,
			connected: false,
			last_ble_advertise: None,
			battery: DeviceBattery::default(),
		}
	}
}

enum DeviceTimeoutType {
	Battery,
	Drop,
}

type TimeoutFuture = Pin<Box<dyn Future<Output = (DeviceTimeoutType, Address)>>>;
pub struct Daemon {
	adapter: Adapter,
	battery_manager: BluezBatteryProvider,

	device_battery_timeout: Duration,
	device_drop_timeout: Duration,
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
			adapter,
			battery_manager,
			device_battery_timeout: Duration::from_secs(60),
			device_drop_timeout: Duration::from_secs(120),
		})
	}

	fn device_accessible(&self, device: &DeviceState) -> bool {
		device.connected
			|| device
				.last_ble_advertise
				.is_some_and(|x| x.elapsed() < self.device_battery_timeout)
	}

	fn device_timeouts(&self, addr: Address) -> [TimeoutFuture; 2] {
		[
			Box::pin(
				tokio::time::sleep(self.device_battery_timeout)
					.map(move |_| (DeviceTimeoutType::Battery, addr)),
			),
			Box::pin(
				tokio::time::sleep(self.device_drop_timeout)
					.map(move |_| (DeviceTimeoutType::Drop, addr)),
			),
		]
	}

	pub async fn monitor(mut self) -> Result<()> {
		let (tx, mut rx) = mpsc::unbounded_channel();

		let mut l2cap =
			tokio::spawn(L2CapClient::new(self.adapter.clone(), tx.clone()).monitor()).fuse();

		let mut timeouts = FuturesUnordered::from_iter(std::iter::once(Box::pin(
			futures::future::pending(),
		) as TimeoutFuture));

		let mut devices = HashMap::new();

		info!("running");

		loop {
			tokio::select! {
				x = &mut l2cap => x.context("failed to join l2cap").flatten()?,
				addr = timeouts.next() => {
					// addr will never be None since we put a infinite pending in the timeout list
					let (ty, addr) = addr.unwrap();
					if let Some(state) = devices.get_mut(&addr) && !self.device_accessible(state) {
						match ty {
							DeviceTimeoutType::Battery => {
								state.battery = Default::default();
								self.battery_manager.drop_battery(addr).await.context("failed to stop exposing battery level")?;
								debug!("dropped battery for device {addr}");
							}
							DeviceTimeoutType::Drop => {
								devices.remove(&addr);
								debug!("removed device {addr}");
							}
						}
					}
				}
				x = rx.recv() => {
					match x {
						Some(DaemonEvent::BrEdrConnected(addr, name)) => {
							let state = devices.entry(addr).or_insert_with(Default::default);
							state.connected = true;
							if let Some(name) = name {
								state.name = Some(name);
							}

							debug!("device {addr} connected");
							trace!("new state for {addr}: {state:?}");
						}
						Some(DaemonEvent::BrEdrPacket(addr, packet)) => {
							if let Some(state) = devices.get_mut(&addr) {
								match packet {
									L2CapPacket::Battery(batteries) => {
										for battery in batteries {
											match battery.component {
												BatteryComponent::Case => state.battery.case = battery.status,
												BatteryComponent::Left => state.battery.left = battery.status,
												BatteryComponent::Right => state.battery.right = battery.status,
											}
										}

										self.battery_manager.update_battery(addr, &state.battery).await.context("failed to update battery level")?;
									}
									L2CapPacket::Info { name, .. } => {
										state.name = Some(name);
									}
									L2CapPacket::CloudKeys { irk, enc_key } => {
										trace!("got keys for {addr}: {irk:?} {enc_key:?}");
									}
									_ => {}
								}

								trace!("new state for {addr}: {state:?}");
							}
						}
						Some(DaemonEvent::BrEdrDisconnected(addr)) => {
							if let Some(state) = devices.get_mut(&addr) {
								state.connected = false;
								debug!("device {addr} disconnected");
								trace!("new state for {addr}: {state:?}");
								timeouts.extend(self.device_timeouts(addr));
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
