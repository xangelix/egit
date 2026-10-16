//! The window's side: a private socket helpers connect to, and the prompts they bring.

use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustix::event::{EventfdFlags, PollFd, PollFlags};
use rustix::io::Errno;
use rustix::net::RecvFlags;

use super::protocol::{self, PromptKind, Token};
use super::{HELPER_NAME, SOCKET_VAR, TOKEN_VAR};

/// How long a connecting helper has to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long writing an answer may take before the helper counts as gone.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// Relays prompts from git's and ssh's helpers to the window.
///
/// Lives in a directory of its own (removed when it is dropped) holding the socket and the
/// helper: a link to egit's executable named [`HELPER_NAME`]. Every git process needs
/// [`Self::environment`] to ask through it.
#[derive(Debug)]
pub struct AskpassServer {
    dir: PathBuf,
    socket: PathBuf,
    helper: PathBuf,
    token: Token,
    /// Wakes the accept loop to stop it. Its own descriptor, so stopping never depends on the
    /// socket's path still being there.
    wake: Arc<OwnedFd>,
    accept: Option<JoinHandle<()>>,
}

/// A prompt waiting for the window's answer.
///
/// Answer it with [`Self::answer`] or [`Self::refuse`]; dropping it refuses it. Meanwhile the
/// asker may give up (git was cancelled, ssh no longer needs the touch): check
/// [`Self::is_abandoned`] while it is shown.
#[derive(Debug)]
pub struct Prompt {
    id: u64,
    kind: PromptKind,
    text: String,
    context: Option<String>,
    stream: Option<UnixStream>,
}

impl AskpassServer {
    /// Starts relaying prompts for the executable at `executable` (normally
    /// [`std::env::current_exe`]), in a directory private to this user: under
    /// `$XDG_RUNTIME_DIR`, or the temporary directory when that isn't usable.
    ///
    /// `notify` is called, on the server's own thread, whenever a prompt arrives; it should wake
    /// the window. Prompts arrive on the returned receiver.
    ///
    /// # Errors
    ///
    /// When no private directory can be made, or the socket can't be opened.
    pub fn start(
        executable: &Path,
        notify: impl Fn() + Send + 'static,
    ) -> io::Result<(Self, Receiver<Prompt>)> {
        Self::start_in(&private_base()?, executable, notify)
    }

    /// As [`Self::start`], inside `base`, which only this user may enter.
    ///
    /// # Errors
    ///
    /// When the directory or socket can't be made, or `base` isn't private.
    pub fn start_in(
        base: &Path,
        executable: &Path,
        notify: impl Fn() + Send + 'static,
    ) -> io::Result<(Self, Receiver<Prompt>)> {
        ensure_private(base)?;
        sweep_stale(base);
        let token = Token::random()?;
        let nonce = Token::random()?.to_hex();
        let dir = base.join(format!(
            "{}-{}",
            std::process::id(),
            nonce.get(..8).unwrap_or(&nonce)
        ));
        DirBuilder::new().mode(0o700).create(&dir)?;
        let cleanup = RemoveOnDrop(Some(dir.clone()));

        let socket = dir.join("socket");
        let helper = dir.join(HELPER_NAME);
        std::os::unix::fs::symlink(executable, &helper)?;
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let wake = Arc::new(rustix::event::eventfd(
            0,
            EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK,
        )?);
        let (sender, receiver) = mpsc::channel();
        let accept = {
            let token = token.clone();
            let wake = Arc::clone(&wake);
            thread::Builder::new()
                .name("askpass".to_owned())
                .stack_size(128 * 1024)
                .spawn(move || accept_loop(&listener, &wake, &token, &sender, &notify))?
        };
        cleanup.disarm();
        Ok((
            Self {
                dir,
                socket,
                helper,
                token,
                wake,
                accept: Some(accept),
            },
            receiver,
        ))
    }

    /// The variables that make git and ssh ask through this server. Every git process needs
    /// them.
    #[must_use]
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        let helper = self.helper.as_os_str().to_owned();
        vec![
            (OsString::from("GIT_ASKPASS"), helper.clone()),
            (OsString::from("SSH_ASKPASS"), helper),
            // ssh asks the helper even when it could find a terminal.
            (
                OsString::from("SSH_ASKPASS_REQUIRE"),
                OsString::from("force"),
            ),
            (
                OsString::from(SOCKET_VAR),
                self.socket.as_os_str().to_owned(),
            ),
            (
                OsString::from(TOKEN_VAR),
                OsString::from(self.token.to_hex()),
            ),
        ]
    }

    /// The helper: a link to egit's executable named [`HELPER_NAME`].
    #[must_use]
    pub fn helper(&self) -> &Path {
        &self.helper
    }
}

impl Drop for AskpassServer {
    fn drop(&mut self) {
        if let Err(error) = rustix::io::write(&*self.wake, &1_u64.to_ne_bytes()) {
            tracing::error!(%error, "couldn't stop the askpass server");
            // Its thread may never end: leave it rather than wait forever.
            self.accept = None;
        }
        if let Some(accept) = self.accept.take()
            && accept.join().is_err()
        {
            tracing::error!("the askpass server's thread panicked");
        }
        if let Err(error) = fs::remove_dir_all(&self.dir) {
            tracing::warn!(%error, dir = %self.dir.display(), "couldn't remove the askpass directory");
        }
    }
}

impl Prompt {
    /// A number telling this prompt apart from others while it is shown.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// What it asks for.
    #[must_use]
    pub const fn kind(&self) -> PromptKind {
        self.kind
    }

