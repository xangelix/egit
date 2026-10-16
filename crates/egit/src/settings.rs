//! What egit remembers between runs: the theme, recent repositories, and the open tabs.
//!
//! Stored by eframe (`~/.local/share/egit/app.ron`). Paths are kept as text when they are
//! UTF-8 and as bytes otherwise, so every path the system allows survives.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::theme::ThemeChoice;

/// The storage key.
const KEY: &str = "egit";

/// How many recent repositories are remembered.
pub(crate) const MAX_RECENT: usize = 20;

/// Everything remembered.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// The theme.
    pub(crate) theme: ThemeChoice,
    /// Repositories opened, the most recent first.
    pub(crate) recent: Vec<Recent>,
    /// The repositories open when egit last closed, in tab order.
    pub(crate) open: Vec<StoredPath>,
}

/// A repository opened before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recent {
    /// Its work tree.
    pub(crate) path: StoredPath,
    /// When it was last opened (seconds since the Unix epoch).
    pub(crate) opened: u64,
}

/// A path as stored: text when it is UTF-8, bytes otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredPath {
    /// A UTF-8 path.
    Text(String),
    /// Any other path, byte for byte.
    Bytes(Vec<u8>),
}

impl StoredPath {
    pub(crate) fn to_path(&self) -> PathBuf {
        match self {
            Self::Text(text) => PathBuf::from(text),
            Self::Bytes(bytes) => PathBuf::from(OsString::from_vec(bytes.clone())),
        }
    }
}

impl From<&Path> for StoredPath {
    fn from(path: &Path) -> Self {
        path.to_str().map_or_else(
            || Self::Bytes(path.as_os_str().as_bytes().to_vec()),
            |text| Self::Text(text.to_owned()),
        )
    }
}

impl Settings {
    /// The settings stored, or the defaults.
    pub(crate) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|storage| eframe::get_value(storage, KEY))
            .unwrap_or_default()
    }

    /// Stores the settings.
    pub(crate) fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, KEY, self);
    }

    /// Records that `path` was opened at `now`, first in the recent list.
    pub(crate) fn remember(&mut self, path: &Path, now: u64) {
        let stored = StoredPath::from(path);
        self.recent.retain(|recent| recent.path != stored);
        self.recent.insert(
            0,
            Recent {
                path: stored,
                opened: now,
            },
        );
        self.recent.truncate(MAX_RECENT);
    }

    /// Removes `path` from the recent list.
    pub(crate) fn forget(&mut self, path: &Path) {
        let stored = StoredPath::from(path);
        self.recent.retain(|recent| recent.path != stored);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_most_recent_first_without_duplicates() {
        let mut settings = Settings::default();
        settings.remember(Path::new("/a"), 1);
        settings.remember(Path::new("/b"), 2);
        settings.remember(Path::new("/a"), 3);
        let paths: Vec<_> = settings
            .recent
            .iter()
            .map(|recent| recent.path.to_path())
            .collect();
        assert_eq!(paths, [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(settings.recent[0].opened, 3);

        settings.forget(Path::new("/a"));
        assert_eq!(settings.recent.len(), 1);
    }

    #[test]
    fn keeps_only_so_many() {
        let mut settings = Settings::default();
        for index in 0..30 {
            settings.remember(Path::new(&format!("/repo{index}")), index);
        }
        assert_eq!(settings.recent.len(), MAX_RECENT);
        assert_eq!(settings.recent[0].path.to_path(), PathBuf::from("/repo29"));
    }

    #[test]
    fn paths_that_arent_utf8_survive() {
        let path = PathBuf::from(OsString::from_vec(b"/caf\xe9".to_vec()));
        let stored = StoredPath::from(path.as_path());
        assert!(matches!(stored, StoredPath::Bytes(_)));
        assert_eq!(stored.to_path(), path);
        let text = StoredPath::from(Path::new("/café"));
        assert_eq!(text, StoredPath::Text("/café".to_owned()));
    }
}
