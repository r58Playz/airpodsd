use std::sync::Arc;

use anyhow::{Context, Result};
use event_listener::Event;
use log::info;
use tokio::sync::Mutex;
use zbus::{
	Connection, conn::Builder as ConnBuilder, fdo::ObjectManager, interface, proxy,
	zvariant::OwnedObjectPath,
};

use super::{PodsBattery, PodsStatus, blconn::Address};

#[proxy]
trait BatteryProviderManager {
	fn register_battery_provider(&self, provider: OwnedObjectPath) -> zbus::Result<()>;
}

struct Battery {
	percentage: u8,
	device: OwnedObjectPath,
}

#[interface(name = "org.bluez.BatteryProvider1")]
impl Battery {
	#[zbus(property)]
	async fn percentage(&self) -> u8 {
		self.percentage
	}

	#[zbus(property)]
	async fn source(&self) -> &str {
		"AAP via airpodsd"
	}

	#[zbus(property)]
	async fn device(&self) -> OwnedObjectPath {
		self.device.clone()
	}
}

async fn create_conn(at: &str) -> Result<Connection> {
	ConnBuilder::system()
		.context("failed to create builder")?
		.serve_at(at, ObjectManager)
		.context("failed to serve objmanager")?
		.build()
		.await
		.context("failed to build")
}

fn calculate_percentage(data: PodsBattery) -> Option<u8> {
	data.left
		.as_percent()
		.zip(data.right.as_percent())
		.map(|(l, r)| (l + r) / 2)
}

pub async fn bluez_main(
	addr: Address,
	status: Arc<Mutex<PodsStatus>>,
	notify: Arc<Event>,
	name: String,
) -> Result<()> {
	let dev = addr.to_string().replace(":", "_");
	let prefix = format!("/dev/r58playz/airpodsd_{dev}");
	let iface_name = OwnedObjectPath::try_from(format!("{prefix}/dev_{dev}"))
		.context("interface object path is invalid")?;
	let bluez_name = OwnedObjectPath::try_from(format!("/org/bluez/{name}/dev_{dev}"))
		.context("bluez object path is invalid")?;
	let conn = create_conn(&prefix)
		.await
		.context("failed to connect to d-bus system bus")?;

	info!("registering bluez battery provider on /org/bluez/{}", name);

	let proxy = BatteryProviderManagerProxy::builder(&conn)
		.interface("org.bluez.BatteryProviderManager1")
		.context("failed to set battery manager interface")?
		.destination("org.bluez")
		.context("failed to set battery manager destination")?
		.path(format!("/org/bluez/{name}"))
		.context("failed to set battery manager path")?
		.build()
		.await
		.context("failed to create battery manager proxy")?;

	proxy
		.register_battery_provider(
			OwnedObjectPath::try_from(prefix).context("failed to create battery provider path")?,
		)
		.await
		.context("failed to register battery provider")?;

	info!("registered bluez battery provider");

	let mut last_battery_info = None;
	loop {
		notify.listen().await;

		let locked = status.lock().await;
		if last_battery_info.is_some_and(|x| x == locked.battery) {
			continue;
		} else {
			last_battery_info.replace(locked.battery);
		}

		let percent = calculate_percentage(locked.battery);
		let iface = conn
			.object_server()
			.interface::<_, Battery>(&iface_name)
			.await
			.ok();

		match (iface, percent) {
			(Some(iface), Some(percent)) => {
				info!("updating bluez battery percentage to {:?}%", percent);
				let mut iface_ref = iface.get_mut().await;
				iface_ref.percentage = percent;
				iface_ref
					.percentage_changed(iface.signal_emitter())
					.await
					.context("failed to fire percentage changed signal")?;
			}
			(Some(iface), None) => {
				info!("removing bluez battery, battery percentage is unavailable");
				drop(iface);
				conn.object_server()
					.remove::<Battery, _>(&iface_name)
					.await
					.context("failed to remove battery from manager")?;
			}
			(None, Some(percent)) => {
				info!(
					"adding bluez battery, battery percentage is now available: {:?}%",
					percent
				);
				conn.object_server()
					.at(&iface_name, Battery {
						device: bluez_name.clone(),
						percentage: percent,
					})
					.await
					.context("failed to add battery to manager")?;
			}
			(None, None) => {
				// nothing to do
			}
		}
	}
}