    /// The prompt as git or ssh wrote it, such as `Password for 'https://me@example.com': `.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What egit was doing when it was asked, when the command said
    /// ([`GitCommand::prompt_context`](crate::GitCommand::prompt_context)).
    #[must_use]
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// Answers: the text typed (for [`PromptKind::Confirm`], any answer accepts).
    ///
    /// # Errors
    ///
    /// When the asker is gone or the answer is longer than a helper accepts.
    pub fn answer(mut self, answer: &[u8]) -> io::Result<()> {
        self.respond(Some(answer))
    }

    /// Refuses: the asker fails as if the user had cancelled.
    ///
    /// # Errors
    ///
    /// When the asker is gone.
    pub fn refuse(mut self) -> io::Result<()> {
        self.respond(None)
    }

    /// Whether the asker has gone away without waiting for an answer.
    #[must_use]
    pub fn is_abandoned(&self) -> bool {
        let Some(stream) = &self.stream else {
            return true;
        };
        let mut byte = [0_u8; 1];
        match rustix::net::recv(stream, &mut byte, RecvFlags::PEEK | RecvFlags::DONTWAIT) {
            // The other end closed.
            Ok((0, _)) => true,
            Ok(_) | Err(Errno::AGAIN | Errno::INTR) => false,
            Err(_) => true,
        }
    }

    fn respond(&mut self, answer: Option<&[u8]>) -> io::Result<()> {
        self.stream.take().map_or(Ok(()), |mut stream| {
            protocol::write_response(&mut stream, answer)
        })
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        if let Err(error) = self.respond(None) {
            tracing::debug!(%error, "couldn't refuse a dropped prompt");
        }
    }
}

/// Accepts helpers until `wake` is signalled.
fn accept_loop(
    listener: &UnixListener,
    wake: &OwnedFd,
    token: &Token,
    prompts: &Sender<Prompt>,
    notify: &impl Fn(),
) {
    let mut next_id = 0_u64;
    loop {
        let mut ready = [
            PollFd::new(listener, PollFlags::IN),
            PollFd::new(wake, PollFlags::IN),
        ];
        match rustix::event::poll(&mut ready, None) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(error) => {
                tracing::error!(%error, "the askpass server stopped");
                return;
            }
        }
        let [listening, woken] = &ready;
        if !woken.revents().is_empty() {
            return;
        }
        if listening.revents().is_empty() {
            continue;
        }
        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    tracing::warn!(%error, "askpass connection failed");
                    // Don't spin on an error that persists (no file descriptors left, say).
                    thread::sleep(Duration::from_millis(50));
                    break;
                }
            };
            next_id = next_id.wrapping_add(1);
            match receive(stream, token, next_id) {
                Ok(prompt) => {
                    if prompts.send(prompt).is_err() {
                        // Nobody is listening any more; the prompt was refused as it dropped.
                        return;
                    }
                    notify();
                }
                Err(error) => tracing::warn!(%error, "refused an askpass connection"),
            }
        }
    }
}

/// Checks a connection and reads its request.
fn receive(stream: UnixStream, token: &Token, id: u64) -> io::Result<Prompt> {
    stream.set_nonblocking(false)?;
    let peer = rustix::net::sockopt::socket_peercred(&stream)?;
    if peer.uid != rustix::process::getuid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "a helper of another user",
        ));
    }
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let request = protocol::read_request(&mut &stream)?;
    if !token.matches(&request.token) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "a helper without egit's token",
        ));
    }
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(ANSWER_TIMEOUT))?;
    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    Ok(Prompt {
        id,
        kind: request.kind,
        text: lossy(&request.prompt),
        context: (!request.context.is_empty()).then(|| lossy(&request.context)),
        stream: Some(stream),
    })
}

/// `egit-<uid>` under `$XDG_RUNTIME_DIR`, or under the temporary directory when the runtime
/// directory is missing or unusable.
fn private_base() -> io::Result<PathBuf> {
    let name = format!("egit-{}", rustix::process::getuid().as_raw());
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    let mut failure = None;
    for parent in runtime.into_iter().chain([std::env::temp_dir()]) {
        let base = parent.join(&name);
        match ensure_private(&base) {
            Ok(()) => return Ok(base),
            Err(error) => {
                tracing::debug!(%error, base = %base.display(), "not a usable askpass directory");
                failure = Some(error);
            }
        }
    }
    Err(failure.unwrap_or_else(|| io::Error::other("no directory for the askpass socket")))
}

/// Creates `dir` (0700) unless it exists, then refuses it unless it is a directory this user
/// owns that nobody else may enter.
fn ensure_private(dir: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(dir)?;
    let ours = metadata.uid() == rustix::process::getuid().as_raw();
    #[expect(
        clippy::verbose_bit_mask,
        reason = "permission bits read best as a mask"
    )]
    let closed_to_others = metadata.mode() & 0o077 == 0;
    if metadata.is_dir() && ours && closed_to_others {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} isn't a directory private to this user", dir.display()),
        ))
    }
}

/// Removes the directories of servers whose process is gone (egit crashed, or was killed).
fn sweep_stale(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.split_once('-'))
            .map(|(pid, _)| pid)
            .filter(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        else {
            continue;
        };
        let alive = Path::new("/proc").join(pid).exists();
        if !alive
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Err(error) = fs::remove_dir_all(entry.path())
        {
            tracing::debug!(%error, "couldn't sweep a stale askpass directory");
        }
    }
}

/// Removes a directory when dropped, unless disarmed: cleanup for a start that fails midway.
struct RemoveOnDrop(Option<PathBuf>);

impl RemoveOnDrop {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take()
            && let Err(error) = fs::remove_dir_all(&dir)
        {
            tracing::debug!(%error, "couldn't remove a half-made askpass directory");
        }
    }
}
