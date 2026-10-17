//! Windows: helpers reach the window through a socket on the loopback address. The standard
//! library has no local sockets here; any local process may connect to the port, so the token
//! from the environment (256 random bits, which other users' processes can't read) is what lets
//! a helper in. The server's directory is in the user's own temporary directory.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The helper's name: a link to egit's executable.
pub(crate) const HELPER_NAME: &str = "egit-askpass.exe";

/// A connection from a helper.
pub(crate) type Stream = TcpStream;

/// Where helpers connect: a port on the loopback address.
pub(crate) struct Listener {
    listener: TcpListener,
    stopping: Arc<AtomicBool>,
}

/// Stops a [`Listener`]'s [`Listener::serve`] from any thread.
#[derive(Debug)]
pub(crate) struct Stopper {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
}

impl Listener {
    /// Listens on a free port; returns the listener, what stops it, and the address helpers
    /// are given.
    pub(crate) fn bind(_dir: &Path) -> io::Result<(Self, Stopper, OsString)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let stopping = Arc::new(AtomicBool::new(false));
        Ok((
            Self {
                listener,
                stopping: Arc::clone(&stopping),
            },
            Stopper { address, stopping },
            OsString::from(address.to_string()),
        ))
    }

    /// Hands each connection to `accepted` until stopped, or until `accepted` says to end.
    pub(crate) fn serve(&self, mut accepted: impl FnMut(Stream) -> bool) {
        loop {
            let accepting = self.listener.accept();
            if self.stopping.load(Ordering::Acquire) {
                return;
            }
            match accepting {
                Ok((stream, _)) => {
                    if !accepted(stream) {
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "askpass connection failed");
                    // Don't spin on an error that persists.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }
}

impl Stopper {
    /// Stops the listener's [`Listener::serve`]: it wakes to a connection of its own.
    pub(crate) fn stop(&self) -> io::Result<()> {
        self.stopping.store(true, Ordering::Release);
        TcpStream::connect(self.address).map(drop)
    }
}

/// Connects to the server at `address` (its port on the loopback address).
pub(crate) fn connect(address: &OsStr) -> io::Result<Stream> {
    let address: SocketAddr = address
        .to_str()
        .and_then(|address| address.parse().ok())
        .filter(|address: &SocketAddr| address.ip().is_loopback())
        .ok_or_else(|| io::Error::other("not an askpass server's address"))?;
    TcpStream::connect(address)
}

/// Whether the process at the other end of `stream` is this user's: not known here; the token
/// decides.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same shape as where the peer's user is known"
)]
pub(crate) const fn same_user(_stream: &Stream) -> io::Result<bool> {
    Ok(true)
}

/// Whether the other end of `stream` has gone away.
pub(crate) fn hung_up(stream: &Stream) -> bool {
    let mut byte = [0_u8; 1];
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let peeked = stream.peek(&mut byte);
    let restored = stream.set_nonblocking(false);
    match peeked {
        Ok(0) => true,
        Ok(_) => restored.is_err(),
        Err(error) => error.kind() != io::ErrorKind::WouldBlock,
    }
}

/// `egit-<user>` under the user's temporary directory.
pub(crate) fn private_base() -> io::Result<PathBuf> {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let name: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
    let base = std::env::temp_dir().join(format!("egit-{name}"));
    ensure_private(&base)?;
    Ok(base)
}

/// Makes `dir` (in the user's temporary directory, whose permissions it inherits).
pub(crate) fn make_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir(dir)
}

/// Creates `dir` unless it exists, then refuses it unless it is a directory (not a link to
/// somewhere else).
pub(crate) fn ensure_private(dir: &Path) -> io::Result<()> {
    match make_private_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if fs::symlink_metadata(dir)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} isn't a directory", dir.display()),
        ))
    }
}

/// Makes `helper` a link to `executable`: a hard link (which needs no privilege), or a copy on
/// another volume.
pub(crate) fn link_helper(executable: &Path, helper: &Path) -> io::Result<()> {
    fs::hard_link(executable, helper).or_else(|_| fs::copy(executable, helper).map(drop))
}

/// A file in a server's directory, held open without sharing while the server runs: another
/// egit can't remove it, which tells it the server is alive.
#[derive(Debug)]
pub(crate) struct Presence(#[expect(dead_code, reason = "held only to keep it open")] File);

/// Marks `dir` as in use by this process.
pub(crate) fn mark_present(dir: &Path) -> io::Result<Presence> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(dir.join("alive"))
        .map(Presence)
}

/// Whether the server of directory `dir` is gone: its presence file can be removed (or is
/// missing).
pub(crate) fn is_stale(dir: &Path, _pid: u32) -> bool {
    match fs::remove_file(dir.join("alive")) {
        Ok(()) => true,
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}
