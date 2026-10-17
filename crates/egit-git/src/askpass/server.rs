//! The window's side: a private socket helpers connect to, and the prompts they bring.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::platform::{
    self, Listener, Presence, Stopper, Stream, ensure_private, make_private_dir, private_base,
};
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
    /// Where helpers connect (a socket's path, or a port on the loopback address).
    address: OsString,
    helper: PathBuf,
    token: Token,
    /// Stops the accept loop.
    stopper: Stopper,
    /// Marks the directory as in use, where that needs something kept.
    presence: Option<Presence>,
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
    stream: Option<Stream>,
}

impl AskpassServer {
    /// Starts relaying prompts for the executable at `executable` (normally
    /// [`std::env::current_exe`]), in a directory private to this user: on Unix under
    /// `$XDG_RUNTIME_DIR`, or the temporary directory when that isn't usable; on Windows under
    /// the user's temporary directory.
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
        make_private_dir(&dir)?;
        let cleanup = RemoveOnDrop(Some(dir.clone()));
        let presence = platform::mark_present(&dir)?;

        let helper = dir.join(HELPER_NAME);
        platform::link_helper(executable, &helper)?;
        let (listener, stopper, address) = Listener::bind(&dir)?;
        let (sender, receiver) = mpsc::channel();
        let accept = {
            let token = token.clone();
            thread::Builder::new()
                .name("askpass".to_owned())
                .stack_size(128 * 1024)
                .spawn(move || accept_loop(&listener, &token, &sender, &notify))?
        };
        cleanup.disarm();
        Ok((
            Self {
                dir,
                address,
                helper,
                token,
                stopper,
                presence: Some(presence),
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
            (OsString::from(SOCKET_VAR), self.address.clone()),
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
        if let Err(error) = self.stopper.stop() {
            tracing::error!(%error, "couldn't stop the askpass server");
            // Its thread may never end: leave it rather than wait forever.
            self.accept = None;
        }
        if let Some(accept) = self.accept.take()
            && accept.join().is_err()
        {
            tracing::error!("the askpass server's thread panicked");
        }
        // Let go of the directory before removing it.
        self.presence = None;
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
        self.stream.as_ref().is_none_or(platform::hung_up)
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

/// Accepts helpers until the server is stopped.
fn accept_loop(listener: &Listener, token: &Token, prompts: &Sender<Prompt>, notify: &impl Fn()) {
    let mut next_id = 0_u64;
    listener.serve(|stream| {
        next_id = next_id.wrapping_add(1);
        match receive(stream, token, next_id) {
            Ok(prompt) => {
                if prompts.send(prompt).is_err() {
                    // Nobody is listening any more; the prompt was refused as it dropped.
                    return false;
                }
                notify();
            }
            Err(error) => tracing::warn!(%error, "refused an askpass connection"),
        }
        true
    });
}

/// Checks a connection and reads its request.
fn receive(stream: Stream, token: &Token, id: u64) -> io::Result<Prompt> {
    stream.set_nonblocking(false)?;
    if !platform::same_user(&stream)? {
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
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if pid != std::process::id()
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && platform::is_stale(&entry.path(), pid)
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
