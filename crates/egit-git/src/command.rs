//! Building and running one git command.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read as _, Write as _};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use zeroize::Zeroizing;

use crate::batch::GitProcess;
use crate::cancel::{CancelToken, Registration};
use crate::error::GitError;
use crate::git::Git;
use crate::output::{Completed, Stderr, TailBuffer};
use crate::process::{self, Group};
use crate::progress::{Progress, ProgressReader};
use crate::transcript::{Stream, Transcript};

/// Whether a command only reads the repository or may change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reads only. Runs with `--no-optional-locks`, so it never takes a lock the user's own git
    /// may want (`git status` would otherwise refresh the index under `index.lock`).
    Read,
    /// May write. Retried for a while when another git process holds a lock it needs.
    Write,
}

/// `-c` settings every command runs with: plain output, and paths printed as they are.
const DEFAULT_CONFIG: [&str; 2] = ["color.ui=never", "core.quotePath=false"];

/// How long a writing command waits before each new attempt while another git holds a lock it
/// needs: about 1.6 s in all, which covers another command finishing without stalling on a lock
/// that someone left behind.
const LOCK_RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
];

/// Helper threads only move bytes between a pipe and a buffer.
const HELPER_STACK: usize = 64 * 1024;

/// Streamed output is read this many bytes at a time, into one reused buffer.
const STREAM_CHUNK: usize = 64 * 1024;

/// The most [`GitCommand::secret_output`] collects: credentials are a few lines.
const SECRET_LIMIT: usize = 64 * 1024;

/// What a command reads on standard input.
#[derive(Clone)]
enum Input {
    Plain(Arc<[u8]>),
    /// A secret: wiped from memory when the last copy goes, and never shown in debug output.
    Secret(Arc<Zeroizing<Vec<u8>>>),
}

impl Input {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Plain(bytes) => bytes,
            Self::Secret(bytes) => bytes.as_slice(),
        }
    }
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(bytes) => write!(f, "{} bytes", bytes.len()),
            Self::Secret(_) => f.write_str("<secret>"),
        }
    }
}

/// What a command that prints a secret printed (`git credential fill`): its standard output is
/// wiped from memory when dropped, and never shown in debug output.
pub struct SecretOutput {
    /// How git exited.
    pub status: ExitStatus,
    /// Everything git printed to standard output.
    pub stdout: Zeroizing<Vec<u8>>,
    /// The end of what git printed to standard error.
    pub stderr: Stderr,
}

impl fmt::Debug for SecretOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretOutput")
            .field("status", &self.status)
            .field("stdout", &"<secret>")
            .field("stderr", &self.stderr)
            .finish()
    }
}

/// One git command, ready to run.
///
/// Built from [`Git::read`] or [`Git::write`], then run with [`Self::output`] (collect what it
/// prints) or [`Self::stream`] (hand it over in chunks).
#[must_use = "a command does nothing until it is run"]
#[derive(Clone, Debug)]
pub struct GitCommand {
    git: Git,
    access: Access,
    subcommand: &'static str,
    config: Vec<OsString>,
    args: Vec<OsString>,
    dir: Option<PathBuf>,
    stdin: Option<Input>,
    env: Vec<(OsString, OsString)>,
    cancel: Option<CancelToken>,
    stdout_limit: usize,
    /// Where git's `--progress` lines are reported, if anywhere.
    progress: Option<Progress>,
    /// Whether what it reads or prints is a secret (kept out of any transcript).
    secret: bool,
}

/// A streamed command that finished, or was stopped by its consumer.
#[derive(Debug)]
pub struct Streamed {
    /// How git exited.
    pub status: ExitStatus,
    /// The end of what git printed to standard error.
    pub stderr: Stderr,
    /// Whether the consumer stopped the stream early (git was then stopped too).
    pub stopped: bool,
}

/// How reading standard output ended, short of success.
enum ReadError {
    TooLarge(usize),
    Io(io::Error),
}

