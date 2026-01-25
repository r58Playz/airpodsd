use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use bluer::Address;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct CloudKeys {
	pub irk: Bytes,
	pub enc_key: Bytes,
}
impl CloudKeys {
	fn parse(bytes: &mut Bytes) -> CloudKeys {
		let irk_len = bytes.get_u8().into();
		let irk = bytes.split_to(irk_len);

		let enc_key_len = bytes.get_u8().into();
		let enc_key = bytes.split_to(enc_key_len);

		CloudKeys { irk, enc_key }
	}

	fn serialize(&self, bytes: &mut BytesMut) -> Result<()> {
		bytes.put_u8(self.irk.len().try_into().context("irk too long")?);
		bytes.extend_from_slice(&self.irk);

		bytes.put_u8(self.enc_key.len().try_into().context("enc_key too long")?);
		bytes.extend_from_slice(&self.enc_key);

		Ok(())
	}
}

pub struct KeyStore {
	path: PathBuf,
	keys: Mutex<HashMap<Address, CloudKeys>>,
}
impl KeyStore {
	fn parse_pair(bytes: &mut Bytes) -> (Address, CloudKeys) {
		let addr = Address(*bytes.split_to(6).as_array().unwrap());
		let keys = CloudKeys::parse(bytes);

		(addr, keys)
	}

	fn serialize_pair(bytes: &mut BytesMut, addr: &Address, keys: &CloudKeys) -> Result<()> {
		bytes.extend_from_slice(&addr.0);
		keys.serialize(bytes)?;

		Ok(())
	}

	fn serialize(keys: &mut HashMap<Address, CloudKeys>) -> Result<Bytes> {
		let mut bytes = BytesMut::new();

		for (addr, keys) in keys.iter() {
			Self::serialize_pair(&mut bytes, addr, keys)?;
		}

		Ok(bytes.freeze())
	}

	pub async fn new(path: PathBuf) -> Result<KeyStore> {
		let mut key_map = HashMap::new();

		if tokio::fs::try_exists(&path)
			.await
			.context("failed to check if keystore exists")?
		{
			let mut bytes = Bytes::from(
				tokio::fs::read(&path)
					.await
					.context("failed to read keystore")?,
			);
			while bytes.has_remaining() {
				let (addr, keys) = Self::parse_pair(&mut bytes);
				key_map.insert(addr, keys);
			}
		}

		Ok(Self {
			path,
			keys: Mutex::new(key_map),
		})
	}

	pub async fn has(&self, addr: &Address) -> bool {
		self.keys.lock().await.contains_key(addr)
	}

	pub async fn find(
		&self,
		mut func: impl FnMut(&CloudKeys) -> bool,
	) -> Option<(Address, CloudKeys)> {
		self.keys
			.lock()
			.await
			.iter()
			.find(|x| func(x.1))
			.map(|(a, b)| (*a, b.clone()))
	}

	pub async fn set(&self, addr: Address, keys: CloudKeys) -> Result<()> {
		let mut locked = self.keys.lock().await;
		locked.insert(addr, keys);
		let serialized = Self::serialize(&mut locked).context("")?;

		tokio::fs::write(&self.path, serialized)
			.await
			.context("failed to write keystore to disk")
	}
}
