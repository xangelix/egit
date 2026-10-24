//! What can go wrong talking to a forge.

use egit_git::GitError;

/// Why a forge request failed.
#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    /// Git's credential helpers have no credential for the host, and asking wasn't allowed or
    /// the user declined.
    #[error("not signed in to {host}")]
    NotSignedIn {
        /// The host.
        host: String,
    },
    /// The host refused the credential (it was then forgotten).
    #[error("{host} refused the credentials")]
    Refused {
        /// The host.
        host: String,
    },
    /// The host's rate limit is used up, until `reset` (seconds since the Unix epoch) when known.
    #[error("{host}'s rate limit is used up")]
    RateLimited {
        /// The host.
        host: String,
        /// When it resets.
        reset: Option<i64>,
    },
    /// The repository (or pull request) doesn't exist, or the credential can't see it.
    #[error("{what} wasn't found on {host}, or isn't visible to this account")]
    NotFound {
        /// The host.
        host: String,
        /// What was asked for (`owner/name`, `owner/name#12`).
        what: String,
    },
    /// The host refused the request for another reason (single sign-on required, the token's
    /// scopes, …), saying why.
    #[error("{host} refused: {message}")]
    Forbidden {
        /// The host.
        host: String,
        /// The host's reason.
        message: String,
    },
    /// The host answered with an error, saying why.
    #[error("{host} answered {status}: {message}")]
    Status {
        /// The host.
        host: String,
        /// The HTTP status.
        status: u16,
        /// The host's reason.
        message: String,
    },
    /// The host's API answered its query with an error (GraphQL's), saying why.
    #[error("{host} said: {message}")]
    Said {
        /// The host.
        host: String,
        /// The host's reason.
        message: String,
    },
    /// The host couldn't be reached (no network, a proxy, a certificate, a timeout).
    #[error("{host} couldn't be reached: {message}")]
    Network {
        /// The host.
        host: String,
        /// What went wrong.
        message: String,
    },
    /// The host answered something egit can't read.
    #[error("{host} answered something egit can't read: {detail}")]
    Malformed {
        /// The host.
        host: String,
        /// What was wrong with it.
        detail: String,
    },
    /// What was downloaded isn't what the host's digest of it says: changed on the way, or
    /// since the digest was taken.
    #[error(
        "{what} doesn't match the SHA-256 digest {host} gave for it: it may have been changed on \
         the way"
    )]
    Mismatched {
        /// The host.
        host: String,
        /// What was downloaded (its file).
        what: String,
    },
    /// A download for the browser didn't reach it whole (the browser stopped it, never came
    /// for it, or the relay found it doesn't match its digest): why, in the relay's words.
    #[error("{what} didn't reach the browser: {why}")]
    Undelivered {
        /// What was downloaded (its file).
        what: String,
        /// Why.
        why: String,
    },
    /// A credential would have gone over a connection that isn't encrypted.
    #[error("not sending credentials to {host} without HTTPS")]
    Insecure {
        /// The host.
        host: String,
    },
    /// It was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Asking git for the credential failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

impl ForgeError {
    /// Whether signing in (again) is what would help.
    #[must_use]
    pub const fn needs_sign_in(&self) -> bool {
        matches!(self, Self::NotSignedIn { .. } | Self::Refused { .. })
    }
}
