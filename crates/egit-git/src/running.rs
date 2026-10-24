//! A program other than git run where a git runs (an agent's bridge, npm, `sh`, ssh's own
//! `-G`): here, on another computer through ssh, or by a relay; leading a group of its own, as
//! git does.
//!
//! Its input is a writer of its own ([`Input`], for another thread to write into), its output
//! read as it comes, its standard error's end kept ([`Running::said`]), and how it ended.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _, Write as _};
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

use crate::git::Git;
use crate::host::{GitStdout, Host, PidReader, RemotePid};
use crate::output::{Exit, TailBuffer};
use crate::process::{Detached, Stopper};
use crate::relay::{self, Relay};

/// A program running where its git runs.
#[derive(Debug)]
pub struct Running {
    input: Option<Input>,
    output: Output,
    said: Arc<Mutex<TailBuffer>>,
}

/// Where a [`Running`]'s standard input goes: written from any thread, closed when dropped.
#[derive(Debug)]
pub struct Input(InputWay);

#[derive(Debug)]
enum InputWay {
    Here(ChildStdin),
    Relayed { relay: Relay, id: u32 },
}

#[derive(Debug)]
enum Output {
    Here {
        process: Detached,
        stdout: Option<GitStdout>,
        drain: Option<JoinHandle<()>>,
    },
    Relayed(relay::Process),
}

/// Stops a [`Running`] from any thread (its group asked to, as cancelling git does).
#[derive(Clone, Debug)]
pub struct Stopping(StopWay);

#[derive(Clone, Debug)]
enum StopWay {
    Here(Stopper),
    Relayed { relay: Relay, id: u32 },
}

impl Git {
    /// Starts `program` with `args` where this git runs, in `dir`, with git's environment there
    /// and `env` over it, without `unset`; its standard input open when `input`.
    ///
    /// # Errors
    ///
    /// When it can't be started.
    pub fn run_there(
        &self,
        program: &OsStr,
        args: &[&OsStr],
        dir: Option<&Path>,
        (env, unset): (&[(OsString, OsString)], &[OsString]),
        input: bool,
    ) -> io::Result<Running> {
        match self.host() {
            Host::Relay(relay) => {
                let env: Vec<(&OsStr, &OsStr)> = env
                    .iter()
                    .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
                    .collect();
                let mut there = relay::program_there(self, program, args, dir, &env);
                there.unset.extend(
                    unset
                        .iter()
                        .map(|key| <Vec<u8> as bstr::ByteVec>::from_os_str_lossy(key).into_owned()),
                );
                there.stdin = input;
                let said = Arc::new(Mutex::new(TailBuffer::default()));
                let sink = {
                    let said = Arc::clone(&said);
                    Box::new(move |chunk: &[u8]| said.lock().push(chunk))
                };
                let process = relay.start(there, sink);
                let input = input.then(|| {
                    Input(InputWay::Relayed {
                        relay: relay.clone(),
                        id: process.id(),
                    })
                });
                Ok(Running {
                    input,
                    output: Output::Relayed(process),
                    said,
                })
            }
            Host::Ssh(_) => {
                // Through `env -u`, those it doesn't get (whatever the login there exports).
                let mut words = Vec::new();
                for key in unset {
                    words.extend([OsStr::new("-u"), key.as_os_str()]);
                }
                words.push(program);
                words.extend_from_slice(args);
                let command = self
                    .command_there(OsStr::new("env"), &words, dir, env)
                    .ok_or_else(|| io::Error::other("not on another computer"))?;
                Running::spawn(command, self.host(), input)
            }
            Host::Local => {
                let mut command = Command::new(program);
                command.args(args);
                if let Some(dir) = dir {
                    command.current_dir(dir);
                }
                self.environment().apply(&mut command);
                for key in unset {
                    command.env_remove(key);
                }
                command.envs(env.iter().map(|(key, value)| (key, value)));
                Running::spawn(command, self.host(), input)
            }
        }
    }
}

impl Running {
    /// Runs `command` here as it stands (its environment its own): a program of egit's, not
    /// where any git runs.
    ///
    /// # Errors
    ///
    /// When it can't be started.
    pub fn here(command: Command, input: bool) -> io::Result<Self> {
        Self::spawn(command, &Host::Local, input)
    }

