//! Sharing ssh connections while egit runs, so a key's passphrase (or a password, or a
//! security key's touch) is asked once rather than for every fetch, pull, and push.
//!
//! git starts `ssh` for each command that reaches a remote over SSH, and without an agent
//! holding the key each one asks for its passphrase. OpenSSH can share one connection to a
//! host between them (`ControlMaster`): the first becomes the master, kept open after it ends
//! (`ControlPersist`), and the later ones run through its socket without authenticating again.
//! [`SshSharing`] points git at a wrapper asking for that (`GIT_SSH`, a script in a directory
//! only this user may enter, beside the sockets), which git runs only where nothing else names
//! the ssh command: `GIT_SSH_COMMAND` and `core.sshCommand` outrank it, so a repository's own
//! choice still holds. The masters are closed when it is dropped (`ssh -O exit`), and close
//! themselves after [`KEPT_IDLE`] unused, or once keepalives find the network gone.
//!
//! Unix only: OpenSSH for Windows can't share connections.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// How long a shared connection stays open with nothing using it.
pub const KEPT_IDLE: &str = "1h";

/// ssh connections shared between git's commands while this is kept.
///
/// Holds a directory of its own (`<base>/<pid>-<nonce>`, removed when dropped) with the
/// wrapper and the masters' sockets; [`Self::environment`] makes git use them.
#[derive(Debug)]
pub struct SshSharing {
    dir: PathBuf,
    wrapper: PathBuf,
}

impl SshSharing {
    /// Shares connections from a directory private to this user (`egit-<uid>/ssh` under
    /// `$XDG_RUNTIME_DIR`, `/tmp`, or the temporary directory: the first whose sockets' paths
    /// fit), unless egit's own environment names the ssh command (`GIT_SSH_COMMAND`,
    /// `GIT_SSH`): that choice stays. `None` where connections can't be shared (on Windows,
    /// say); why is logged.
    #[must_use]
    pub fn start() -> Option<Self> {
        if let Some(name) = ["GIT_SSH_COMMAND", "GIT_SSH"]
            .into_iter()
            .find(|name| std::env::var_os(name).is_some())
        {
            tracing::debug!("{name} is set: ssh connections aren't shared");
            return None;
        }
        platform::start()
    }

    /// As [`Self::start`], inside `base`, which only this user may enter (made if missing),
    /// whatever the environment says.
    ///
    /// # Errors
    ///
    /// When `base` isn't private, the sockets' paths would be too long (or hold characters a
    /// shell or ssh would read), the directory or wrapper can't be made, or on Windows.
    pub fn start_in(base: &Path) -> io::Result<Self> {
        platform::start_in(base)
    }

