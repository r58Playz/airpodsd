use anyhow::{Context, Result, bail};
use bytes::{Buf, Bytes};
use log::trace;

pub const HANDSHAKE_PACKET: &'static [u8] = &[
	0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
pub const ENABLE_FEATURES: &'static [u8] = &[
	0x04, 0x00, 0x04, 0x00, 0x4d, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
pub const ENABLE_NOTIFICATIONS: &'static [u8] =
	&[0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
pub const GET_CLOUD_KEYS: &'static [u8] = &[0x04, 0x00, 0x04, 0x00, 0x30, 0x00, 0x05, 0x00];

trait Decode {
	fn decode(data: &mut Bytes) -> Result<Self>
	where
		Self: Sized;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryComponent {
	Case,
	Left,
	Right,
}

impl Decode for BatteryComponent {
	fn decode(data: &mut Bytes) -> Result<Self> {
		Ok(match data.get_u8() {
			0x08 => Self::Case,
			0x04 => Self::Left,
			0x02 => Self::Right,
			x => bail!("invalid battery component: {:x?}", x),
		})
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
	Unknown,
	Disconnected,
	Charging(u8),
	Discharging(u8),
}

impl BatteryStatus {
	pub fn as_percent(&self) -> Option<u8> {
		match self {
			Self::Charging(x) | Self::Discharging(x) => Some(*x),
			Self::Unknown | Self::Disconnected => None,
		}
	}
}

impl Decode for BatteryStatus {
	fn decode(data: &mut Bytes) -> Result<Self> {
		Ok(match (data.get_u8(), data.get_u8()) {
			(_, 0x00) | (_, 0x03) => BatteryStatus::Unknown,
			(x, 0x01) => BatteryStatus::Charging(x),
			(x, 0x02) => BatteryStatus::Discharging(x),
			(_, 0x04) => BatteryStatus::Disconnected,
			x => bail!("invalid battery status: {:x?}", x),
		})
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
	pub component: BatteryComponent,
	pub status: BatteryStatus,
}

impl Decode for Battery {
	fn decode(data: &mut Bytes) -> Result<Self> {
		if data.remaining() < 5 {
			bail!("battery packet is too small");
		}

		let component = BatteryComponent::decode(data)?;

		if data.get_u8() != 0x01 {
			bail!("spacer between component and level is not 0x01");
		}

		let status = BatteryStatus::decode(data)?;

		if data.get_u8() != 0x01 {
			bail!("spacer after status is not 0x01");
		}

		Ok(Self { component, status })
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseControlStatus {
	Off,
	NoiseCancellation,
	Transparency,
	AdaptiveTransparency,
}

impl Decode for NoiseControlStatus {
	fn decode(data: &mut Bytes) -> Result<Self> {
		if data.remaining() < 1 {
			bail!("packet too small");
		}

		Ok(match data.get_u8() {
			0x01 => Self::Off,
			0x02 => Self::NoiseCancellation,
			0x03 => Self::Transparency,
			0x04 => Self::AdaptiveTransparency,
			x => bail!("invalid noise control status: {:x?}", x),
		})
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarDetectionStatus {
	InEar,
	OutOfEar,
	InCase,
}

impl Decode for EarDetectionStatus {
	fn decode(data: &mut Bytes) -> Result<Self> {
		if data.remaining() < 1 {
			bail!("packet too small");
		}

		Ok(match data.get_u8() {
			0x00 => Self::InEar,
			0x01 => Self::OutOfEar,
			0x02 => Self::InCase,
			0x03 => Self::OutOfEar, // TODO find the real value for this
			x => bail!("invalid ear detection status: {:x?}", x),
		})
	}
}

#[derive(Debug, Clone)]
pub enum L2CapPacket {
	Battery(Vec<Battery>),
	NoiseControl(NoiseControlStatus),
	EarDetection {
		primary: EarDetectionStatus,
		secondary: EarDetectionStatus,
	},
	Info {
		name: String,
		model: String,
		serial: String,
	},
	CloudKeys {
		irk: Bytes,
		enc_key: Bytes,
	},
}

impl L2CapPacket {
	pub fn decode(mut data: Bytes) -> Result<Option<Self>> {
		if data.remaining() < 6 {
			bail!("packet is too small");
		}

		match data.split_to(4).as_ref() {
			[0x04, 0x00, 0x04, 0x00] => {}
			x => {
				trace!("ignoring packet with invalid header {:x?}: {:x?}", x, data);
				return Ok(None);
			}
		}

		match data.split_to(2).as_ref() {
			[0x04, 0x00] => {
				// Battery
				if data.remaining() < 1 {
					bail!("battery packet is too small");
				}

				let mut vec = Vec::with_capacity(3);

				for _ in 0..data.get_u8() {
					vec.push(Battery::decode(&mut data).context("failed to parse battery")?);
				}

				Ok(Some(Self::Battery(vec)))
			}
			[0x09, 0x00] => {
				// Noise control
				if data.remaining() < 1 {
					bail!("packet too small");
				}

				match data.get_u8() {
					0x0D => {
						let decoded = NoiseControlStatus::decode(&mut data)
							.context("failed to parse noise control status")?;
						Ok(Some(Self::NoiseControl(decoded)))
					}
					x => {
						trace!(
							"ignoring unknown packet of type [0x09, 0x00] and secondary type {:x?}: {:x?}",
							x, data
						);
						// some other packet also has 0x09 0x00 but not 0x0D so we ignore in this case
						Ok(None)
					}
				}
			}
			[0x06, 0x00] => {
				// Ear detection
				let primary = EarDetectionStatus::decode(&mut data)
					.context("failed to parse primary ear detection status")?;
				let secondary = EarDetectionStatus::decode(&mut data)
					.context("failed to parse secondary ear detection status")?;

				Ok(Some(Self::EarDetection { primary, secondary }))
			}
			[0x1d, 0x00] => {
				// Info
				dbg!(&data);
				let _unknown_1 = data.split_to(
					data.iter()
						.position(|x| *x == b'\0')
						.context("failed to find string terminator")?
						+ 1,
				);
				let _unknown_2 = data.split_to(
					data.iter()
						.position(|x| *x == b'\0')
						.context("failed to find string terminator")?
						+ 1,
				);

				let name = String::from_utf8(
					data.split_to(
						data.iter()
							.position(|x| *x == b'\0')
							.context("failed to find string terminator")?,
					)
					.to_vec(),
				)
				.context("invalid string")?;
				let _null = data.get_u8();
				let model = String::from_utf8(
					data.split_to(
						data.iter()
							.position(|x| *x == b'\0')
							.context("failed to find string terminator")?,
					)
					.to_vec(),
				)
				.context("invalid string")?;
				let _null = data.get_u8();

				let _apple_inc = data.split_to(
					data.iter()
						.position(|x| *x == b'\0')
						.context("failed to find string terminator")?
						+ 1,
				);

				let serial = String::from_utf8(
					data.split_to(
						data.iter()
							.position(|x| *x == b'\0')
							.context("failed to find string terminator")?,
					)
					.to_vec(),
				)
				.context("invalid string")?;
				let _null = data.get_u8();

				Ok(Some(Self::Info {
					name,
					model,
					serial,
				}))
			}
			[0x31, 0x00] => {
				// Cloud magic keys
				let count = data.get_u8();

				let mut irk: Option<Bytes> = None;
				let mut enc_key: Option<Bytes> = None;

				for _ in 0..count {
					let ty = data.get_u16_le();
					let len = data.get_u16_le();

					let key = data.split_to(len as usize);

					match ty {
						0x01 => irk = Some(key),
						0x04 => enc_key = Some(key),
						ty => bail!("unknown key type {ty:?}"),
					}
				}

				Ok(irk
					.zip(enc_key)
					.map(|(irk, enc_key)| Self::CloudKeys { irk, enc_key }))
			}
			x => {
				trace!("ignoring unknown packet of type {:x?}: {:x?}", x, data);
				Ok(None)
			}
		}
	}
}

pub struct ProximityPairingPacket {}
