//! A git process kept running for one of git's batch protocols (`cat-file --batch-command`):
//! requests go to its standard input, answers come from its standard output.

use std::io::{self, BufReader};
use std::process::{Child, ChildStdin, ChildStdout, ExitStatus};
use std::thread::JoinHandle;

use rustix::process::Pid;

use crate::error::GitError;
use crate::output::Stderr;
use crate::process;

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
    group: Pid,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    drain: Option<JoinHandle<Stderr>>,
    reaped: bool,
}

impl GitProcess {
    pub(crate) fn new(
        subcommand: &'static str,
        child: Child,
        stdin: ChildStdin,
        stdout: ChildStdout,
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

    /// Where requests go.
    ///
    /// # Errors
    ///
    /// When the input was closed already.
    pub fn input(&mut self) -> io::Result<&mut ChildStdin> {
        self.stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "git's input is closed"))
    }

    /// Where answers come from.
    pub const fn output(&mut self) -> &mut BufReader<ChildStdout> {
        &mut self.stdout
    }

    /// Closes git's input, waits for it to finish what it was asked, and reaps it.
    ///
    /// # Errors
    ///
    /// [`GitError::Failed`] when git exits unsuccessfully, [`GitError::Io`] when waiting fails.
    pub fn finish(mut self) -> Result<(), GitError> {
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
