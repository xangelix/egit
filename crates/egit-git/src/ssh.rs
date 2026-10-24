//! Holding a key's passphrase, and sharing ssh connections, while egit runs, so a passphrase
//! (or a password, or a security key's touch) is asked once rather than for every fetch, pull,
//! and push.
//!
//! git starts `ssh` for each command that reaches a remote over SSH, and without an agent
//! holding the key each one asks for its passphrase. [`SshSharing`] points git at a wrapper
//! (`GIT_SSH`, a script in a directory only this user may enter) running ssh so that:
//!
//! - A key whose passphrase is typed is added to an agent (`AddKeysToAgent`), as `ssh-add`
//!   would: the one egit's environment names, where it answers, else one of egit's own, for
//!   git's processes alone ([`Holding`]), which ends with egit (however egit ends). Every later
//!   connection authenticates with it, asking nothing: a background fetch too, which can't ask.
//! - One connection to a host is shared between commands (`ControlMaster`): the first becomes
//!   the master, kept open after it ends (`ControlPersist`), and later ones run through its
//!   socket without authenticating again (faster; and a password, which no agent holds, is
//!   asked once). A server may refuse a second session on one connection, or drop it while
//!   idle: ssh then connects anew, with the key held.
//!
//! git runs the wrapper only where nothing else names the ssh command: `GIT_SSH_COMMAND` and
//! `core.sshCommand` outrank it, so a repository's own choice still holds (a key egit's agent
//! holds serves it too). The masters are closed when it is dropped (`ssh -O exit`), and close
//! themselves after [`KEPT_IDLE`] unused, or once keepalives find the network gone; egit's own
//! agent forgets its keys and ends.
//!
//! Unix only: OpenSSH for Windows can't share connections.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// How long a shared connection stays open with nothing using it.
pub const KEPT_IDLE: &str = "1h";

/// Where keys whose passphrase is typed are held, so they aren't asked for again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holding {
    /// In the agent egit's environment names (`SSH_AUTH_SOCK`), as `ssh-add` would.
    Theirs,
    /// In an agent of egit's own, for git's processes alone, ended with egit.
    Own,
    /// Nowhere: connections are shared, nothing more.
    Nowhere,
}

/// ssh connections shared between git's commands, and keys held, while this is kept.
///
/// Holds a directory of its own (`<base>/<pid>-<nonce>`, removed when dropped) with the
/// wrapper, the masters' sockets, and egit's own agent's; [`Self::environment`] makes git use
/// them.
#[derive(Debug)]
pub struct SshSharing {
    dir: PathBuf,
    wrapper: PathBuf,
    /// Where keys are held.
    holding: Holding,
    /// egit's own agent, when keys are held there.
    agent: Option<platform::Agent>,
}

impl SshSharing {
    /// Shares connections from a directory private to this user (`egit-<uid>/ssh` under
    /// `$XDG_RUNTIME_DIR`, `/tmp`, or the temporary directory: the first whose sockets' paths
    /// fit), unless egit's own environment names the ssh command (`GIT_SSH_COMMAND`,
    /// `GIT_SSH`): that choice stays. Keys are held in the agent the environment names where it
    /// answers, else in one of egit's own (where `ssh-agent` is installed). `None` where
    /// connections can't be shared (on Windows, say); why is logged.
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
    /// keys held as `holding` says, whatever the environment says.
    ///
    /// # Errors
    ///
    /// When `base` isn't private, the sockets' paths would be too long (or hold characters a
    /// shell or ssh would read), the directory or wrapper can't be made, egit's own agent can't
    /// be started (`Holding::Own`), or on Windows.
    pub fn start_in(base: &Path, holding: Holding) -> io::Result<Self> {
        platform::start_in(base, holding)
    }

