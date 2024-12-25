use anyhow::{Context, Result, bail};
use bytes::{Buf, Bytes};
use log::warn;
use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
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
			(_, 0x00) => BatteryStatus::Unknown,
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
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
			x => bail!("invalid ear detection status: {:x?}", x),
		})
	}
}

#[derive(Debug, Clone)]
pub enum ParsedPacket {
	Battery(Vec<Battery>),
	NoiseControl(NoiseControlStatus),
	EarDetection {
		primary: EarDetectionStatus,
		secondary: EarDetectionStatus,
	},
}

impl ParsedPacket {
	pub fn decode(mut data: Bytes) -> Result<Option<Self>> {
		if data.remaining() < 6 {
			bail!("packet is too small");
		}

		match data.split_to(4).as_ref() {
			[0x04, 0x00, 0x04, 0x00] => {}
			x => {
				warn!("ignoring packet with invalid header {:x?}: {:x?}", x, data);
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
						warn!(
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
			x => {
				warn!("ignoring unknown packet of type {:x?}: {:x?}", x, data);
				Ok(None)
			}
		}
	}
}