/// One run of git, before its output is interpreted.
struct Attempt {
    status: ExitStatus,
    stderr: Stderr,
    stopped: bool,
}

impl GitCommand {
    /// The default for [`Self::stdout_limit`].
    pub const DEFAULT_STDOUT_LIMIT: usize = 64 * 1024 * 1024;

    pub(crate) const fn new(git: Git, access: Access, subcommand: &'static str) -> Self {
        Self {
            git,
            access,
            subcommand,
            config: Vec::new(),
            args: Vec::new(),
            dir: None,
            stdin: None,
            env: Vec::new(),
            cancel: None,
            stdout_limit: Self::DEFAULT_STDOUT_LIMIT,
            progress: None,
            secret: false,
        }
    }

    /// Reports what git prints to standard error as it works into `progress` (pass git's own
    /// `--progress` too: it reports only to a terminal unless asked).
    pub fn progress(mut self, progress: &Progress) -> Self {
        self.progress = Some(progress.clone());
        self
    }

    /// Adds an argument after the subcommand.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Adds arguments after the subcommand.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    /// Sets a configuration value for this command only (`git -c key=value`), over any the
    /// user's configuration sets.
    pub fn config(mut self, key: &str, value: impl AsRef<OsStr>) -> Self {
        let mut setting = OsString::from(key);
        setting.push("=");
        setting.push(value);
        self.config.push(OsString::from("-c"));
        self.config.push(setting);
        self
    }

    /// Runs git in `dir` (a repository, or anywhere inside one).
    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// Feeds `input` to git's standard input (otherwise it reads nothing).
    pub fn stdin(mut self, input: impl Into<Arc<[u8]>>) -> Self {
        self.stdin = Some(Input::Plain(input.into()));
        self
    }

    /// Feeds `input`, a secret, to git's standard input: it is wiped from memory once git has
    /// it, and never shown in debug output.
    pub fn secret_stdin(mut self, input: Zeroizing<Vec<u8>>) -> Self {
        self.stdin = Some(Input::Secret(Arc::new(input)));
        self
    }

    /// Sets an environment variable for this command, over egit's own.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Says what this command is doing, shown with any prompt it causes ("Fetching origin").
    pub fn prompt_context(self, context: impl AsRef<OsStr>) -> Self {
        self.env(crate::askpass::CONTEXT_VAR, context)
    }

    /// Makes every prompt fail at once instead of asking the user: for work nobody is
    /// watching, such as a background fetch.
    pub fn non_interactive(self) -> Self {
        self.env(crate::askpass::INTERACTIVE_VAR, "0")
            // Git Credential Manager asks in windows of its own.
            .env("GCM_INTERACTIVE", "never")
    }

    /// Runs under `token`: cancelling it stops the command.
    pub fn cancel_token(mut self, token: &CancelToken) -> Self {
        self.cancel = Some(token.clone());
        self
    }

    /// The most [`Self::output`] collects from standard output before giving up with
    /// [`GitError::OutputTooLarge`]. Defaults to [`Self::DEFAULT_STDOUT_LIMIT`].
    pub const fn stdout_limit(mut self, limit: usize) -> Self {
        self.stdout_limit = limit;
        self
    }

