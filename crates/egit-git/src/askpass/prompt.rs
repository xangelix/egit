//! A question git or ssh asks through its helper, waiting for the window's answer: from a
//! helper here (its connection waits for the answer), or from one a relay ran (answered through
//! the relay).

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use zeroize::Zeroizing;

use super::platform::{self, Stream};
use super::protocol::{self, PromptKind};
use crate::relay::Relay;

/// A prompt waiting for the window's answer.
///
/// Answer it with [`Self::answer`] or [`Self::refuse`]; dropping it refuses it. Meanwhile the
/// asker may give up (git was cancelled, ssh no longer needs the touch): check
/// [`Self::is_abandoned`] while it is shown.
pub struct Prompt {
    id: u64,
    kind: PromptKind,
    text: String,
    context: Option<String>,
    asker: Asker,
}

/// Who waits for the answer.
enum Asker {
    /// A helper here, on its connection (none once answered).
    Helper(Option<Stream>),
    /// A helper a relay ran.
    Relayed {
        relay: Relay,
        /// The relay's name for it.
        id: u32,
        /// Set when the relay says its asker went away.
        withdrawn: Arc<AtomicBool>,
        answered: bool,
    },
}

/// Numbers prompts, wherever they come from.
static NEXT: AtomicU64 = AtomicU64::new(1);

impl Prompt {
    /// A number no other prompt has.
    pub(crate) fn next_id() -> u64 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// A helper's prompt, its connection waiting for the answer.
    pub(crate) const fn from_helper(
        id: u64,
        kind: PromptKind,
        text: String,
        context: Option<String>,
        stream: Stream,
    ) -> Self {
        Self {
            id,
            kind,
            text,
            context,
            asker: Asker::Helper(Some(stream)),
        }
    }

    /// A prompt a relay relays (`id` its name for it), withdrawn when `withdrawn` is set.
    pub(crate) fn relayed(
        kind: PromptKind,
        text: String,
        context: Option<String>,
        (relay, id, withdrawn): (Relay, u32, Arc<AtomicBool>),
    ) -> Self {
        Self {
            id: Self::next_id(),
            kind,
            text,
            context,
            asker: Asker::Relayed {
                relay,
                id,
                withdrawn,
                answered: false,
            },
        }
    }

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
        match &self.asker {
            Asker::Helper(stream) => stream.as_ref().is_none_or(platform::hung_up),
            Asker::Relayed {
                relay, withdrawn, ..
            } => withdrawn.load(Ordering::Acquire) || relay.has_ended(),
        }
    }

    fn respond(&mut self, answer: Option<&[u8]>) -> io::Result<()> {
        match &mut self.asker {
            Asker::Helper(stream) => stream.take().map_or(Ok(()), |mut stream| {
                protocol::write_response(&mut stream, answer)
            }),
            Asker::Relayed {
                relay,
                id,
                answered,
                ..
            } => {
                if !std::mem::replace(answered, true) {
                    relay.answer(*id, answer.map(|answer| Zeroizing::new(answer.to_vec())));
                }
                Ok(())
            }
        }
    }
}

impl fmt::Debug for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prompt")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field("context", &self.context)
            .field("relayed", &matches!(self.asker, Asker::Relayed { .. }))
            .finish()
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        if let Err(error) = self.respond(None) {
            tracing::debug!(%error, "couldn't refuse a dropped prompt");
        }
    }
}
