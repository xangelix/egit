//! A git process kept running for one of git's batch protocols (`cat-file --batch-command`):
//! requests go to its standard input, answers come from its standard output.

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdin, ExitStatus};
use std::sync::Arc;
use std::thread::JoinHandle;

use parking_lot::Mutex;

use crate::error::GitError;
use crate::host::GitStdout;
use crate::output::{Exit, Stderr, TailBuffer};
use crate::process::{self, Group};
use crate::relay;

/// How much of git's answers is read at a time.
const READ_BUFFER: usize = 64 * 1024;

/// A running git, started with [`GitCommand::spawn`](crate::GitCommand::spawn).
///
/// It runs in a session of its own like every git egit starts. Dropping it closes its input and
/// stops it (SIGTERM to its process group), then reaps it.
#[derive(Debug)]
pub struct GitProcess {
    subcommand: &'static str,
    running: Running,
}

#[derive(Debug)]
enum Running {
    /// Here (or the ssh running it there).
    Here {
        child: Child,
        group: Group,
        stdin: Option<ChildStdin>,
        stdout: BufReader<GitStdout>,
        drain: Option<JoinHandle<Stderr>>,
        reaped: bool,
    },
    /// Through a relay: its answers read into a buffer of our own, `start` of it taken.
    Relayed {
        process: relay::Process,
        buffer: Vec<u8>,
        start: usize,
        tail: Arc<Mutex<TailBuffer>>,
    },
}

impl GitProcess {
    pub(crate) fn new(
        subcommand: &'static str,
        child: Child,
        stdin: ChildStdin,
        stdout: GitStdout,
        drain: JoinHandle<Stderr>,
    ) -> Self {
        let group = process::group_of(&child);
        Self {
            subcommand,
            running: Running::Here {
                child,
                group,
                stdin: Some(stdin),
                stdout: BufReader::with_capacity(READ_BUFFER, stdout),
                drain: Some(drain),
                reaped: false,
            },
        }
    }

    /// Git running through a relay, its standard error's tail kept in `tail`.
    pub(crate) const fn relayed(
        subcommand: &'static str,
        process: relay::Process,
        tail: Arc<Mutex<TailBuffer>>,
    ) -> Self {
        Self {
            subcommand,
            running: Running::Relayed {
                process,
                buffer: Vec::new(),
                start: 0,
                tail,
            },
        }
    }

