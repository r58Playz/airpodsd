use anyhow::{Context, Result};
use bluer::{Adapter, Address};
use zbus::{
	Connection, conn::Builder as ConnBuilder, fdo::ObjectManager, interface, proxy,
	zvariant::OwnedObjectPath,
};

use crate::bluetooth::proto::BatteryStatus;

const AIRPODSD_SERVE_PATH: &str = "/dev/r58playz/airpodsd";

#[proxy]
trait BatteryProviderManager {
	fn register_battery_provider(&self, provider: OwnedObjectPath) -> zbus::Result<()>;
}

struct BluezBattery {
	percentage: u8,
	device: OwnedObjectPath,
}

#[interface(name = "org.bluez.BatteryProvider1")]
impl BluezBattery {
	#[zbus(property)]
	async fn percentage(&self) -> u8 {
		self.percentage
	}

	#[zbus(property)]
	async fn source(&self) -> &str {
		"airpodsd"
	}

	#[zbus(property)]
	async fn device(&self) -> OwnedObjectPath {
		self.device.clone()
	}
}

pub struct BluezBatteryProvider {
	adapter_name: String,
	conn: Connection,
}
impl BluezBatteryProvider {
	pub async fn new(adapter: &Adapter) -> Result<Self> {
		let adapter_name = adapter.name().to_string();
		let conn = ConnBuilder::system()
			.context("failed to create builder")?
			.serve_at(AIRPODSD_SERVE_PATH, ObjectManager)
			.context("failed to serve objmanager")?
			.build()
			.await
			.context("failed to build")?;

		let proxy = BatteryProviderManagerProxy::builder(&conn)
			.interface("org.bluez.BatteryProviderManager1")
			.context("failed to set battery manager interface")?
			.destination("org.bluez")
			.context("failed to set battery manager destination")?
			.path(format!("/org/bluez/{}", &adapter_name))
			.context("failed to set battery manager path")?
			.build()
			.await
			.context("failed to create battery manager proxy")?;

		proxy
			.register_battery_provider(
				OwnedObjectPath::try_from(AIRPODSD_SERVE_PATH)
					.context("failed to parse battery provider path")?,
			)
			.await
			.context("failed to register battery provider")?;

		Ok(Self { conn, adapter_name })
	}

	fn calculate_percentage(
		_case: BatteryStatus,
		left: BatteryStatus,
		right: BatteryStatus,
	) -> Option<u8> {
		match (left.as_percent(), right.as_percent()) {
			(Some(l), Some(r)) => Some((l + r) / 2),
			(Some(l), None) => Some(l),
			(None, Some(r)) => Some(r),
			(None, None) => None,
		}
	}

	pub async fn update_battery(
		&mut self,
		address: Address,
		case: BatteryStatus,
		left: BatteryStatus,
		right: BatteryStatus,
	) -> Result<()> {
		let dev = address.to_string().replace(":", "_");
		let iface_name = OwnedObjectPath::try_from(format!("{AIRPODSD_SERVE_PATH}/dev_{dev}"))
			.context("failed to parse airpodsd battery interface path")?;
		let bluez_name =
			OwnedObjectPath::try_from(format!("/org/bluez/{}/dev_{dev}", self.adapter_name))
				.context("failed to parse bluez battery interface path")?;
		let percent = Self::calculate_percentage(case, left, right);

		let iface = self
			.conn
			.object_server()
			.interface::<_, BluezBattery>(&iface_name)
			.await
			.ok();

		match (iface, percent) {
			(Some(iface), Some(percent)) => {
				let mut iface_ref = iface.get_mut().await;
				iface_ref.percentage = percent;
				iface_ref
					.percentage_changed(iface.signal_emitter())
					.await
					.context("failed to fire percentage changed signal")?;
			}
			(Some(iface), None) => {
				drop(iface);
				self.conn
					.object_server()
					.remove::<BluezBattery, _>(&iface_name)
					.await
					.context("failed to remove battery from manager")?;
			}
			(None, Some(percent)) => {
				self.conn
					.object_server()
					.at(
						&iface_name,
						BluezBattery {
							device: bluez_name.clone(),
							percentage: percent,
						},
					)
					.await
					.context("failed to add battery to manager")?;
			}
			(None, None) => {
				// nothing to do
			}
		}

		Ok(())
	}
}
