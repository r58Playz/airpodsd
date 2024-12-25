use anyhow::Result;
use clap::{Parser, Subcommand};
use client::status;
use daemon::{Address, daemon_main};

mod client;
mod daemon;

#[derive(Debug, Parser)]
struct Cli {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
	/// Run the daemon.
	#[command(arg_required_else_help = true)]
	Daemon { mac_address: Address },
	/// Watch or get the status of a device.
	#[command(arg_required_else_help = true)]
	Status {
		mac_address: Address,
		#[clap(short, long)]
		watch: bool,
	},
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
	let args = Cli::parse();

	match args.command {
		Commands::Daemon { mac_address } => {
			daemon_main(mac_address).await?;
		}
		Commands::Status { mac_address, watch } => {
			if watch {
				status::watch(mac_address).await?;
			} else {
				status::get(mac_address).await?;
			}
		}
	}

	Ok(())
}
