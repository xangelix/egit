//! Where git runs: this computer, or another reached over SSH.
//!
//! On another computer each command is a session of one SSH connection, shared while egit runs
//! (OpenSSH's `ControlMaster`, in an [`SshSharing`](crate::SshSharing)'s folder), so logging in
//! is asked once. The session runs `/bin/sh` there with a script egit wrote: it says its
//! process id on standard error (cancelling asks its whole group there to stop, as here), goes
//! into the command's directory, marks the start of its output on standard output (whatever a
//! login script printed before is skipped), and `exec`s git with egit's environment. The
//! script reaches `sh` encoded in base64, so the login shell in between (bash, zsh, fish) passes
//! it on untouched, whatever its quoting rules.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;

use crate::relay::Relay;
use bstr::ByteSlice as _;

/// Where a repository's git runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Host {
    /// This computer.
    #[default]
    Local,
    /// Another computer, over SSH.
    Ssh(SshHost),
    /// The computer a relay runs on (`egit-relay`), which runs every program egit asks for.
    Relay(Relay),
}

impl Host {
    /// The other computer reached over SSH, if it is one.
    #[must_use]
    pub const fn ssh(&self) -> Option<&SshHost> {
        match self {
            Self::Ssh(host) => Some(host),
            Self::Local | Self::Relay(_) => None,
        }
    }

    /// The relay, if git runs through one.
    #[must_use]
    pub const fn relay(&self) -> Option<&Relay> {
        match self {
            Self::Relay(relay) => Some(relay),
            Self::Local | Self::Ssh(_) => None,
        }
    }

    /// Whether git runs on this computer.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

/// Another computer, reached over SSH.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SshHost {
    /// What ssh connects to: `host`, `user@host`, an alias from `~/.ssh/config`, or
    /// `ssh://user@host:port`.
    pub destination: String,
    /// More of ssh's options (a test's own key and known hosts), before egit's own: ssh takes
    /// the first value it is given for each.
    pub options: Vec<String>,
    /// The folder its connection is shared through (an `SshSharing`'s): without one, each
    /// command logs in again.
    pub control: Option<PathBuf>,
}

/// How long a shared connection outlives its last session.
const KEPT_IDLE: &str = "1h";

/// Marks the start of a command's output there: what a login script printed before it isn't
/// git's.
const MARKER: &[u8] = b"\x01EGIT-GO\x02";

/// How the script there says its process id, on standard error, before anything else of its own.
const PID_LINE: &[u8] = b"egit-pid ";

impl SshHost {
    /// The local `ssh` running `script` there (standard input, output, and error piped as the
    /// caller sets them).
    pub(crate) fn command(&self, script: &[u8]) -> Command {
        let mut command = Command::new("ssh");
        // No terminal (bytes pass untouched), no escape character, no forwarding asked here.
        // The host's own options first: ssh takes the first value it is given for each.
        command
            .args(["-T", "-e", "none"])
            .args(&self.options)
            .args(["-o", "ForwardX11=no"]);
        if let Some(control) = &self.control {
            let mut path = control.as_os_str().to_owned();
            path.push("/%C");
            let mut option = std::ffi::OsString::from("ControlPath=");
            option.push(path);
            // As git's ssh: a key's passphrase held (in the agent egit's sharing names), one
            // connection shared.
            command
                .args(["-o", "AddKeysToAgent=yes"])
                .args(["-o", "ControlMaster=auto", "-o"])
                .arg(option)
                .arg("-o")
                .arg(format!("ControlPersist={KEPT_IDLE}"))
                .args(["-o", "ServerAliveInterval=15"]);
        }
        command
            .arg("--")
            .arg(&self.destination)
            .arg(remote_line(script));
        command
    }

    /// Asks the process group a command there leads (`pid`) to stop, as cancelling stops git
    /// here (SIGTERM: git removes its lock files), waiting a few seconds at most.
    pub(crate) fn terminate(&self, pid: u32) {
        let script = format!("kill -TERM -- -{pid} 2>/dev/null");
        let mut command = self.command(script.as_bytes());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match crate::process::Detached::spawn(&mut command) {
            Ok(mut kill) => kill.stop(Duration::from_secs(5)),
            Err(error) => tracing::warn!(%error, pid, "could not ask git there to stop"),
        }
    }
}

