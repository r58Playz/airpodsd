use anyhow::{Context, Result};
use tokio::net::UnixStream;

use crate::daemon::Address;

pub mod status;

async fn connect(addr: Address) -> Result<UnixStream> {
	UnixStream::connect(format!("\0dev.r58playz.airpodsd.{addr}"))
		.await
		.context("failed to connect to daemon")
}
