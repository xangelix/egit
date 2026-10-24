//! A git process kept running for one of git's batch protocols (`cat-file --batch-command`):
//! requests go to its standard input, answers come from its standard output.

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdin, ExitStatus};
use std::thread::JoinHandle;

use crate::error::GitError;
use crate::host::GitStdout;
use crate::output::Stderr;
use crate::process::{self, Group};

/// How much of git's answers is read at a time.
const READ_BUFFER: usize = 64 * 1024;

/// A running git, started with [`GitCommand::spawn`](crate::GitCommand::spawn).
///
/// It runs in a session of its own like every git egit starts. Dropping it closes its input and
/// stops it (SIGTERM to its process group), then reaps it.
#[derive(Debug)]
pub struct GitProcess {
    subcommand: &'static str,
    child: Child,
    group: Group,
    stdin: Option<ChildStdin>,
    stdout: BufReader<GitStdout>,
    drain: Option<JoinHandle<Stderr>>,
    reaped: bool,
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
            child,
            group,
            stdin: Some(stdin),
            stdout: BufReader::with_capacity(READ_BUFFER, stdout),
            drain: Some(drain),
            reaped: false,
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
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "here it blocks; through a relay it will await"
    )]
    pub async fn send(&mut self, request: &[u8]) -> io::Result<()> {
        let input = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "git's input is closed"))?;
        input.write_all(request)?;
        input.flush()
    }

    /// Reads git's answer into `buffer` up to `byte` and through it, saying how much was read
    /// (0 once git stopped answering).
    ///
    /// # Errors
    ///
    /// When its output can't be read.
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "here it blocks; through a relay it will await"
    )]
    pub async fn read_until(&mut self, byte: u8, buffer: &mut Vec<u8>) -> io::Result<usize> {
        self.stdout.read_until(byte, buffer)
    }

    /// Fills `buffer` with what git answers next.
    ///
    /// # Errors
    ///
    /// When its output can't be read, or ends first.
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "here it blocks; through a relay it will await"
    )]
    pub async fn read_exact(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        self.stdout.read_exact(buffer)
    }

    /// Closes git's input, waits for it to finish what it was asked, and reaps it.
    ///
    /// # Errors
    ///
    /// [`GitError::Failed`] when git exits unsuccessfully, [`GitError::Io`] when waiting fails.
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "here it blocks; through a relay it will await"
    )]
    pub async fn finish(mut self) -> Result<(), GitError> {
        drop(self.stdin.take());
        let (status, stderr) = self.reap();
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(GitError::failed(self.subcommand, status, stderr)),
            Err(source) => Err(GitError::Io {
                subcommand: self.subcommand,
                source,
            }),
        }
    }

    /// Waits for git to exit and reaps it, returning how it exited and what it said.
    fn reap(&mut self) -> (io::Result<ExitStatus>, Stderr) {
        let waited = process::wait_until_exited(self.group);
        let status = waited.and_then(|()| self.child.wait());
        self.reaped = true;
        let stderr = self
            .drain
            .take()
            .and_then(|drain| drain.join().ok())
            .unwrap_or_default();
        (status, stderr)
    }
}

impl Drop for GitProcess {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        drop(self.stdin.take());
        process::terminate_group(self.group);
        if let (Err(error), _) = self.reap() {
            tracing::debug!(%error, subcommand = self.subcommand, "reaping a stopped git failed");
        }
    }
}
