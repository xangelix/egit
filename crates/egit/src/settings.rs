//! What egit remembers between runs: the theme and the other preferences, recent repositories,
//! the open tabs, and where the last clone went.
//!
//! Stored by eframe (`~/.local/share/egit/app.ron`). Paths are kept as text when they are
//! UTF-8 and as bytes otherwise, so every path the system allows survives.

use std::path::{Path, PathBuf};

use bstr::{ByteSlice as _, ByteVec as _};

use serde::{Deserialize, Serialize};

use crate::preferences::Preferences;
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
    pub(crate) open: Vec<OpenTab>,
    /// The folder the last clone went into (the next goes there too).
    pub(crate) clone_folder: Option<StoredPath>,
    /// How the user likes egit.
    pub(crate) preferences: Preferences,
    /// The accounts agents run as, besides Claude Code's own.
    pub(crate) agent_accounts: Vec<crate::agents::AgentAccount>,
}

/// A repository opened before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recent {
    /// Its work tree.
    pub(crate) path: StoredPath,
    /// The other computer it is on, as ssh reaches it; none here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<String>,
    /// When it was last opened (seconds since the Unix epoch).
    pub(crate) opened: u64,
}

impl Recent {
    /// Where it is.
    pub(crate) fn location(&self) -> Location {
        Location {
            host: self.host.clone(),
            path: self.path.to_path(),
        }
    }
}

/// A repository open when egit last closed: a folder here, or one on another computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum OpenTab {
    Here(StoredPath),
    There { host: String, path: StoredPath },
}

impl OpenTab {
    /// Where it is.
    pub(crate) fn location(&self) -> Location {
        match self {
            Self::Here(path) => Location::here(path.to_path()),
            Self::There { host, path } => Location {
                host: Some(host.clone()),
                path: path.to_path(),
            },
        }
    }
}

impl From<&Location> for OpenTab {
    fn from(location: &Location) -> Self {
        let path = StoredPath::from(location.path.as_path());
        match &location.host {
            None => Self::Here(path),
            Some(host) => Self::There {
                host: host.clone(),
                path,
            },
        }
    }
}

/// Where a repository is: a folder on this computer, or one on another (`host`, as ssh
/// reaches it: `host`, `user@host`, an alias of `~/.ssh/config`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Location {
    pub(crate) host: Option<String>,
    pub(crate) path: PathBuf,
}

impl Location {
    /// A folder on this computer.
    pub(crate) const fn here(path: PathBuf) -> Self {
        Self { host: None, path }
    }
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
            // Byte for byte where paths are bytes (Unix); elsewhere as near as UTF-8 comes.
            Self::Bytes(bytes) => bytes.to_path_lossy().into_owned(),
        }
    }
}

impl From<&Path> for StoredPath {
    fn from(path: &Path) -> Self {
        path.to_str().map_or_else(
            || Self::Bytes(<Vec<u8>>::from_path_lossy(path).into_owned()),
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

    /// Records that `location` was opened at `now`, first in the recent list.
    pub(crate) fn remember(&mut self, location: &Location, now: u64) {
        self.forget(location);
        self.recent.insert(
            0,
            Recent {
                path: StoredPath::from(location.path.as_path()),
                host: location.host.clone(),
                opened: now,
            },
        );
        self.recent.truncate(MAX_RECENT);
    }

    /// Removes `location` from the recent list.
    pub(crate) fn forget(&mut self, location: &Location) {
        let stored = StoredPath::from(location.path.as_path());
        self.recent
            .retain(|recent| recent.path != stored || recent.host != location.host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_most_recent_first_without_duplicates() {
        let mut settings = Settings::default();
        settings.remember(&Location::here("/a".into()), 1);
        settings.remember(&Location::here("/b".into()), 2);
        settings.remember(&Location::here("/a".into()), 3);
        let paths: Vec<_> = settings
            .recent
            .iter()
            .map(|recent| recent.path.to_path())
            .collect();
        assert_eq!(paths, [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(settings.recent[0].opened, 3);

        settings.forget(&Location::here("/a".into()));
        assert_eq!(settings.recent.len(), 1);

        // The same path on another computer is another repository.
        let there = Location {
            host: Some("devbox".to_owned()),
            path: "/b".into(),
        };
        settings.remember(&there, 4);
        assert_eq!(settings.recent.len(), 2);
        assert_eq!(settings.recent[0].location(), there);
        settings.forget(&Location::here("/b".into()));
        assert_eq!(settings.recent.len(), 1);
    }

    #[test]
    fn open_tabs_are_kept_here_or_there() {
        let tabs = vec![
            OpenTab::Here(StoredPath::Text("/a".to_owned())),
            OpenTab::There {
                host: "devbox".to_owned(),
                path: StoredPath::Text("/b".to_owned()),
            },
        ];
        let written = ron::to_string(&tabs).unwrap();
        let read: Vec<OpenTab> = ron::from_str(&written).unwrap();
        assert_eq!(read, tabs);
        // As an earlier egit wrote them: paths alone.
        let earlier: Vec<OpenTab> = ron::from_str(r#"["/a"]"#).unwrap();
        assert_eq!(earlier[0].location(), Location::here("/a".into()));
    }

    #[test]
    fn keeps_only_so_many() {
        let mut settings = Settings::default();
        for index in 0..30 {
            settings.remember(&Location::here(format!("/repo{index}").into()), index);
        }
        assert_eq!(settings.recent.len(), MAX_RECENT);
        assert_eq!(settings.recent[0].path.to_path(), PathBuf::from("/repo29"));
    }

    #[cfg(unix)]
    #[test]
    fn paths_that_arent_utf8_survive() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let path = PathBuf::from(OsString::from_vec(b"/caf\xe9".to_vec()));
        let stored = StoredPath::from(path.as_path());
        assert!(matches!(stored, StoredPath::Bytes(_)));
        assert_eq!(stored.to_path(), path);
        let text = StoredPath::from(Path::new("/café"));
        assert_eq!(text, StoredPath::Text("/café".to_owned()));
    }
}
