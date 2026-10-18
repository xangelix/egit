//! How the user likes egit: the text's size, how often repositories are fetched from in the
//! background, whether the sidebar shows, how diffs and files are shown at first (side by side,
//! lines wrapped, a tab's width), changed files (a list or a tree of their folders), and commit
//! dates (as dates, or how long ago).
//!
//! Kept with the other settings (`Settings::preferences`, older settings reading as the
//! defaults) and shared for the frame through the egui context (`Preferences::of`), so a view
//! reads them where it draws. A tab's width is wanted where text is measured, in jobs too: it
//! is also kept in a process-wide value (`text_options`).

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How often repositories are fetched from in the background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum FetchEvery {
    /// Never.
    Never,
    /// Every minute.
    Minute,
    /// Every five minutes.
    #[default]
    FiveMinutes,
    /// Every quarter of an hour.
    QuarterHour,
    /// Every half hour.
    HalfHour,
}

impl FetchEvery {
    /// Every choice, in the order offered.
    pub(crate) const ALL: [Self; 5] = [
        Self::Never,
        Self::Minute,
        Self::FiveMinutes,
        Self::QuarterHour,
        Self::HalfHour,
    ];

    /// How long between fetches; `None` for never.
    pub(crate) const fn interval(self) -> Option<Duration> {
        match self {
            Self::Never => None,
            Self::Minute => Some(Duration::from_mins(1)),
            Self::FiveMinutes => Some(Duration::from_mins(5)),
            Self::QuarterHour => Some(Duration::from_mins(15)),
            Self::HalfHour => Some(Duration::from_mins(30)),
        }
    }

    /// The choice, in words.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::Minute => "Every minute",
            Self::FiveMinutes => "Every 5 minutes",
            Self::QuarterHour => "Every 15 minutes",
            Self::HalfHour => "Every 30 minutes",
        }
    }
}

/// The smallest and largest text size, in percent.
pub(crate) const ZOOM: std::ops::RangeInclusive<u16> = 70..=200;

/// The tab widths offered, in cells.
pub(crate) const TAB_WIDTHS: [u8; 3] = [2, 4, 8];

/// How commit dates show in the commit table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Dates {
    /// When a commit was written (2026-10-17 14:05).
    #[default]
    Written,
    /// How long ago (5 minutes ago).
    Ago,
}

/// How lists of changed files show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FileLists {
    /// Each file's path, in a list.
    #[default]
    List,
    /// A tree of their folders.
    Tree,
}

impl FileLists {
    /// Whether it is a tree.
    pub(crate) const fn is_tree(self) -> bool {
        matches!(self, Self::Tree)
    }
}

/// How diffs and files show at first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FilePreferences {
    /// Whether a diff shows side by side.
    pub(crate) split_diffs: bool,
    /// Whether long lines wrap.
    pub(crate) wrap: bool,
    /// Cells between tab stops.
    pub(crate) tab_width: u8,
    /// How lists of changed files show.
    pub(crate) lists: FileLists,
    /// Whether the merge editor shows each side whole above the merged file.
    pub(crate) merge_panes: bool,
}

impl Default for FilePreferences {
    fn default() -> Self {
        Self {
            split_diffs: false,
            wrap: false,
            tab_width: 8,
            lists: FileLists::List,
            merge_panes: true,
        }
    }
}

/// How the user likes egit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
    /// The text's size, in percent of egit's own.
    pub(crate) zoom: u16,
    /// How often repositories are fetched from in the background.
    pub(crate) fetch: FetchEvery,
    /// Whether the sidebar shows when a repository opens.
    pub(crate) sidebar: bool,
    /// How diffs and files show at first.
    pub(crate) files: FilePreferences,
    /// How commit dates show.
    pub(crate) dates: Dates,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            zoom: 100,
            fetch: FetchEvery::default(),
            sidebar: true,
            files: FilePreferences::default(),
            dates: Dates::default(),
        }
    }
}

/// The tab width text is measured with (shared with jobs).
static TAB_WIDTH: AtomicU8 = AtomicU8::new(8);

impl Preferences {
    /// The preferences in effect for this frame (the defaults until egit sets them).
    pub(crate) fn of(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(egui::Id::new("egit-preferences")))
            .unwrap_or_default()
    }

    /// Puts them in effect: the text's size, and what views and jobs read.
    pub(crate) fn apply(self, ctx: &egui::Context) {
        let zoom = self.zoom.clamp(*ZOOM.start(), *ZOOM.end());
        ctx.set_zoom_factor(f32::from(zoom) / 100.0);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("egit-preferences"), self));
        let width = if TAB_WIDTHS.contains(&self.files.tab_width) {
            self.files.tab_width
        } else {
            8
        };
        TAB_WIDTH.store(width, Ordering::Relaxed);
    }

    /// The text's size as egui has it now (the keyboard's ⌘+ and ⌘− change it), in percent.
    pub(crate) fn zoom_of(ctx: &egui::Context) -> u16 {
        let percent = (ctx.zoom_factor() * 100.0).round();
        // Within the range offered: a value egui can't hold is the default.
        ZOOM.clone()
            .find(|zoom| (f32::from(*zoom) - percent).abs() < 0.5)
            .unwrap_or(100)
    }
}

/// How text is measured into cells: tab stops as preferred, a `\r` ending a line hidden.
pub(crate) fn text_options() -> egit_text::Options {
    egit_text::Options {
        tab_width: TAB_WIDTH.load(Ordering::Relaxed),
        ..egit_text::Options::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_read_as_the_defaults() {
        let read: Preferences = serde_json::from_str(r#"{"zoom": 120}"#).unwrap_or_default();
        assert_eq!(read.zoom, 120);
        assert_eq!(read.fetch, FetchEvery::FiveMinutes);
        assert!(read.sidebar);
        assert_eq!(read.files.tab_width, 8);
    }

    #[test]
    fn fetching_never_has_no_interval() {
        assert_eq!(FetchEvery::Never.interval(), None);
        assert_eq!(
            FetchEvery::QuarterHour.interval(),
            Some(Duration::from_mins(15))
        );
    }
}
