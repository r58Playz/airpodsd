use std::{
	fmt::Display,
	io::{Error, Result},
	os::fd::{FromRawFd, OwnedFd},
	str::FromStr,
};

use anyhow::{Context, anyhow};
use libbluetooth::{
	bluetooth::{self, bdaddr_t},
	l2cap::sockaddr_l2,
};
use libc::sockaddr;
use tokio::task::spawn_blocking;

const L2CAP_SOCKADDR_LEN: usize = size_of::<sockaddr_l2>();

#[derive(Debug, Clone, Copy)]
pub struct Address([u8; 6]);

impl Address {
	pub fn into_inner(mut self) -> [u8; 6] {
		self.0.reverse();
		self.0
	}
}

impl Display for Address {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(
			f,
			"{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
			self.0[5], self.0[4], self.0[3], self.0[2], self.0[1], self.0[0]
		)
	}
}

impl FromStr for Address {
	type Err = anyhow::Error;

	fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
		let mut vec: Vec<_> = s
			.split(":")
			.map(|x| u8::from_str_radix(x, 16))
			.collect::<std::result::Result<_, _>>()
			.context("failed to parse numbers")?;
		vec.reverse();
		Ok(Address(<[u8; 6]>::try_from(vec).map_err(|_| {
			anyhow!("address was too long or too short")
		})?))
	}
}

#[derive(Clone, Copy)]
pub struct L2CapAddr(sockaddr_l2);

impl L2CapAddr {
	pub fn new(addr: Address, psm: u16) -> Self {
		Self(sockaddr_l2 {
			l2_family: bluetooth::AF_BLUETOOTH,
			l2_psm: psm,
			l2_cid: 0,
			l2_bdaddr: bdaddr_t { b: addr.0 },
			l2_bdaddr_type: 0,
		})
	}
}

fn checkerr(err: i32) -> Result<i32> {
	if err < 0 {
		Err(Error::last_os_error())
	} else {
		Ok(err)
	}
}

unsafe fn connect_inner_unsafe(addr: sockaddr_l2) -> Result<OwnedFd> {
	let fd = unsafe {
		checkerr(libc::socket(
			libc::AF_BLUETOOTH,
			libc::SOCK_STREAM,
			bluetooth::BTPROTO_L2CAP,
		))?
	};

	unsafe {
		checkerr(libc::connect(
			fd,
			&addr as *const sockaddr_l2 as *const sockaddr,
			L2CAP_SOCKADDR_LEN as u32,
		))?;
	}

	Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn connect_inner(addr: L2CapAddr) -> Result<tokio::net::UnixStream> {
	let fd = unsafe { connect_inner_unsafe(addr.0)? };
	let std = std::os::unix::net::UnixStream::from(fd);
	std.set_nonblocking(true)?;
	tokio::net::UnixStream::from_std(std)
}

pub async fn connect(addr: L2CapAddr) -> Result<tokio::net::UnixStream> {
	spawn_blocking(move || {
		connect_inner(addr)
	}).await?
}
