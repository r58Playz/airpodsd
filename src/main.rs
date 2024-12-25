use anyhow::{Context, Result};

mod daemon;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
	let addr = std::env::args().nth(1).context("no address provided")?;
	daemon::daemon_main(addr.parse().context("address was invalid")?).await
}