    /// The variables that make git share connections: `GIT_SSH`, the wrapper.
    #[must_use]
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        vec![(OsString::from("GIT_SSH"), self.wrapper.clone().into())]
    }

    /// The directory holding the wrapper and the masters' sockets.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for SshSharing {
    fn drop(&mut self) {
        platform::close_masters(&self.dir);
        if let Err(error) = std::fs::remove_dir_all(&self.dir) {
            tracing::warn!(%error, dir = %self.dir.display(), "couldn't remove the ssh directory");
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::fs;
    use std::io::{self, Write as _};
    use std::os::unix::fs::{FileTypeExt as _, OpenOptionsExt as _};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use super::{KEPT_IDLE, SshSharing};
    use crate::askpass::{ensure_private, is_stale, make_private_dir};

    /// The longest socket path the system takes, its terminating NUL counted (`sun_path`).
    const SOCKET_PATH: usize = if cfg!(any(target_os = "linux", target_os = "android")) {
        108
    } else {
        104
    };

    /// What ssh adds to the directory for a socket: `/`, `%C` (40 hex digits), and while a
    /// master starts, a `.` and 16 random characters.
    const SOCKET_NAME: usize = 1 + 40 + 17;

    /// How long closing a master may take before its `ssh -O exit` is given up on.
    const CLOSING: Duration = Duration::from_secs(2);

    pub(super) fn start() -> Option<SshSharing> {
        let name = format!("egit-{}", rustix::process::getuid().as_raw());
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        let parents = runtime
            .into_iter()
            .chain([PathBuf::from("/tmp"), std::env::temp_dir()]);
        for parent in parents {
            let private = parent.join(&name);
            let started = ensure_private(&private).and_then(|()| start_in(&private.join("ssh")));
            match started {
                Ok(sharing) => return Some(sharing),
                Err(error) => {
                    tracing::debug!(%error, base = %private.display(), "can't share ssh connections from here");
                }
            }
        }
        None
    }

    pub(super) fn start_in(base: &Path) -> io::Result<SshSharing> {
        ensure_private(base)?;
        sweep_stale(base);
        let mut nonce = [0_u8; 4];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let dir = base.join(format!(
            "{}-{:08x}",
            std::process::id(),
            u32::from_le_bytes(nonce)
        ));
        let Some(path) = dir.to_str().filter(|path| path.bytes().all(plain)) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} holds characters ssh or a shell would read",
                    dir.display()
                ),
            ));
        };
        if path.len().saturating_add(SOCKET_NAME) >= SOCKET_PATH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{path} is too long a path for ssh's sockets"),
            ));
        }
        make_private_dir(&dir)?;
        let wrapper = dir.join("ssh");
        let written = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&wrapper)
            .and_then(|mut file| file.write_all(script(path).as_bytes()));
        if let Err(error) = written {
            // Nothing in it is in use yet.
            let _ignored = fs::remove_dir_all(&dir);
            return Err(error);
        }
        Ok(SshSharing { dir, wrapper })
    }

    /// The wrapper git runs as its ssh: ssh itself (found on `PATH`, as git finds it), sharing
    /// connections through sockets in `dir`. Keepalives every 15 seconds close a master whose
    /// network has gone (a laptop woken elsewhere) within a minute, rather than leaving the
    /// commands sent through it waiting.
    fn script(dir: &str) -> String {
        format!(
            "#!/bin/sh\n\
             # egit's: one connection to a host shared while egit runs.\n\
             exec ssh -o ControlMaster=auto -o ControlPersist={KEPT_IDLE} \
             -o ServerAliveInterval=15 -o 'ControlPath={dir}/%C' \"$@\"\n"
        )
    }

    /// Whether a path's byte is read as itself by the shell (inside single quotes) and by ssh
    /// (no `%` or `$` expansion, no quoting or spaces).
    const fn plain(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+' | b'@')
    }

    /// Closes every master whose socket is in `dir`, at once, waiting at most [`CLOSING`].
    pub(super) fn close_masters(dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let closing: Vec<Child> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_socket()))
            .filter_map(|entry| {
                Command::new("ssh")
                    // Not the user's configuration: nothing in it applies to a socket's path.
                    .args(["-F", "/dev/null", "-S"])
                    .arg(entry.path())
                    .args(["-O", "exit", "egit"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .inspect_err(|error| tracing::debug!(%error, "couldn't close an ssh master"))
                    .ok()
            })
            .collect();
        let until = Instant::now().checked_add(CLOSING);
        for mut child in closing {
            while matches!(child.try_wait(), Ok(None))
                && until.is_some_and(|until| Instant::now() < until)
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            // ssh, not git: nothing of it is left to clean up.
            if matches!(child.try_wait(), Ok(None)) {
                let _ignored = child.kill();
            }
            let _ignored = child.wait();
        }
    }

    /// Removes the directories of egit processes that ended without removing their own (their
    /// masters closed first, so none lingers on).
    fn sweep_stale(base: &Path) {
        let Ok(entries) = fs::read_dir(base) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|name| name.split_once('-'))
                .and_then(|(pid, _)| pid.parse::<u32>().ok())
            else {
                continue;
            };
            if pid != std::process::id()
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && is_stale(&entry.path(), pid)
            {
                close_masters(&entry.path());
                if let Err(error) = fs::remove_dir_all(entry.path()) {
                    tracing::debug!(%error, "couldn't sweep a stale ssh directory");
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_wrapper_shares_through_its_directory() {
            let script = script("/run/user/1000/egit-1000/ssh/42-0a1b2c3d");
            assert!(script.starts_with("#!/bin/sh\n"));
            assert!(script.contains(
                "exec ssh -o ControlMaster=auto -o ControlPersist=1h -o ServerAliveInterval=15 \
                 -o 'ControlPath=/run/user/1000/egit-1000/ssh/42-0a1b2c3d/%C' \"$@\""
            ));
            assert!("/tmp/egit-501/ssh".bytes().all(plain));
            for odd in ["/a b", "/a%b", "/a$b", "/a'b", "/a\"b", "/a\\b", "/日本"] {
                assert!(!odd.bytes().all(plain), "{odd}");
            }
        }

        #[test]
        fn a_base_too_deep_for_sockets_is_refused() {
            let root = tempfile::tempdir().unwrap();
            let deep = root.path().join("d".repeat(SOCKET_PATH));
            let refused = start_in(&deep).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);

            let sharing = start_in(&root.path().join("ssh")).unwrap();
            let dir = sharing.dir().to_owned();
            assert!(dir.join("ssh").is_file());
            drop(sharing);
            assert!(!dir.exists());
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::io;
    use std::path::Path;

    use super::SshSharing;

    pub(super) const fn start() -> Option<SshSharing> {
        None
    }

    pub(super) fn start_in(_base: &Path) -> io::Result<SshSharing> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OpenSSH for Windows can't share connections",
        ))
    }

    pub(super) const fn close_masters(_dir: &Path) {}
}
