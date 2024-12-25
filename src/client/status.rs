use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::daemon::{Address, PodsStatus};

use super::connect;

fn print_status(addr: Address, status: PodsStatus) {
	println!("Status for device {addr}:");
	println!(
		"\tBattery: Case {:?} Left {:?} Right {:?}",
		status.battery.case, status.battery.left, status.battery.right
	);
	println!("\tNoise control: {:?}", status.noise);
	println!(
		"\tEar detection: Primary {:?} Secondary {:?}",
		status.ear.primary, status.ear.secondary
	);
}

pub async fn get(addr: Address) -> Result<()> {
	let sock = connect(addr).await?;
	let mut sock = BufReader::new(sock).lines();
	let status = sock
		.next_line()
		.await
		.context("failed to read status from server")?
		.context("server returned no status")?;
	sock.into_inner()
		.shutdown()
		.await
		.context("failed to close connection to daemon")?;
	let decoded =
		serde_json::from_str::<PodsStatus>(&status).context("failed to deserialize status")?;

	print_status(addr, decoded);
	Ok(())
}

pub async fn watch(addr: Address) -> Result<()> {
	let sock = connect(addr).await?;
	let mut sock = BufReader::new(sock).lines();
	while let Some(status) = sock
		.next_line()
		.await
		.context("failed to read status from server")?
	{
		let decoded =
			serde_json::from_str::<PodsStatus>(&status).context("failed to deserialize status")?;
		print_status(addr, decoded);
		println!();
	}
	Ok(())
}
