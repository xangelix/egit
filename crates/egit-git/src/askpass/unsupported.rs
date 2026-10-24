//! Where no program asks anything (egit in a browser, whose programs run through a relay, which
//! asks through its own askpass): no helper connects, and nothing is listened on.

#![allow(
    clippy::unused_self,
    clippy::unnecessary_wraps,
    clippy::missing_const_for_fn,
    reason = "the shape of every platform's own module, which these stand in for"
)]

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The helper's name, as a relay's is.
pub(crate) const HELPER_NAME: &str = "egit-askpass";

/// A connection from a helper: none ever comes here.
#[derive(Debug)]
pub(crate) struct Stream;

/// Where helpers would connect: never bound here.
pub(crate) struct Listener;

/// What would stop a [`Listener`].
#[derive(Debug)]
pub(crate) struct Stopper;

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "no program asks anything here: a relay asks for them",
    )
}

impl Stream {
    pub(crate) fn set_nonblocking(&self, _nonblocking: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Err(unsupported())
    }
}

impl io::Read for Stream {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(unsupported())
    }
}

impl io::Read for &Stream {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(unsupported())
    }
}

impl io::Write for Stream {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(unsupported())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(unsupported())
    }
}

impl Listener {
    pub(crate) fn bind(_dir: &Path) -> io::Result<(Self, Stopper, OsString)> {
        Err(unsupported())
    }

    pub(crate) fn serve(&self, _accepted: impl FnMut(Stream) -> bool) {}
}

impl Stopper {
    pub(crate) const fn stop(&self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn connect(_address: &OsStr) -> io::Result<Stream> {
    Err(unsupported())
}

pub(crate) const fn same_user(_stream: &Stream) -> io::Result<bool> {
    Ok(false)
}

pub(crate) const fn hung_up(_stream: &Stream) -> bool {
    true
}

pub(crate) fn private_base() -> io::Result<PathBuf> {
    Err(unsupported())
}

pub(crate) fn make_private_dir(_dir: &Path) -> io::Result<()> {
    Err(unsupported())
}

pub(crate) fn ensure_private(_dir: &Path) -> io::Result<()> {
    Err(unsupported())
}

pub(crate) fn link_helper(_executable: &Path, _helper: &Path) -> io::Result<()> {
    Err(unsupported())
}

/// A server's directory kept in use: never here.
#[derive(Debug)]
pub(crate) struct Presence;

pub(crate) fn mark_present(_dir: &Path) -> io::Result<Presence> {
    Err(unsupported())
}

pub(crate) const fn is_stale(_dir: &Path, _pid: u32) -> bool {
    false
}
