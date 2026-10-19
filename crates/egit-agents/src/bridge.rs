//! The bridge run as a process: a conversation ([`Bridge`]), its lines read on a thread of its
//! own and handed over as [`Event`]s, or a question answered once ([`run_once`]: the sessions
//! kept for a folder, one's messages).
//!
//! The bridge leads a group of its own (Claude Code's process in it), as git does. Dropped, a
//! conversation's standard input closes, which ends it; its process is waited for on a thread
//! of its own and stopped if it lingers, so the window never waits.

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use egit_git::Detached;
use parking_lot::Mutex;
use serde_json::Value;

use crate::AgentError;
use crate::protocol::{Event, Request};

/// The most a one-shot answer may be (a long session's messages).
const ANSWER_LIMIT: u64 = 256 << 20;
/// How long a conversation may take to end once its input closed, then once asked to stop.
const ENDING: Duration = Duration::from_secs(3);

/// How to run the bridge: Node, the script, and its environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// `node`.
    pub node: PathBuf,
    /// `bridge.mjs`, in its installation (its `node_modules` beside it).
    pub bridge: PathBuf,
    /// Variables set for it (an account's, a stand-in SDK's).
    pub env: Vec<(OsString, OsString)>,
    /// Variables it doesn't get (another account's).
    pub unset: Vec<OsString>,
}

impl Launch {
    /// The command running the bridge with `args`.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.node);
        command.arg(&self.bridge).args(args);
        if let Some(folder) = self.bridge.parent() {
            command.current_dir(folder);
        }
        for key in &self.unset {
            command.env_remove(key);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}

/// A conversation's bridge, running.
#[derive(Debug)]
pub struct Bridge {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    process: Option<Detached>,
}

impl Bridge {
    /// Starts the bridge for a conversation. Its events arrive on the receiver, `notify` called
    /// after each (to wake the window); its last is always [`Event::Ended`].
    ///
    /// # Errors
    ///
    /// When Node can't be started.
    pub fn start(
        launch: &Launch,
        notify: impl Fn() + Send + 'static,
    ) -> Result<(Self, Receiver<Event>), AgentError> {
        let mut command = launch.command(&[]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = Detached::spawn(&mut command)?;
        let child = process.child_mut();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let said = Arc::new(Mutex::new(String::new()));
        if let Some(mut stderr) = stderr {
            let said = Arc::clone(&said);
            std::thread::Builder::new()
                .name("agent stderr".to_owned())
                .spawn(move || {
                    let mut chunk = [0_u8; 4096];
                    while let Ok(read @ 1..) = stderr.read(&mut chunk) {
                        let mut tail = said.lock();
                        tail.push_str(&String::from_utf8_lossy(
                            chunk.get(..read).unwrap_or_default(),
                        ));
                        let excess = tail.len().saturating_sub(8192);
                        if excess > 0 {
                            let cut = (excess..tail.len())
                                .find(|at| tail.is_char_boundary(*at))
                                .unwrap_or(tail.len());
                            tail.drain(..cut);
                        }
                    }
                })?;
        }
        let (sender, receiver) = mpsc::channel();
        if let Some(stdout) = stdout {
            std::thread::Builder::new()
                .name("agent".to_owned())
                .spawn(move || {
                    let mut ended = false;
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        if line.trim().is_empty() {
                            continue;
                        }
                        let event =
                            serde_json::from_str(&line).unwrap_or_else(|error| Event::Failed {
                                op: None,
                                message: format!("the bridge said something unreadable: {error}"),
                            });
                        ended |= matches!(event, Event::Ended { .. });
                        if sender.send(event).is_err() {
                            return;
                        }
                        notify();
                    }
                    if !ended {
                        let tail = said.lock().trim().to_owned();
                        let error = if tail.is_empty() {
                            "The bridge stopped.".to_owned()
                        } else {
                            format!("The bridge stopped: {tail}")
                        };
                        // The receiver may be gone; nothing to tell then.
                        let _sent = sender.send(Event::Ended { error: Some(error) });
                        notify();
                    }
                })?;
        }
        Ok((
            Self {
                stdin: Arc::new(Mutex::new(stdin)),
                process: Some(process),
            },
            receiver,
        ))
    }

    /// Writes `request` to the bridge.
    ///
    /// # Errors
    ///
    /// When the bridge has gone.
    pub fn send(&self, request: &Request) -> Result<(), AgentError> {
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock();
        let pipe = stdin.as_mut().ok_or(AgentError::Gone)?;
        let written = pipe.write_all(&line).and_then(|()| pipe.flush());
        drop(stdin);
        Ok(written?)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Its input closed ends the conversation; the process is waited for elsewhere.
        self.stdin.lock().take();
        if let Some(mut process) = self.process.take() {
            let waited = std::thread::Builder::new()
                .name("agent ending".to_owned())
                .spawn(move || process.stop(ENDING));
            if let Err(error) = waited {
                tracing::warn!(%error, "couldn't wait for an agent's bridge to end");
            }
        }
    }
}

/// Runs the bridge once with `args` and reads the one JSON value it prints. Blocks: run it in a
/// job.
///
/// # Errors
///
/// When Node can't be started, the bridge fails (its words said), or what it prints isn't JSON.
pub fn run_once(launch: &Launch, args: &[&str]) -> Result<Value, AgentError> {
    let mut command = launch.command(args);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = Detached::spawn(&mut command)?;
    let child = process.child_mut();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let complaint = std::thread::spawn(move || {
        let mut said = String::new();
        if let Some(stderr) = stderr {
            let _read = stderr.take(64 << 10).read_to_string(&mut said);
        }
        said
    });
    let mut answer = Vec::new();
    if let Some(stdout) = stdout {
        stdout.take(ANSWER_LIMIT).read_to_end(&mut answer)?;
    }
    process.stop(ENDING);
    let said = complaint.join().unwrap_or_default();
    let value: Value = serde_json::from_slice(&answer).map_err(|error| {
        let said = said.trim();
        if said.is_empty() {
            AgentError::Unreadable(error)
        } else {
            AgentError::Bridge(said.to_owned())
        }
    })?;
    if let Some(message) = value
        .get("event")
        .filter(|event| *event == "failed")
        .and_then(|_| value.get("message"))
        .and_then(Value::as_str)
    {
        return Err(AgentError::Bridge(message.to_owned()));
    }
    Ok(value)
}
