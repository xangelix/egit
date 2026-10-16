//! Everything that can go wrong running git.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use bstr::BStr;

use crate::output::Stderr;
use crate::version::GitVersion;

/// Running git failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitError {
    /// No `git` executable was found on `PATH`.
    #[error("git isn't installed: there is no `git` on PATH")]
    NotFound,

    /// The executable couldn't be started.
    #[error("{} couldn't be started: {source}", program.display())]
    Launch {
        /// The executable.
        program: PathBuf,
        /// Why it couldn't be started.
        #[source]
        source: io::Error,
    },

    /// `git version` printed something that isn't a version.
    #[error("{} doesn't report a git version egit can read", program.display())]
    UnknownVersion {
        /// The executable.
        program: PathBuf,
    },

    /// The installed git is older than egit supports.
    #[error(
        "git {found} is too old: egit needs git {} or newer",
        GitVersion::MINIMUM
    )]
    Unsupported {
        /// The version found.
        found: GitVersion,
    },

    /// Git ran and exited unsuccessfully.
    #[error(transparent)]
    Failed(Box<Failure>),

    /// The command was cancelled before it finished.
    #[error("`git {subcommand}` was cancelled")]
    Cancelled {
        /// The git command (`commit`, `fetch`, …).
        subcommand: &'static str,
    },

    /// Git printed more than the caller allowed it to.
    #[error("`git {subcommand}` printed more than {limit} bytes")]
    OutputTooLarge {
        /// The git command.
        subcommand: &'static str,
        /// The limit it exceeded.
        limit: usize,
    },

    /// Reading from or writing to git failed.
    #[error("couldn't communicate with `git {subcommand}`: {source}")]
    Io {
        /// The git command.
        subcommand: &'static str,
        /// What failed.
        #[source]
        source: io::Error,
    },

    /// Git printed something egit couldn't make sense of.
    #[error("`git {subcommand}` printed something egit can't read: {detail}")]
    Unexpected {
        /// The git command.
        subcommand: &'static str,
        /// What was wrong with it.
        detail: String,
    },
}

impl GitError {
    /// The failure, if git ran and exited unsuccessfully.
    #[must_use]
    pub fn failure(&self) -> Option<&Failure> {
        match self {
            Self::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    /// Whether git failed because another git process holds a lock in the repository.
    #[must_use]
    pub fn is_lock_contention(&self) -> bool {
        self.failure()
            .is_some_and(|failure| failure.stderr.held_lock().is_some())
    }

    /// An [`Unexpected`](Self::Unexpected) error.
    pub(crate) fn unexpected(subcommand: &'static str, detail: impl Into<String>) -> Self {
        Self::Unexpected {
            subcommand,
            detail: detail.into(),
        }
    }
}

/// A git command that ran and exited unsuccessfully.
#[derive(Debug, thiserror::Error)]
#[error("`git {subcommand}` failed ({status}): {message}", message = stderr.message())]
pub struct Failure {
    /// The git command.
    pub subcommand: &'static str,
    /// How it exited.
    pub status: ExitStatus,
    /// The end of what it printed to standard error.
    pub stderr: Stderr,
}

impl Failure {
    /// The exit code, unless git was ended by a signal.
    #[must_use]
    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }

    /// The lock file git couldn't take because another git process holds it.
    #[must_use]
    pub fn held_lock(&self) -> Option<&BStr> {
        self.stderr.held_lock()
    }
}