/// The line ssh hands the login shell there: `/bin/sh` running `script`, decoded from base64,
/// which every shell passes on as it is inside single quotes.
fn remote_line(script: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(script);
    format!("exec /bin/sh -c 'eval \"$(printf %s {encoded} | base64 -d)\"'")
}

/// A command to run there, as [`script`] writes it.
pub(crate) struct Remote<'a> {
    /// The program (`git`).
    pub(crate) program: &'a OsStr,
    pub(crate) args: Vec<&'a OsStr>,
    /// Where it runs.
    pub(crate) dir: Option<&'a Path>,
    /// Variables it doesn't get, and those it does (set after, and so over, the removals).
    pub(crate) unset: Vec<&'a OsStr>,
    pub(crate) set: Vec<(&'a OsStr, &'a OsStr)>,
}

/// The script running `remote` there: its process id said, into its directory (exit 126 when
/// it can't), the start of its output marked, then the program `exec`ed with its environment.
pub(crate) fn script(remote: &Remote<'_>) -> Vec<u8> {
    let mut script = b"printf 'egit-pid %s\\n' \"$$\" >&2\n".to_vec();
    if let Some(dir) = remote.dir {
        script.extend_from_slice(b"cd ");
        quote_into(&mut script, dir.as_os_str());
        script.extend_from_slice(format!(" || exit {NO_DIRECTORY}\n").as_bytes());
    }
    script.extend_from_slice(b"printf '\\001EGIT-GO\\002'\nexec env");
    for name in &remote.unset {
        script.extend_from_slice(b" -u ");
        quote_into(&mut script, name);
    }
    for (name, value) in &remote.set {
        let mut pair = name.to_os_string();
        pair.push("=");
        pair.push(value);
        script.push(b' ');
        quote_into(&mut script, &pair);
    }
    script.push(b' ');
    quote_into(&mut script, remote.program);
    for arg in &remote.args {
        script.push(b' ');
        quote_into(&mut script, arg);
    }
    script.push(b'\n');
    script
}

/// `word` quoted for a POSIX shell: in single quotes, each single quote of its own written
/// `'\''`. Its bytes go as they are (on Windows, its UTF-8).
fn quote_into(script: &mut Vec<u8>, word: &OsStr) {
    let bytes = <[u8]>::from_os_str(word).map_or_else(
        || word.to_string_lossy().into_owned().into_bytes(),
        <[u8]>::to_vec,
    );
    script.push(b'\'');
    for byte in bytes {
        if byte == b'\'' {
            script.extend_from_slice(b"'\\''");
        } else {
            script.push(byte);
        }
    }
    script.push(b'\'');
}

/// What git printed to standard output: as it printed it here; there, from the script's marker
/// on (what a login script printed before it is skipped).
#[derive(Debug)]
pub(crate) struct GitStdout {
    pipe: ChildStdout,
    /// How much of the marker has been read, while it is still looked for.
    marker: Option<usize>,
}

impl GitStdout {
    /// Standard output of a command run on `host`.
    pub(crate) const fn new(pipe: ChildStdout, host: &Host) -> Self {
        Self {
            pipe,
            marker: match host {
                Host::Local | Host::Relay(_) => None,
                Host::Ssh(_) => Some(0),
            },
        }
    }

    /// Standard output of a program run where `host` is (see
    /// [`Git::command_there`](crate::Git::command_there)).
    #[must_use]
    pub(crate) const fn of(pipe: ChildStdout, host: &Host) -> Self {
        Self::new(pipe, host)
    }

    /// Whether the program started: here, always; there, once its script marked its output.
    pub(crate) const fn started(&self) -> bool {
        self.marker.is_none()
    }
}

/// How the script there ends when it can't go into the command's directory.
pub(crate) const NO_DIRECTORY: i32 = 126;

impl Read for GitStdout {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // A byte at a time up to the marker: what comes after it stays in the pipe.
        while let Some(matched) = self.marker {
            let mut byte = [0_u8; 1];
            if self.pipe.read(&mut byte)? == 0 {
                return Ok(0);
            }
            let [byte] = byte;
            let next = if MARKER.get(matched) == Some(&byte) {
                matched.saturating_add(1)
            } else {
                usize::from(MARKER.first() == Some(&byte))
            };
            self.marker = (next < MARKER.len()).then_some(next);
        }
        self.pipe.read(buffer)
    }
}

/// The process id a command there says first on standard error, once said.
pub(crate) type RemotePid = Arc<OnceLock<u32>>;