    /// The git command running (`cat-file`, …).
    #[must_use]
    pub const fn subcommand(&self) -> &'static str {
        self.subcommand
    }

    /// Sends `request` to git, all of it, at once.
    ///
    /// # Errors
    ///
    /// When the input was closed already, or git stopped reading.
    pub async fn send(&mut self, request: &[u8]) -> io::Result<()> {
        match &mut self.running {
            Running::Here { stdin, .. } => {
                let input = stdin.as_mut().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "git's input is closed")
                })?;
                input.write_all(request)?;
                input.flush()
            }
            Running::Relayed { process, .. } => process.write(request).await,
        }
    }

    /// Reads git's answer into `buffer` up to `byte` and through it, saying how much was read
    /// (0 once git stopped answering).
    ///
    /// # Errors
    ///
    /// When its output can't be read.
    pub async fn read_until(&mut self, byte: u8, buffer: &mut Vec<u8>) -> io::Result<usize> {
        match &mut self.running {
            Running::Here { stdout, .. } => stdout.read_until(byte, buffer),
            Running::Relayed {
                process,
                buffer: read,
                start,
                ..
            } => {
                let mut taken = 0_usize;
                loop {
                    let waiting = read.get(*start..).unwrap_or_default();
                    if let Some(at) = waiting.iter().position(|found| *found == byte) {
                        let through = at.saturating_add(1);
                        buffer.extend_from_slice(waiting.get(..through).unwrap_or_default());
                        *start = start.saturating_add(through);
                        return Ok(taken.saturating_add(through));
                    }
                    buffer.extend_from_slice(waiting);
                    taken = taken.saturating_add(waiting.len());
                    if !refill(process, read, start).await? {
                        return Ok(taken);
                    }
                }
            }
        }
    }

    /// Fills `buffer` with what git answers next.
    ///
    /// # Errors
    ///
    /// When its output can't be read, or ends first.
    pub async fn read_exact(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        match &mut self.running {
            Running::Here { stdout, .. } => stdout.read_exact(buffer),
            Running::Relayed {
                process,
                buffer: read,
                start,
                ..
            } => {
                let mut filled = 0_usize;
                while filled < buffer.len() {
                    let waiting = read.get(*start..).unwrap_or_default();
                    if waiting.is_empty() {
                        // A large object goes straight into its buffer, not through ours.
                        let rest = buffer.get_mut(filled..).unwrap_or_default();
                        if rest.len() >= READ_BUFFER {
                            let got = process.read(rest).await?;
                            if got == 0 {
                                return Err(ended_early());
                            }
                            filled = filled.saturating_add(got);
                        } else if !refill(process, read, start).await? {
                            return Err(ended_early());
                        }
                        continue;
                    }
                    let count = waiting.len().min(buffer.len().saturating_sub(filled));
                    if let (Some(into), Some(from)) = (
                        buffer.get_mut(filled..filled.saturating_add(count)),
                        waiting.get(..count),
                    ) {
                        into.copy_from_slice(from);
                    }
                    filled = filled.saturating_add(count);
                    *start = start.saturating_add(count);
                }
                Ok(())
            }
        }
    }

    /// Closes git's input, waits for it to finish what it was asked, and reaps it.
    ///
    /// # Errors
    ///
    /// [`GitError::Failed`] when git exits unsuccessfully, [`GitError::Io`] when waiting fails.
    pub async fn finish(mut self) -> Result<(), GitError> {
        let subcommand = self.subcommand;
        let (status, stderr) = match &mut self.running {
            Running::Here { stdin, .. } => {
                drop(stdin.take());
                let (status, stderr) = self.reap();
                (status.map(Exit::from), stderr)
            }
            Running::Relayed { process, tail, .. } => {
                process.close_input();
                let ended = process.wait().await.map_err(io::Error::from);
                (ended, std::mem::take(&mut *tail.lock()).finish())
            }
        };
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(GitError::failed(subcommand, status, stderr)),
            Err(source) => Err(GitError::Io { subcommand, source }),
        }
    }

    /// Waits for git here to exit and reaps it, returning how it exited and what it said.
    fn reap(&mut self) -> (io::Result<ExitStatus>, Stderr) {
        let Running::Here {
            child,
            group,
            drain,
            reaped,
            ..
        } = &mut self.running
        else {
            return (
                Err(io::Error::other("git runs elsewhere")),
                Stderr::default(),
            );
        };
        let waited = process::wait_until_exited(*group);
        let status = waited.and_then(|()| child.wait());
        *reaped = true;
        let stderr = drain
            .take()
            .and_then(|drain| drain.join().ok())
            .unwrap_or_default();
        (status, stderr)
    }
}

/// Reads more of a relayed process's answers into `read` (what was taken of it dropped); whether
/// any came.
async fn refill(
    process: &mut relay::Process,
    read: &mut Vec<u8>,
    start: &mut usize,
) -> io::Result<bool> {
    read.clear();
    *start = 0;
    read.resize(READ_BUFFER, 0);
    let got = process.read(read).await?;
    read.truncate(got);
    Ok(got > 0)
}

fn ended_early() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "git stopped answering")
}

impl Drop for GitProcess {
    fn drop(&mut self) {
        match &mut self.running {
            Running::Here {
                stdin,
                group,
                reaped,
                ..
            } => {
                if *reaped {
                    return;
                }
                drop(stdin.take());
                process::terminate_group(*group);
                if let (Err(error), _) = self.reap() {
                    tracing::debug!(%error, subcommand = self.subcommand, "reaping a stopped git failed");
                }
            }
            // Dropped, it is stopped there.
            Running::Relayed { .. } => {}
        }
    }
}