    /// Spawns `command` (here, or the ssh running it on `host`).
    fn spawn(mut command: Command, host: &Host, input: bool) -> io::Result<Self> {
        command
            .stdin(if input { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = Detached::spawn(&mut command)?;
        let child = process.child_mut();
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .map(|stdout| GitStdout::of(stdout, host));
        let stderr = child.stderr.take();
        let said = Arc::new(Mutex::new(TailBuffer::default()));
        let drain = match stderr {
            Some(mut stderr) => {
                let said = Arc::clone(&said);
                // There, its script's process id comes first: not the program's words.
                let mut pid = (!host.is_local()).then(|| PidReader::new(RemotePid::default()));
                Some(
                    std::thread::Builder::new()
                        .name("a program's errors".to_owned())
                        .spawn(move || {
                            let mut chunk = [0_u8; 4096];
                            while let Ok(read @ 1..) = stderr.read(&mut chunk) {
                                let chunk = chunk.get(..read).unwrap_or_default();
                                match &mut pid {
                                    Some(pid) => {
                                        if let Some(words) = pid.feed(chunk) {
                                            said.lock().push(&words);
                                        }
                                    }
                                    None => said.lock().push(chunk),
                                }
                            }
                            if let Some(pid) = pid {
                                said.lock().push(&pid.finish());
                            }
                        })?,
                )
            }
            None => None,
        };
        Ok(Self {
            input: stdin.map(|stdin| Input(InputWay::Here(stdin))),
            output: Output::Here {
                process,
                stdout,
                drain,
            },
            said,
        })
    }

    /// Its standard input, for writing apart (dropped, it closes).
    pub const fn take_input(&mut self) -> Option<Input> {
        self.input.take()
    }

    /// What stops it from elsewhere.
    #[must_use]
    pub fn stopping(&self) -> Stopping {
        match &self.output {
            Output::Here { process, .. } => Stopping(StopWay::Here(process.stopper())),
            Output::Relayed(process) => Stopping(StopWay::Relayed {
                relay: process.relay().clone(),
                id: process.id(),
            }),
        }
    }

    /// The end of what it said on standard error so far (8 KiB at most), as text.
    #[must_use]
    pub fn said(&self) -> String {
        let tail = self.said.lock();
        let bytes = tail.bytes();
        let start = bytes.len().saturating_sub(8192);
        let said = String::from_utf8_lossy(bytes.get(start..).unwrap_or_default()).into_owned();
        drop(tail);
        said
    }

    /// Reads what it prints next: how much, 0 at its end.
    ///
    /// # Errors
    ///
    /// When its output can't be read.
    pub async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.output {
            Output::Here { stdout, .. } => match stdout {
                Some(stdout) => loop {
                    match stdout.read(buffer) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        read => return read,
                    }
                },
                None => Ok(0),
            },
            Output::Relayed(process) => process.read(buffer).await,
        }
    }

    /// Reads everything it prints, `limit` bytes at most.
    ///
    /// # Errors
    ///
    /// When its output can't be read, or is longer.
    pub async fn read_to_end(&mut self, limit: usize) -> io::Result<Vec<u8>> {
        let mut printed = Vec::new();
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let read = self.read(&mut buffer).await?;
            if read == 0 {
                return Ok(printed);
            }
            printed.extend_from_slice(buffer.get(..read).unwrap_or_default());
            if printed.len() > limit {
                return Err(io::Error::other("it printed more than it may"));
            }
        }
    }

    /// Waits for it to end (its input closed first), and says how.
    ///
    /// # Errors
    ///
    /// When it can't be waited on, couldn't start, or its relay went away.
    pub async fn wait(&mut self) -> io::Result<Exit> {
        self.input = None;
        match &mut self.output {
            Output::Here { process, drain, .. } => {
                let status = process.wait()?;
                if let Some(drain) = drain.take() {
                    let _drained = drain.join();
                }
                Ok(Exit::from(status))
            }
            Output::Relayed(process) => process.wait().await.map_err(io::Error::from),
        }
    }

    /// Waits for it to end as [`Self::wait`] does, stopping it should `cancel` be cancelled
    /// meanwhile: how it ended, or `None` when it was cancelled.
    ///
    /// # Errors
    ///
    /// As [`Self::wait`].
    pub async fn wait_unless(&mut self, cancel: &crate::CancelToken) -> io::Result<Option<Exit>> {
        let registration = cancel.register_program(self.stopping());
        let ended = self.wait().await;
        drop(registration);
        if cancel.is_cancelled() {
            return Ok(None);
        }
        ended.map(Some)
    }

    /// Stops it (its input closed): waits up to `patience` for it to end by itself, then asks its
    /// group to stop.
    pub fn stop(&mut self, patience: Duration) {
        self.input = None;
        match &mut self.output {
            Output::Here { process, .. } => process.stop(patience),
            Output::Relayed(process) => process.terminate(),
        }
    }
}

impl Input {
    /// Writes `data`, all of it.
    ///
    /// # Errors
    ///
    /// When the program stopped reading.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        match &mut self.0 {
            InputWay::Here(stdin) => stdin.write_all(data).and_then(|()| stdin.flush()),
            InputWay::Relayed { relay, id } => {
                relay.input(*id, data);
                Ok(())
            }
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        if let InputWay::Relayed { relay, id } = &self.0 {
            relay.close_input(*id);
        }
    }
}

impl Stopping {
    /// Asks it to stop, and every process it started.
    pub fn stop(&self) {
        match &self.0 {
            StopWay::Here(stopper) => stopper.terminate(),
            StopWay::Relayed { relay, id } => relay.terminate(*id),
        }
    }
}
