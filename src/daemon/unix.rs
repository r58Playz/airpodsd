use std::sync::Arc;

use anyhow::{Context, Result};
use event_listener::Event;
use log::{info, warn};
use tokio::{
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
	select,
	sync::Mutex,
};

use super::{Address, PodsStatus};

enum ListenerEvent {
	ReadLine(String),
	Update,
	Exit,
}

fn serialize_status(status: &PodsStatus) -> Result<Vec<u8>> {
	let mut vec = serde_json::to_vec(&status).context("failed to serialize status")?;
	vec.extend_from_slice(b"\n");
	Ok(vec)
}

async fn write_status(tx: &mut OwnedWriteHalf, status: &Mutex<PodsStatus>) -> Result<()> {
	tx.write_all(&serialize_status(&*status.lock().await)?)
		.await
		.context("failed to write initial status to listener")
}

async fn handle_listener(
	conn: UnixStream,
	status: Arc<Mutex<PodsStatus>>,
	notify: Arc<Event>,
) -> Result<()> {
	let (rx, mut tx) = conn.into_split();
	let mut rx = BufReader::new(rx).lines();
	write_status(&mut tx, &status).await?;

	loop {
		match select! {
			x = rx.next_line() => {
				match x? {
					Some(x) => ListenerEvent::ReadLine(x),
					None => ListenerEvent::Exit,
				}
			},
			_ = notify.listen() => {
				ListenerEvent::Update
			}
		} {
			ListenerEvent::ReadLine(x) => {
				warn!("ignoring received line {:?}", x);
			}
			ListenerEvent::Update => write_status(&mut tx, &status).await?,
			ListenerEvent::Exit => break,
		}
	}

	Ok(())
}

pub async fn unix_listener_main(
	addr: Address,
	status: Arc<Mutex<PodsStatus>>,
	notify: Arc<Event>,
) -> Result<()> {
	let sock = UnixListener::bind(format!("\0dev.r58playz.airpodsd.{addr}"))
		.context("failed to bind to unix socket")?;

	while let Ok((conn, addr)) = sock.accept().await {
		info!("accepted client at addr {:?}", addr);
		tokio::spawn(handle_listener(conn, status.clone(), notify.clone()));
	}

	Ok(())
}