    /// The git command this runs (`log`, `commit`, …).
    #[must_use]
    pub const fn subcommand(&self) -> &'static str {
        self.subcommand
    }

    /// Whether this command only reads.
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    /// Runs the command and collects its output.
    ///
    /// # Errors
    ///
    /// [`GitError::Failed`] when git exits unsuccessfully, and any error of
    /// [`Self::output_unchecked`].
    pub fn output(self) -> Result<Completed, GitError> {
        let subcommand = self.subcommand;
        let completed = self.output_unchecked()?;
        if completed.status.success() {
            Ok(completed)
        } else {
            Err(GitError::failed(
                subcommand,
                completed.status,
                completed.stderr,
            ))
        }
    }

    /// Runs the command and collects its output, however git exits (for commands whose exit
    /// code is an answer, such as `git symbolic-ref --quiet`).
    ///
    /// A command that writes is retried while another git process holds a lock it needs.
    ///
    /// # Errors
    ///
    /// [`GitError::Cancelled`] when cancelled before git succeeded,
    /// [`GitError::OutputTooLarge`] past [`Self::stdout_limit`], [`GitError::Launch`] when git
    /// can't be started, and [`GitError::Io`] when the pipes to it fail.
    pub fn output_unchecked(self) -> Result<Completed, GitError> {
        let mut delays = LOCK_RETRY_DELAYS.iter();
        loop {
            let completed = self.collect_once()?;
            if completed.status.success() || self.access == Access::Read {
                return Ok(completed);
            }
            let Some(lock) = completed.stderr.held_lock() else {
                return Ok(completed);
            };
            let Some(&delay) = delays.next() else {
                return Ok(completed);
            };
            tracing::debug!(
                subcommand = self.subcommand,
                %lock,
                ?delay,
                "another git holds a lock; trying again"
            );
            if self.cancel.clone().unwrap_or_default().sleep(delay) {
                return Err(self.cancelled());
            }
        }
    }

    /// Runs the command, however git exits, collecting standard output as a secret: into buffers
    /// wiped from memory when done with (growing never leaves a copy behind), at most 64 KiB.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::output_unchecked`].
    pub fn secret_output(mut self) -> Result<SecretOutput, GitError> {
        // Nothing of it in a transcript.
        self.secret = true;
        let (attempt, stdout) = self.run_once(|stdout| {
            let mut collected = Zeroizing::new(Vec::with_capacity(4096));
            let mut chunk = Zeroizing::new([0_u8; 4096]);
            loop {
                let read = match stdout.read(chunk.as_mut()) {
                    Ok(0) => return Ok((collected, false)),
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(ReadError::Io(error)),
                };
                let total = collected.len().saturating_add(read);
                if total > SECRET_LIMIT {
                    return Err(ReadError::TooLarge(SECRET_LIMIT));
                }
                if total > collected.capacity() {
                    let mut grown = Zeroizing::new(Vec::with_capacity(
                        collected.capacity().saturating_mul(2).max(total),
                    ));
                    grown.extend_from_slice(&collected);
                    // The smaller buffer is wiped as it goes.
                    collected = grown;
                }
                collected.extend_from_slice(chunk.get(..read).unwrap_or_default());
            }
        })?;
        Ok(SecretOutput {
            status: attempt.status,
            stdout,
            stderr: attempt.stderr,
        })
    }

    /// Runs the command, handing standard output to `consume` in chunks as git prints it.
    ///
    /// `consume` returns [`ControlFlow::Break`] to stop early, which stops git too. Chunks split
    /// git's output at arbitrary points; the buffer behind them is reused.
    ///
    /// # Errors
    ///
    /// [`GitError::Failed`] when git exits unsuccessfully (unless the consumer stopped it), and
    /// the errors of [`Self::output_unchecked`] other than the size limit.
    pub fn stream(
        self,
        mut consume: impl FnMut(&[u8]) -> ControlFlow<()>,
    ) -> Result<Streamed, GitError> {
        let (attempt, ()) = self.run_once(|stdout| {
            let mut buffer = vec![0; STREAM_CHUNK].into_boxed_slice();
            loop {
                let read = match stdout.read(&mut buffer) {
                    Ok(0) => return Ok(((), false)),
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(ReadError::Io(error)),
                };
                let chunk = buffer.get(..read).unwrap_or_default();
                if let Some(transcript) = self.transcript() {
                    transcript.feed(Stream::Out, chunk);
                }
                if consume(chunk).is_break() {
                    return Ok(((), true));
                }
            }
        })?;
        if attempt.stopped || attempt.status.success() {
            Ok(Streamed {
                status: attempt.status,
                stderr: attempt.stderr,
                stopped: attempt.stopped,
            })
        } else {
            Err(GitError::failed(
                self.subcommand,
                attempt.status,
                attempt.stderr,
            ))
        }
    }

    /// Starts git and keeps it running for one of its batch protocols (such as
    /// `cat-file --batch-command`): requests go to its standard input, answers come from its
    /// standard output. Any [`Self::stdin`] given is ignored; a [`CancelToken`] isn't watched
    /// (dropping the process stops it).
    ///
    /// # Errors
    ///
    /// [`GitError::Cancelled`] when the token is already cancelled, [`GitError::Launch`] when
    /// git can't be started, and [`GitError::Io`] when its pipes or helper can't be set up.
    pub fn spawn(self) -> Result<GitProcess, GitError> {
        if self.cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Err(self.cancelled());
        }
        tracing::debug!(subcommand = self.subcommand, "starting git");
        let mut command = self.build();
        command.stdin(Stdio::piped());
        let mut child = process::spawn_in_new_session(&mut command)
            .map_err(|source| self.launch_error(source))?;
        let group = process::group_of(&child);
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            abandon(child, group, None);
            return Err(self.io_error(io::Error::other("git's pipes weren't set up")));
        };
        let reader = self.progress.as_ref().map(Progress::reader);
        let drain = thread::Builder::new()
            .name(format!("git {} stderr", self.subcommand))
            .stack_size(HELPER_STACK)
            .spawn(move || drain(stderr, reader, None));
        match drain {
            Ok(drain) => Ok(GitProcess::new(
                self.subcommand,
                child,
                stdin,
                stdout,
                drain,
            )),
            Err(error) => {
                abandon(child, group, None);
                Err(self.io_error(error))
            }
        }
    }

    /// Runs git once, collecting standard output up to the limit.
    fn collect_once(&self) -> Result<Completed, GitError> {
        let limit = self.stdout_limit;
        let transcript = self.transcript();
        let (attempt, stdout) = self.run_once(|stdout| {
            let mut collected = Vec::new();
            let allowed = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
            let read = match transcript {
                // Read as it comes, so the transcript shows it as it comes.
                Some(transcript) => read_teeing(stdout.take(allowed), &mut collected, transcript),
                None => stdout.take(allowed).read_to_end(&mut collected),
            };
            match read {
                Ok(_) if collected.len() > limit => Err(ReadError::TooLarge(limit)),
                Ok(_) => Ok((collected, false)),
                Err(error) => Err(ReadError::Io(error)),
            }
        })?;
        Ok(Completed {
            status: attempt.status,
            stdout,
            stderr: attempt.stderr,
        })
    }

    /// Spawns git, feeds it, reads it with `read`, and waits for it.
    ///
    /// `read` returns what it read and whether it stopped before the end of the output. Git is
    /// stopped (SIGTERM to its process group) whenever reading ends early.
    fn run_once<R>(
        &self,
        read: impl FnOnce(&mut ChildStdout) -> Result<(R, bool), ReadError>,
    ) -> Result<(Attempt, R), GitError> {
        if self.cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Err(self.cancelled());
        }
        tracing::debug!(subcommand = self.subcommand, access = ?self.access, "running git");
        let transcript = self.transcript();
        if let Some(transcript) = transcript {
            transcript.command(self.command_line());
        }

        let mut command = self.build();
        let mut child = process::spawn_in_new_session(&mut command)
            .map_err(|source| self.launch_error(source))?;
        let group = process::group_of(&child);
        let registration = self.cancel.as_ref().map(|token| token.register(group));

        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (stdin, Some(mut stdout), Some(stderr)) = pipes else {
            abandon(child, group, registration);
            return Err(self.io_error(io::Error::other("git's pipes weren't set up")));
        };
        let helpers = match self.start_helpers(stdin, stderr) {
            Ok(helpers) => helpers,
            Err(error) => {
                abandon(child, group, registration);
                return Err(self.io_error(error));
            }
        };

        let reading = read(&mut stdout);
        drop(stdout);
        if !matches!(reading, Ok((_, false))) {
            process::terminate_group(group);
        }

        let waited = process::wait_until_exited(group);
        // The leader has exited but isn't reaped: its id can't have been reused, so a cancel
        // racing with this point still signals the right group. After this, nothing signals it.
        drop(registration);
        let status = child.wait();
        let (stderr, fed) = helpers.join();
        waited.map_err(|error| self.io_error(error))?;
        let status = status.map_err(|error| self.io_error(error))?;

        let cancelled = self.cancel.as_ref().is_some_and(CancelToken::is_cancelled);
        if let Some(transcript) = transcript {
            transcript.finish(Stream::Out);
            if cancelled && !status.success() {
                transcript.ended(false, "Cancelled.".to_owned());
            } else if !status.success() {
                transcript.ended(false, format!("git {} ended: {status}.", self.subcommand));
            }
        }
        if cancelled && !status.success() {
            return Err(self.cancelled());
        }
        let (output, stopped) = match reading {
            Ok(read) => read,
            Err(ReadError::TooLarge(limit)) => {
                return Err(GitError::OutputTooLarge {
                    subcommand: self.subcommand,
                    limit,
                });
            }
            Err(ReadError::Io(error)) => return Err(self.io_error(error)),
        };
        if status.success() {
            fed.map_err(|error| self.io_error(error))?;
        }
        Ok((
            Attempt {
                status,
                stderr,
                stopped,
            },
            output,
        ))
    }

    /// Starts the threads that feed git's standard input and drain its standard error.
    fn start_helpers(&self, stdin: Option<ChildStdin>, stderr: ChildStderr) -> io::Result<Helpers> {
        let reader = self.progress.as_ref().map(Progress::reader);
        let transcript = self.transcript().cloned();
        let drain = thread::Builder::new()
            .name(format!("git {} stderr", self.subcommand))
            .stack_size(HELPER_STACK)
            .spawn(move || {
                let tail = drain(stderr, reader, transcript.as_ref());
                if let Some(transcript) = &transcript {
                    transcript.finish(Stream::Err);
                }
                tail
            })?;
        let feed = match (stdin, &self.stdin) {
            (Some(pipe), Some(input)) => {
                let input = input.clone();
                Some(
                    thread::Builder::new()
                        .name(format!("git {} stdin", self.subcommand))
                        .stack_size(HELPER_STACK)
                        .spawn(move || feed(pipe, input.bytes()))?,
                )
            }
            _ => None,
        };
        Ok(Helpers { drain, feed })
    }

    /// The process, ready to spawn.
    fn build(&self) -> Command {
        let mut command = Command::new(self.git.program());
        self.git.environment().apply(&mut command);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        if self.access == Access::Read {
            command
                .env("GIT_OPTIONAL_LOCKS", "0")
                .arg("--no-optional-locks");
        }
        // Shown in a transcript, which draws colours: the pre-commit framework's in colour.
        if self.transcript().is_some() {
            command.env("PRE_COMMIT_COLOR", "always");
        }
        for setting in DEFAULT_CONFIG {
            command.arg("-c").arg(setting);
        }
        command
            .args(&self.config)
            .arg(self.subcommand)
            .args(&self.args)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.dir {
            command.current_dir(dir);
        }
        command
    }

    fn launch_error(&self, source: io::Error) -> GitError {
        match &self.dir {
            Some(dir) if !dir.is_dir() => self.io_error(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} isn't a directory", dir.display()),
            )),
            _ => GitError::Launch {
                program: self.git.program().to_owned(),
                source,
            },
        }
    }

    /// The transcript this command prints into: its git's, if it may write and carries no
    /// secret.
    fn transcript(&self) -> Option<&Transcript> {
        let secret = self.secret || matches!(self.stdin, Some(Input::Secret(_)));
        (self.access == Access::Write && !secret)
            .then(|| self.git.transcript())
            .flatten()
    }

    /// The command as a terminal would show it: `git` and its arguments (egit's own `-c`
    /// settings left out), any with a space quoted.
    fn command_line(&self) -> String {
        let mut line = format!("git {}", self.subcommand);
        for arg in &self.args {
            let arg = arg.to_string_lossy();
            line.push(' ');
            if arg.contains([' ', '\'', '"']) || arg.is_empty() {
                line.push('\'');
                line.push_str(&arg.replace('\'', "'\\''"));
                line.push('\'');
            } else {
                line.push_str(&arg);
            }
        }
        line
    }

    const fn io_error(&self, source: io::Error) -> GitError {
        GitError::Io {
            subcommand: self.subcommand,
            source,
        }
    }

    const fn cancelled(&self) -> GitError {
        GitError::Cancelled {
            subcommand: self.subcommand,
        }
    }
}