/// The most kept of what came before the process id (a login script's words, or ssh's when
/// logging in failed).
const BEFORE_LIMIT: usize = 16 << 10;

/// Reads a command's standard error from there: what comes before its process id (a login
/// script's, or ssh's own) is held back, the id kept in `pid`; what follows is the program's.
#[derive(Debug)]
pub(crate) struct PidReader {
    pid: RemotePid,
    /// What came before the id, in whole lines, and a line not yet ended.
    before: Vec<u8>,
    pending: Vec<u8>,
}

impl PidReader {
    /// A reader keeping the id in `pid`.
    #[must_use]
    pub(crate) const fn new(pid: RemotePid) -> Self {
        Self {
            pid,
            before: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Takes in `chunk`; returns the program's part of it (none until the id came).
    pub(crate) fn feed<'c>(&mut self, chunk: &'c [u8]) -> Option<std::borrow::Cow<'c, [u8]>> {
        if self.pid.get().is_some() {
            return Some(std::borrow::Cow::Borrowed(chunk));
        }
        self.pending.extend_from_slice(chunk);
        while let Some(end) = self.pending.find_byte(b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let said = line.strip_prefix(PID_LINE).and_then(|rest| {
                std::str::from_utf8(rest.trim_ascii())
                    .ok()?
                    .parse::<u32>()
                    .ok()
            });
            if let Some(pid) = said {
                // The first a line says: a login script can't have said one.
                let _ = self.pid.set(pid);
                self.before = Vec::new();
                let rest = std::mem::take(&mut self.pending);
                return Some(std::borrow::Cow::Owned(rest));
            }
            if self.before.len() < BEFORE_LIMIT {
                self.before.extend_from_slice(&line);
            }
        }
        None
    }

    /// What was held back when standard error ended without the id (logging in failed, or the
    /// script couldn't run): all of it, to say why.
    #[must_use]
    pub(crate) fn finish(mut self) -> Vec<u8> {
        if self.pid.get().is_some() {
            Vec::new()
        } else {
            self.before.append(&mut self.pending);
            self.before
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn a_script_quotes_every_word_for_sh() {
        let program = OsString::from("git");
        let args = [
            OsString::from("log"),
            OsString::from("--grep=it's \\n $HOME"),
        ];
        let unset = OsString::from("GIT_DIR");
        let (name, value) = (OsString::from("LC_MESSAGES"), OsString::from("C"));
        let dir = PathBuf::from("/home/ada/my repo");
        let script = script(&Remote {
            program: &program,
            args: args.iter().map(OsString::as_os_str).collect(),
            dir: Some(&dir),
            unset: vec![&unset],
            set: vec![(&name, &value)],
        });
        assert_eq!(
            script.to_str().unwrap(),
            "printf 'egit-pid %s\\n' \"$$\" >&2\n\
             cd '/home/ada/my repo' || exit 126\n\
             printf '\\001EGIT-GO\\002'\n\
             exec env -u 'GIT_DIR' 'LC_MESSAGES=C' 'git' 'log' '--grep=it'\\''s \\n $HOME'\n"
        );
        // Only base64's letters inside the login shell's single quotes.
        let line = remote_line(&script);
        let quoted = line.split('\'').nth(1).unwrap();
        assert!(quoted.starts_with("eval \"$(printf %s "));
        let encoded = quoted
            .trim_start_matches("eval \"$(printf %s ")
            .trim_end_matches(" | base64 -d)\"");
        assert!(
            encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
        );
    }

    #[test]
    fn the_process_id_comes_first_and_what_came_before_it_goes() {
        let pid = RemotePid::default();
        let mut reader = PidReader::new(Arc::clone(&pid));
        assert_eq!(reader.feed(b"Welcome to the box\negit-p"), None);
        assert_eq!(
            reader.feed(b"id 4242\nerror: one").as_deref(),
            Some(&b"error: one"[..])
        );
        assert_eq!(pid.get(), Some(&4242));
        assert_eq!(reader.feed(b"\nmore").as_deref(), Some(&b"\nmore"[..]));
        assert_eq!(reader.finish(), Vec::<u8>::new());

        let failed = PidReader::new(RemotePid::default());
        let mut failed = failed;
        assert_eq!(failed.feed(b"Permission denied (publickey).\n"), None);
        assert_eq!(
            failed.finish(),
            b"Permission denied (publickey).\n".to_vec()
        );
    }
}