    /// The variables that make git share connections and hold keys: `GIT_SSH`, the wrapper;
    /// and `SSH_AUTH_SOCK`, egit's own agent, when keys are held there.
    #[must_use]
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        let wrapper = (OsString::from("GIT_SSH"), self.wrapper.clone().into());
        // Only where egit has an agent of its own (Unix).
        #[cfg(unix)]
        let agent = self.agent.as_ref().map(|agent| {
            (
                OsString::from("SSH_AUTH_SOCK"),
                agent.socket().as_os_str().to_owned(),
            )
        });
        #[cfg(not(unix))]
        let agent: Option<(OsString, OsString)> = None;
        std::iter::once(wrapper).chain(agent).collect()
    }

    /// Where keys are held.
    #[must_use]
    pub const fn holding(&self) -> Holding {
        self.holding
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
        // Its keys forgotten at once; it ends as its keeper does.
        self.agent = None;
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
    use std::time::Duration;

    use web_time::Instant;

    use super::{Holding, KEPT_IDLE, SshSharing};
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

    /// How long egit's own agent may take to listen.
    const AGENT_START: Duration = Duration::from_secs(2);

    /// egit's own `ssh-agent`, its socket in the sharing directory. It runs `cat` as its
    /// command, `cat`'s input this: when it is closed (dropped, or egit ending however it
    /// ends), `cat` ends, and the agent with it (it looks every ten seconds), so no agent
    /// outlives egit holding keys.
    #[derive(Debug)]
    pub(crate) struct Agent {
        socket: PathBuf,
        keeper: Child,
    }

    impl Agent {
        /// Starts one listening at `socket`.
        fn start(socket: PathBuf) -> io::Result<Self> {
            let mut keeper = Command::new("ssh-agent")
                .arg("-a")
                .arg(&socket)
                .arg("cat")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let until = Instant::now().checked_add(AGENT_START);
            while !fs::symlink_metadata(&socket).is_ok_and(|meta| meta.file_type().is_socket()) {
                let ended = !matches!(keeper.try_wait(), Ok(None));
                if ended || until.is_none_or(|until| Instant::now() >= until) {
                    let _ignored = keeper.kill();
                    let _ignored = keeper.wait();
                    return Err(io::Error::other("ssh-agent didn't start"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(Self { socket, keeper })
        }

        /// Its socket.
        pub(crate) fn socket(&self) -> &Path {
            &self.socket
        }
    }

    impl Drop for Agent {
        fn drop(&mut self) {
            // Its keys forgotten at once (`ssh-add -D`), rather than when it notices.
            let forgetting = Command::new("ssh-add")
                .arg("-D")
                .env("SSH_AUTH_SOCK", &self.socket)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut forgetting) = forgetting {
                let until = Instant::now().checked_add(CLOSING);
                while matches!(forgetting.try_wait(), Ok(None))
                    && until.is_some_and(|until| Instant::now() < until)
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ignored = forgetting.kill();
                let _ignored = forgetting.wait();
            }
            // `cat`'s input closes: it ends, and the agent after it.
            drop(self.keeper.stdin.take());
            let _ignored = self.keeper.wait();
        }
    }

    /// Whether the agent egit's environment names (`SSH_AUTH_SOCK`) answers.
    fn their_agent_answers() -> bool {
        std::env::var_os("SSH_AUTH_SOCK")
            .filter(|socket| !socket.is_empty())
            .is_some_and(|socket| std::os::unix::net::UnixStream::connect(socket).is_ok())
    }

    pub(super) fn start() -> Option<SshSharing> {
        let name = format!("egit-{}", rustix::process::getuid().as_raw());
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        let parents = runtime
            .into_iter()
            .chain([PathBuf::from("/tmp"), std::env::temp_dir()]);
        let holding = if their_agent_answers() {
            Holding::Theirs
        } else {
            Holding::Own
        };
        for parent in parents {
            let private = parent.join(&name);
            let started = ensure_private(&private).and_then(|()| {
                let base = private.join("ssh");
                // Without an agent of egit's own (no `ssh-agent`), connections are still shared.
                start_in(&base, holding).or_else(|error| {
                    if holding == Holding::Own {
                        tracing::info!(%error, "keys can't be held: no agent of egit's own");
                        start_in(&base, Holding::Nowhere)
                    } else {
                        Err(error)
                    }
                })
            });
            match started {
                Ok(sharing) => return Some(sharing),
                Err(error) => {
                    tracing::debug!(%error, base = %private.display(), "can't share ssh connections from here");
                }
            }
        }
        None
    }

    pub(super) fn start_in(base: &Path, holding: Holding) -> io::Result<SshSharing> {
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
        let agent = written.and_then(|()| match holding {
            Holding::Own => Agent::start(dir.join("agent")).map(Some),
            Holding::Theirs | Holding::Nowhere => Ok(None),
        });
        match agent {
            Ok(agent) => Ok(SshSharing {
                dir,
                wrapper,
                holding,
                agent,
            }),
            Err(error) => {
                // Nothing in it is in use yet.
                let _ignored = fs::remove_dir_all(&dir);
                Err(error)
            }
        }
    }

    /// The wrapper git runs as its ssh: ssh itself (found on `PATH`, as git finds it), adding a
    /// key whose passphrase is typed to the agent (`SSH_AUTH_SOCK`), and sharing connections
    /// through sockets in `dir`. Keepalives every 15 seconds close a master whose network has
    /// gone (a laptop woken elsewhere) within a minute, rather than leaving the commands sent
    /// through it waiting.
    fn script(dir: &str) -> String {
        format!(
            "#!/bin/sh\n\
             # egit's: a key's passphrase held, and one connection to a host shared, while \
             egit runs.\n\
             exec ssh -o AddKeysToAgent=yes -o ControlMaster=auto -o ControlPersist={KEPT_IDLE} \
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
                "exec ssh -o AddKeysToAgent=yes -o ControlMaster=auto -o ControlPersist=1h \
                 -o ServerAliveInterval=15 \
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
            let refused = start_in(&deep, Holding::Nowhere).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);

            let sharing = start_in(&root.path().join("ssh"), Holding::Nowhere).unwrap();
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

    use super::{Holding, SshSharing};

    /// No agent of egit's own where connections can't be shared.
    #[derive(Debug)]
    pub(crate) enum Agent {}

    pub(super) const fn start() -> Option<SshSharing> {
        None
    }

    pub(super) fn start_in(_base: &Path, _holding: Holding) -> io::Result<SshSharing> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OpenSSH for Windows can't share connections",
        ))
    }

    pub(super) const fn close_masters(_dir: &Path) {}
}
