use std::path::PathBuf;

use anyhow::{Context, Result};
use log::LevelFilter;

use crate::daemon::Daemon;

mod battery;
mod bluetooth;
mod keystore;
mod daemon;

#[tokio::main]
async fn main() -> Result<()> {
	env_logger::builder()
		.filter_level(LevelFilter::Off)
		.filter_module("airpodsd", LevelFilter::Debug)
		.parse_default_env()
		.init();

	let daemon = Daemon::new(PathBuf::from("/home/r58playz/keystore"))
		.await
		.context("failed to create daemon instance")?;

	daemon
		.monitor()
		.await
		.context("failed to monitor airpods devices")?;

	Ok(())
}