/// Stops a git process that won't be read after all, and reaps it.
fn abandon(mut child: Child, group: Group, registration: Option<Registration<'_>>) {
    process::terminate_group(group);
    // The caller reports why the process was abandoned; failing to wait here only means it is
    // already gone.
    if let Err(error) = process::wait_until_exited(group) {
        tracing::debug!(%error, "waiting for an abandoned git failed");
    }
    drop(registration);
    if let Err(error) = child.wait() {
        tracing::debug!(%error, "reaping an abandoned git failed");
    }
}

/// The threads around one git process.
struct Helpers {
    drain: JoinHandle<Stderr>,
    feed: Option<JoinHandle<io::Result<()>>>,
}

impl Helpers {
    /// Waits for both threads: what git printed to standard error, and whether feeding it worked.
    fn join(self) -> (Stderr, io::Result<()>) {
        let stderr = self.drain.join().unwrap_or_else(|_| {
            tracing::error!("the thread reading git's stderr panicked");
            Stderr::default()
        });
        let fed = self.feed.map_or(Ok(()), |feed| {
            feed.join()
                .unwrap_or_else(|_| Err(io::Error::other("the thread feeding git panicked")))
        });
        (stderr, fed)
    }
}

/// Reads standard error to its end, keeping the tail and reporting progress to `reader` (and
/// everything to `transcript`).
fn drain(
    mut stderr: ChildStderr,
    mut reader: Option<ProgressReader>,
    transcript: Option<&Transcript>,
) -> Stderr {
    let mut tail = TailBuffer::default();
    let mut buffer = [0; 4096];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let chunk = buffer.get(..read).unwrap_or_default();
                tail.push(chunk);
                if let Some(reader) = &mut reader {
                    reader.feed(chunk);
                }
                if let Some(transcript) = transcript {
                    transcript.feed(Stream::Err, chunk);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::warn!(%error, "reading git's stderr failed");
                break;
            }
        }
    }
    tail.finish()
}

/// Reads `stdout` to its end into `collected`, each chunk into `transcript` too as it comes.
fn read_teeing(
    mut stdout: impl io::Read,
    collected: &mut Vec<u8>,
    transcript: &Transcript,
) -> io::Result<usize> {
    let mut buffer = [0; 4096];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => return Ok(collected.len()),
            Ok(read) => {
                let chunk = buffer.get(..read).unwrap_or_default();
                transcript.feed(Stream::Out, chunk);
                collected.extend_from_slice(chunk);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// Writes `input` to git's standard input and closes it. Git may stop reading early (it failed,
/// or needed less); a broken pipe is then no error of ours.
fn feed(mut stdin: ChildStdin, input: &[u8]) -> io::Result<()> {
    match stdin.write_all(input) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}
