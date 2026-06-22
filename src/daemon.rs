use std::{
	collections::HashMap,
	os::{linux::net::SocketAddrExt, unix::net::SocketAddr},
	path::PathBuf,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bluer::{Adapter, Address, Session};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use log::{debug, info, trace};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::UnixListener,
	sync::mpsc,
};

use crate::{
	battery::BluezBatteryProvider,
	bluetooth::{
		ble::BleClient,
		l2cap::L2CapClient,
		proto::{Battery, BatteryComponent, BatteryStatus, L2CapPacket, ProximityPairingPacket},
	},
	keystore::KeyStore,
};

pub enum DaemonEvent {
	BrEdrConnected(Address, Option<String>),
	BrEdrPacket(Address, L2CapPacket),
	BrEdrDisconnected(Address),
	BleAdvertisement(Address, ProximityPairingPacket),
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
	keystore: Arc<KeyStore>,

	device_battery_timeout: Duration,
	device_drop_timeout: Duration,
}
impl Daemon {
	pub async fn new(
		keystore_path: PathBuf,
		device_battery_timeout: Duration,
		device_drop_timeout: Duration,
	) -> Result<Self> {
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

		let keystore = Arc::new(
			KeyStore::new(keystore_path)
				.await
				.context("failed to initialize keystore")?,
		);

		Ok(Self {
			adapter,
			battery_manager,
			keystore,
			device_battery_timeout,
			device_drop_timeout,
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

	fn update_device_battery(state: &mut DeviceState, batteries: Vec<Battery>) {
		for battery in batteries {
			match battery.component {
				BatteryComponent::Case => state.battery.case = battery.status,
				BatteryComponent::Left => state.battery.left = battery.status,
				BatteryComponent::Right => state.battery.right = battery.status,
			}
		}
	}

	pub async fn monitor(mut self) -> Result<()> {
		let (tx, mut rx) = mpsc::unbounded_channel();

		let mut l2cap = tokio::spawn(
			L2CapClient::new(self.adapter.clone(), self.keystore.clone(), tx.clone()).monitor(),
		)
		.fuse();
		let mut ble =
			tokio::spawn(BleClient::new(self.adapter.clone(), self.keystore.clone(), tx).monitor())
				.fuse();

		let mut timeouts = FuturesUnordered::from_iter(std::iter::once(Box::pin(
			futures::future::pending(),
		) as TimeoutFuture));

		let mut devices = HashMap::new();

		let unix = std::os::unix::net::UnixListener::bind_addr(
			&SocketAddr::from_abstract_name("dev.r58playz.airpodsd").unwrap(),
		)
		.context("failed to bind to daemon unix socket")?;
		unix.set_nonblocking(true)
			.context("failed to set nonblocking")?;
		let unix = UnixListener::from_std(unix).context("failed to convert to tokio")?;

		info!("running");

		loop {
			tokio::select! {
				x = &mut l2cap => x.context("failed to join l2cap").flatten()?,
				x = &mut ble => x.context("failed to join ble").flatten()?,
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
				stream = unix.accept() => {
					let (stream, addr) = stream.context("failed to accept unix socket stream")?;

					debug!("new unix socket client {addr:?}");

					let devices = devices.clone();
					tokio::spawn({
						async move {
							let (mut rx, mut tx) = stream.into_split();
							let mut str = String::new();
							rx.read_to_string(&mut str).await.context("failed to read client request")?;

							match str.as_str() {
								"detailed" => {
									tx.write_all(b"Current device info:\n").await.context("failed to write")?;
									for (addr, state) in devices {
										tx.write_all(&format!("Device {addr:?}:\n").into_bytes()).await.context("failed to write")?;
										tx.write_all(&format!("\tConnected: {}\n", state.connected).into_bytes()).await.context("failed to write")?;
										tx.write_all(&format!("\tLast BLE advertise: {:?}\n", state.last_ble_advertise).into_bytes()).await.context("failed to write")?;
										tx.write_all(&format!("\tCase: {:?}\n", state.battery.case).into_bytes()).await.context("failed to write")?;
										tx.write_all(&format!("\tLeft: {:?}\n", state.battery.left).into_bytes()).await.context("failed to write")?;
										tx.write_all(&format!("\tRight: {:?}\n", state.battery.right).into_bytes()).await.context("failed to write")?;
									}
								}
								"status" => {
									tx.write_all(&format!("{} devices connected :3\n", devices.iter().filter(|(_, b)| b.connected).count()).into_bytes()).await.context("failed to write")?;
								}
								_ => {}
							}

							tx.shutdown().await.context("failed to end stream")?;
							anyhow::Ok(())
						}.then(|x| async move { trace!("served client {addr:?}: {x:?}") })
					});
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
										Self::update_device_battery(state, batteries);
										self.battery_manager.update_battery(addr, &state.battery).await.context("failed to update battery level")?;
									}
									L2CapPacket::Info { name, .. } => {
										state.name = Some(name);
									}
									L2CapPacket::CloudKeys(keys) => {
										self.keystore.set(addr, keys).await.context("failed to add to keystore")?;
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
						Some(DaemonEvent::BleAdvertisement(addr, advertisement)) => {
							let state = devices.entry(addr).or_insert_with(Default::default);
							state.last_ble_advertise = Some(Instant::now());
							Self::update_device_battery(state, advertisement.to_battery_state());
							self.battery_manager.update_battery(addr, &state.battery).await.context("failed to update battery level")?;

							debug!("device {addr} advertised");
							trace!("new state for {addr}: {state:?}");
							timeouts.extend(self.device_timeouts(addr));
						}
						None => {
							break Ok(());
						}
					}
				}
			}
		}
	}
}
