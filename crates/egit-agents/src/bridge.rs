//! The bridge run as a process: a conversation ([`Bridge`]), its lines read by a task of its
//! own and handed over as [`Event`]s, or a question answered once ([`run_once`]: the sessions
//! kept for a folder, one's messages).
//!
//! The bridge leads a group of its own (Claude Code's process in it), as git does. Dropped, a
//! conversation's standard input closes, which ends it; it is stopped if it lingers, so the
//! window never waits.
//!
//! On another computer ([`Launch::there`]) the bridge runs there, through that computer's git
//! ([`egit_git::Git::run_there`]): a session of egit's shared SSH connection, or its relay's.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use egit_git::sync::Mutex;
use egit_git::{Git, Input, Running, Stopping};
use serde_json::Value;

use crate::AgentError;
use crate::protocol::{Event, Request};

/// The most a one-shot answer may be (a long session's messages).
const ANSWER_LIMIT: usize = 256 << 20;
/// How long a conversation may take to end once its input closed, then once asked to stop.
const ENDING: Duration = Duration::from_secs(3);

/// How to run the bridge: Node, the script, and its environment; where.
#[derive(Clone, Debug)]
pub struct Launch {
    /// `node`.
    pub node: PathBuf,
    /// `bridge.mjs`, in its installation (its `node_modules` beside it).
    pub bridge: PathBuf,
    /// Variables set for it (an account's, a stand-in SDK's). On another computer never a
    /// secret: a command line there would show it (an account's go in the start request).
    pub env: Vec<(OsString, OsString)>,
    /// Variables it doesn't get (another account's credentials): here, out of egit's
    /// environment; there, out of the one it gets there.
    pub unset: Vec<OsString>,
    /// The other computer it runs on (its git), if it isn't this one.
    pub there: Option<Git>,
}

impl Launch {
    /// Starts the bridge with `args`, its input open when `input`.
    fn run(&self, args: &[&str], input: bool) -> Result<Running, AgentError> {
        let args: Vec<&OsStr> = std::iter::once(self.bridge.as_os_str())
            .chain(args.iter().map(OsStr::new))
            .collect();
        if let Some(git) = &self.there {
            return Ok(git.run_there(
                self.node.as_os_str(),
                &args,
                self.bridge.parent(),
                (&self.env, &self.unset),
                input,
            )?);
        }
        let mut command = Command::new(&self.node);
        command.args(&args);
        if let Some(folder) = self.bridge.parent() {
            command.current_dir(folder);
        }
        for key in &self.unset {
            command.env_remove(key);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        Ok(Running::here(command, input)?)
    }
}

/// A conversation's bridge, running.
pub struct Bridge {
    input: Arc<Mutex<Option<Input>>>,
    stopping: Stopping,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("open", &self.input.lock().is_some())
            .finish_non_exhaustive()
    }
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
        let mut running = launch.run(&[], true)?;
        let input = running.take_input();
        let stopping = running.stopping();
        let (sender, receiver) = mpsc::channel();
        egit_git::exec::spawn("agent", async move {
            let mut lines = Lines::default();
            let mut ended = false;
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let read = match running.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let mut gone = false;
                lines.feed(buffer.get(..read).unwrap_or_default(), |line| {
                    if gone || line.trim_ascii().is_empty() {
                        return;
                    }
                    let event =
                        serde_json::from_slice(line).unwrap_or_else(|error| Event::Failed {
                            op: None,
                            message: format!("the bridge said something unreadable: {error}"),
                        });
                    ended |= matches!(event, Event::Ended { .. });
                    gone = sender.send(event).is_err();
                    notify();
                });
                if gone {
                    break;
                }
            }
            if !ended {
                let tail = running.said().trim().to_owned();
                let error = if tail.is_empty() {
                    "The bridge stopped.".to_owned()
                } else {
                    format!("The bridge stopped: {tail}")
                };
                // The receiver may be gone; nothing to tell then.
                let _sent = sender.send(Event::Ended { error: Some(error) });
                notify();
            }
            running.stop(ENDING);
        })?;
        Ok((
            Self {
                input: Arc::new(Mutex::new(input)),
                stopping,
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
        let mut input = self.input.lock();
        let pipe = input.as_mut().ok_or(AgentError::Gone)?;
        let written = pipe.write(&line);
        drop(input);
        Ok(written?)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Its input closed ends the conversation; it is stopped if it lingers.
        self.input.lock().take();
        let stopping = self.stopping.clone();
        let lingering = egit_git::exec::spawn("agent ending", async move {
            egit_git::exec::sleep(ENDING).await;
            stopping.stop();
        });
        if let Err(error) = lingering {
            tracing::warn!(%error, "couldn't wait for an agent's bridge to end");
        }
    }
}

/// Lines split from chunks as they come.
#[derive(Default)]
struct Lines {
    pending: Vec<u8>,
}

impl Lines {
    fn feed(&mut self, chunk: &[u8], mut each: impl FnMut(&[u8])) {
        let mut rest = chunk;
        while let Some(at) = rest.iter().position(|byte| *byte == b'\n') {
            let (line, after) = rest.split_at(at);
            self.pending.extend_from_slice(line);
            each(&self.pending);
            self.pending.clear();
            rest = after.get(1..).unwrap_or_default();
        }
        self.pending.extend_from_slice(rest);
    }
}

/// Runs the bridge once with `args` and reads the one JSON value it prints.
///
/// # Errors
///
/// When Node can't be started, the bridge fails (its words said), or what it prints isn't JSON.
pub async fn run_once(launch: &Launch, args: &[&str]) -> Result<Value, AgentError> {
    let mut running = launch.run(args, false)?;
    let answer = running.read_to_end(ANSWER_LIMIT).await?;
    // It said what it had to: it ends, or is stopped.
    running.stop(ENDING);
    let said = running.said();
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
