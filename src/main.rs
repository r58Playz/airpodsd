use std::{os::{linux::net::SocketAddrExt, unix::net::SocketAddr}, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::LevelFilter;
use tokio::{io::{AsyncWriteExt, stdout}, net::UnixStream};

use crate::daemon::Daemon;

mod battery;
mod bluetooth;
mod daemon;
mod keystore;

#[derive(Debug, Parser)]
struct Cli {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
	/// Run the daemon.
	#[command(arg_required_else_help = true)]
	Daemon {
		/// Path to keystore location. Used to store BLE advertisement keys.
		keystore: PathBuf,

		/// Timeout (in seconds) until inaccessible devices get dropped from airpodsd's battery reporting.
		#[clap(short, long, default_value_t = 60)]
		battery_drop_timeout: u64,

		/// Timeout (in seconds) until inaccessible devices are removed from airpodsd's tracking entirely.
		#[clap(short, long, default_value_t = 120)]
		device_drop_timeout: u64,
	},
	/// Fetch detailed device status
	Status {
		#[clap(short, long)]
		short: bool,
	},
}

#[tokio::main]
async fn main() -> Result<()> {
	env_logger::builder()
		.filter_level(LevelFilter::Off)
		.filter_module("airpodsd", LevelFilter::Debug)
		.parse_default_env()
		.init();

	let args = Cli::parse();

	match args.command {
		Commands::Daemon {
			keystore,
			battery_drop_timeout,
			device_drop_timeout,
		} => {
			let daemon = Daemon::new(
				keystore,
				Duration::from_secs(battery_drop_timeout),
				Duration::from_secs(device_drop_timeout),
			)
			.await
			.context("failed to create daemon instance")?;

			daemon
				.monitor()
				.await
				.context("failed to monitor airpods devices")?;
		}
		Commands::Status { short } => {
			let unix = std::os::unix::net::UnixStream::connect_addr(
				&SocketAddr::from_abstract_name("dev.r58playz.airpodsd").unwrap(),
			)
			.context("failed to bind to daemon unix socket")?;
			unix.set_nonblocking(true)
				.context("failed to set nonblocking")?;
			let unix = UnixStream::from_std(unix).context("failed to convert to tokio")?;

			let (mut rx, mut tx) = unix.into_split();
			tx.write_all(if short { b"status" } else { b"detailed" }).await.context("failed to send to daemon")?;
			tx.shutdown().await.context("failed to close daemon conn")?;

			tokio::io::copy(&mut rx, &mut stdout()).await.context("failed to forward response")?;
		}
	}

	Ok(())
}
