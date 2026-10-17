//! Unix: helpers reach the window through a socket in a directory only this user may enter,
//! and only this user's processes are let in.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use rustix::event::{PollFd, PollFlags};
use rustix::io::Errno;
use rustix::net::RecvFlags;

/// The helper's name: a link to egit's executable.
pub(crate) const HELPER_NAME: &str = "egit-askpass";

/// A connection from a helper.
pub(crate) type Stream = UnixStream;

/// Where helpers connect: a socket in the server's directory.
pub(crate) struct Listener {
    listener: UnixListener,
    /// Read when the server is stopping: its own pair of sockets, so stopping never depends on
    /// the listening socket's path still being there.
    woken: UnixStream,
}

/// Stops a [`Listener`]'s [`Listener::serve`] from any thread.
#[derive(Debug)]
pub(crate) struct Stopper(UnixStream);

impl Listener {
    /// Listens in `dir`; returns the listener, what stops it, and the address helpers are given.
    pub(crate) fn bind(dir: &Path) -> io::Result<(Self, Stopper, OsString)> {
        let socket = dir.join("socket");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let (stopper, woken) = UnixStream::pair()?;
        Ok((
            Self { listener, woken },
            Stopper(stopper),
            socket.into_os_string(),
        ))
    }

    /// Hands each connection to `accepted` until stopped, or until `accepted` says to end.
    pub(crate) fn serve(&self, mut accepted: impl FnMut(Stream) -> bool) {
        loop {
            let mut ready = [
                PollFd::new(&self.listener, PollFlags::IN),
                PollFd::new(&self.woken, PollFlags::IN),
            ];
            match rustix::event::poll(&mut ready, None) {
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(error) => {
                    tracing::error!(%error, "the askpass server stopped");
                    return;
                }
            }
            let [listening, woken] = &ready;
            if !woken.revents().is_empty() {
                return;
            }
            if listening.revents().is_empty() {
                continue;
            }
            loop {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        if !accepted(stream) {
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        tracing::warn!(%error, "askpass connection failed");
                        // Don't spin on an error that persists (no file descriptors left, say).
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        break;
                    }
                }
            }
        }
    }
}

impl Stopper {
    /// Stops the listener's [`Listener::serve`].
    pub(crate) fn stop(&self) -> io::Result<()> {
        (&self.0).write_all(&[1])
    }
}

/// Connects to the server at `address` (its socket).
pub(crate) fn connect(address: &OsStr) -> io::Result<Stream> {
    UnixStream::connect(address)
}

/// Whether the process at the other end of `stream` is this user's.
pub(crate) fn same_user(stream: &Stream) -> io::Result<bool> {
    Ok(peer::uid(stream)? == rustix::process::getuid().as_raw())
}

/// Whether the other end of `stream` has gone away.
pub(crate) fn hung_up(stream: &Stream) -> bool {
    let mut byte = [0_u8; 1];
    match rustix::net::recv(stream, &mut byte, RecvFlags::PEEK | RecvFlags::DONTWAIT) {
        // The other end closed.
        Ok((0, _)) => true,
        Ok(_) | Err(Errno::AGAIN | Errno::INTR) => false,
        Err(_) => true,
    }
}

/// `egit-<uid>` under `$XDG_RUNTIME_DIR`, or under the temporary directory when the runtime
/// directory is missing or unusable.
pub(crate) fn private_base() -> io::Result<PathBuf> {
    let name = format!("egit-{}", rustix::process::getuid().as_raw());
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    let mut failure = None;
    for parent in runtime.into_iter().chain([std::env::temp_dir()]) {
        let base = parent.join(&name);
        match ensure_private(&base) {
            Ok(()) => return Ok(base),
            Err(error) => {
                tracing::debug!(%error, base = %base.display(), "not a usable askpass directory");
                failure = Some(error);
            }
        }
    }
    Err(failure.unwrap_or_else(|| io::Error::other("no directory for the askpass socket")))
}

/// Makes `dir`, which only this user may enter.
pub(crate) fn make_private_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(dir)
}

/// Creates `dir` (0700) unless it exists, then refuses it unless it is a directory this user
/// owns that nobody else may enter.
pub(crate) fn ensure_private(dir: &Path) -> io::Result<()> {
    match make_private_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(dir)?;
    let ours = metadata.uid() == rustix::process::getuid().as_raw();
    #[expect(
        clippy::verbose_bit_mask,
        reason = "permission bits read best as a mask"
    )]
    let closed_to_others = metadata.mode() & 0o077 == 0;
    if metadata.is_dir() && ours && closed_to_others {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} isn't a directory private to this user", dir.display()),
        ))
    }
}

/// Makes `helper` a link to `executable`.
pub(crate) fn link_helper(executable: &Path, helper: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(executable, helper)
}

/// Marks a server's directory as in use while it is kept (nothing to keep here: a server is
/// known by its process).
#[derive(Debug)]
pub(crate) struct Presence;

/// Marks `dir` as in use by this process.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same shape as where a file is held open"
)]
pub(crate) const fn mark_present(_dir: &Path) -> io::Result<Presence> {
    Ok(Presence)
}

/// Whether the server of directory `dir`, made by process `pid`, is gone.
pub(crate) fn is_stale(_dir: &Path, pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    rustix::process::test_kill_process(pid) == Err(Errno::SRCH)
}

/// The peer's user, which the kernel vouches for.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod peer {
    use std::io;

    use super::Stream;

    pub(super) fn uid(stream: &Stream) -> io::Result<u32> {
        Ok(rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw())
    }
}

/// The peer's user, which the kernel vouches for: `getpeereid`, which the standard library and
/// rustix don't call here.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod peer {
    #![expect(
        unsafe_code,
        reason = "`getpeereid` has no safe wrapper on this platform"
    )]

    use std::io;
    use std::os::fd::AsRawFd as _;

    use super::Stream;

    pub(super) fn uid(stream: &Stream) -> io::Result<u32> {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: an open socket's descriptor, borrowed for the call, and two places to write to.
        let status = unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) };
        if status == 0 {
            Ok(uid)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
