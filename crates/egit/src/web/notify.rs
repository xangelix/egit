//! A browser has no files of its own to watch: `notify`'s types, as the watches name them, and
//! never a watch. A repository there is followed by a session through its relay instead
//! (`Watcher::elsewhere`).

#![allow(
    dead_code,
    clippy::unused_self,
    reason = "notify's own shape, which the watches name; nothing here is ever watched"
)]

use std::fmt;
use std::path::{Path, PathBuf};

/// What a watch's setting up gives.
pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Why a watch couldn't be set up.
#[derive(Debug)]
pub(crate) struct Error {
    pub(crate) kind: ErrorKind,
}

/// What kind of failure.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    /// In words.
    Generic(String),
    /// The system's.
    Io(std::io::Error),
    /// What was to be watched isn't there.
    PathNotFound,
    /// Too many watches.
    MaxFilesWatch,
}

impl Error {
    pub(crate) const fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }

    pub(crate) fn generic(message: &str) -> Self {
        Self::new(ErrorKind::Generic(message.to_owned()))
    }

    pub(crate) const fn io(error: std::io::Error) -> Self {
        Self::new(ErrorKind::Io(error))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Generic(message) => f.write_str(message),
            ErrorKind::Io(error) => error.fmt(f),
            ErrorKind::PathNotFound => f.write_str("not there"),
            ErrorKind::MaxFilesWatch => f.write_str("too many watches"),
        }
    }
}

impl std::error::Error for Error {}

/// Whether a folder's watch takes in those inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecursiveMode {
    Recursive,
    NonRecursive,
}

/// What a change was.
pub(crate) mod event {
    /// Something made.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum CreateKind {
        Any,
        File,
        Folder,
    }

    /// Something removed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum RemoveKind {
        Any,
        File,
        Folder,
    }

    /// Something changed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum ModifyKind {
        Any,
        Metadata(MetadataKind),
    }

    /// A change to what is said of it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum MetadataKind {
        Any,
    }

    /// Something read.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum AccessKind {
        Any,
    }
}

/// A change's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    Any,
    Access(event::AccessKind),
    Create(event::CreateKind),
    Modify(event::ModifyKind),
    Remove(event::RemoveKind),
    Other,
}

/// A change: never one here.
#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub(crate) kind: EventKind,
    pub(crate) paths: Vec<PathBuf>,
}

impl Event {
    pub(crate) const fn need_rescan(&self) -> bool {
        false
    }
}

/// A watch set up here: never one ([`recommended_watcher`] gives none).
#[derive(Debug)]
pub(crate) struct RecommendedWatcher;

/// Adding and removing watches.
pub(crate) trait Watcher {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> Result<()>;
    fn unwatch(&mut self, path: &Path) -> Result<()>;
}

impl Watcher for RecommendedWatcher {
    fn watch(&mut self, _path: &Path, _mode: RecursiveMode) -> Result<()> {
        Err(Error::generic(NOTHING))
    }

    fn unwatch(&mut self, _path: &Path) -> Result<()> {
        Err(Error::generic(NOTHING))
    }
}

/// Why nothing is watched.
const NOTHING: &str = "a browser watches nothing of its own";

/// Never sets a watch up: a browser has nothing of its own to watch.
pub(crate) fn recommended_watcher(
    _handler: impl FnMut(Result<Event>) + Send + 'static,
) -> Result<RecommendedWatcher> {
    Err(Error::generic(NOTHING))
}
