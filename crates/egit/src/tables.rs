//! What egit's tables show, as `egui-table-kit` providers: configuration entries and recent
//! repositories. Cells are produced on demand for the rows drawn, borrowing where the data is
//! already text.

use std::borrow::Cow;
use std::path::Path;

use bstr::ByteSlice as _;
use egit_git::{ConfigEntry, ConfigSnapshot};
use egui_table_kit::error::TableError;
use egui_table_kit::operations::{HeaderIter, Row, RowCallback, TableCell, TableProvider};
use egui_table_kit::state::TableState;

use crate::format::{ago, display_path};
use crate::settings::Recent;
use crate::widgets::scope_name;

/// A table of a configuration snapshot: its state starts over when the snapshot is replaced
/// (the rows are others).
pub(crate) struct SnapshotTable {
    id: String,
    shown: Option<std::sync::Arc<ConfigSnapshot>>,
    pub(crate) state: TableState,
}

impl SnapshotTable {
    pub(crate) fn new(id: String) -> Self {
        let state = TableState::new(id.as_str(), 0);
        Self {
            id,
            shown: None,
            state,
        }
    }

    /// Makes the table show `snapshot`.
    pub(crate) fn sync(&mut self, snapshot: &std::sync::Arc<ConfigSnapshot>) {
        let same = self
            .shown
            .as_ref()
            .is_some_and(|shown| std::sync::Arc::ptr_eq(shown, snapshot));
        if !same {
            self.state = TableState::new(self.id.as_str(), snapshot.len());
            self.shown = Some(std::sync::Arc::clone(snapshot));
        }
    }
}

/// Every entry of a configuration snapshot: key, value, scope, and file.
pub(crate) struct ConfigRows<'a> {
    snapshot: &'a ConfigSnapshot,
    home: Option<&'a Path>,
}

impl<'a> ConfigRows<'a> {
    pub(crate) const fn new(snapshot: &'a ConfigSnapshot, home: Option<&'a Path>) -> Self {
        Self { snapshot, home }
    }
}

/// The columns of [`ConfigRows`].
const CONFIG_HEADERS: [&str; 4] = ["Key", "Value", "Where", "File"];

struct ConfigRow<'a> {
    entry: ConfigEntry<'a>,
    home: Option<&'a Path>,
}

impl Row for ConfigRow<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        config_cell(self.entry, self.home, col_index)
    }

    fn column_count(&self) -> usize {
        CONFIG_HEADERS.len()
    }
}

fn config_cell<'e>(
    entry: ConfigEntry<'e>,
    home: Option<&Path>,
    col_index: usize,
) -> Option<TableCell<'e>> {
    let text = match col_index {
        0 => entry.key.to_str_lossy(),
        1 => entry
            .value
            .map_or(Cow::Borrowed("(true)"), |value| value.to_str_lossy()),
        2 => Cow::Borrowed(scope_name(entry.scope)),
        3 => Cow::Owned(entry.file().map_or_else(
            || entry.origin.to_str_lossy().into_owned(),
            |file| display_path(&file, home),
        )),
        _ => return None,
    };
    Some((text, None))
}

impl TableProvider for ConfigRows<'_> {
    fn column_count(&self) -> usize {
        CONFIG_HEADERS.len()
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        CONFIG_HEADERS.get(index).map(|&name| Cow::Borrowed(name))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.snapshot.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok(self
            .snapshot
            .entry(row_index)
            .and_then(|entry| config_cell(entry, self.home, col_index)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.snapshot.entry(index).map_or(Ok(()), |entry| {
            f(&ConfigRow {
                entry,
                home: self.home,
            })
        })
    }

    fn for_selected_rows(
        &self,
        state: &TableState,
        f: &mut RowCallback<'_>,
    ) -> Result<(), TableError> {
        for index in &state.selected_rows {
            self.for_row_at(usize::try_from(index).unwrap_or(usize::MAX), f)?;
        }
        Ok(())
    }

    fn for_all_rows(&self, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        for entry in self.snapshot.entries() {
            f(&ConfigRow {
                entry,
                home: self.home,
            })?;
        }
        Ok(())
    }
}

/// The recent repositories: name, location, and when each was last opened.
pub(crate) struct RecentRows<'a> {
    recent: &'a [Recent],
    home: Option<&'a Path>,
    now: u64,
}

impl<'a> RecentRows<'a> {
    pub(crate) const fn new(recent: &'a [Recent], home: Option<&'a Path>, now: u64) -> Self {
        Self { recent, home, now }
    }
}

/// The columns of [`RecentRows`].
const RECENT_HEADERS: [&str; 3] = ["Repository", "Location", "Opened"];

struct RecentRow<'a> {
    recent: &'a Recent,
    home: Option<&'a Path>,
    now: u64,
}

impl Row for RecentRow<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        recent_cell(self.recent, self.home, self.now, col_index)
    }

    fn column_count(&self) -> usize {
        RECENT_HEADERS.len()
    }
}

fn recent_cell(
    recent: &Recent,
    home: Option<&Path>,
    now: u64,
    col_index: usize,
) -> Option<TableCell<'static>> {
    let path = recent.path.to_path();
    let text = match col_index {
        0 => path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        1 => path
            .parent()
            .map_or_else(String::new, |parent| display_path(parent, home)),
        2 => ago(recent.opened, now),
        _ => return None,
    };
    Some((Cow::Owned(text), None))
}

impl TableProvider for RecentRows<'_> {
    fn column_count(&self) -> usize {
        RECENT_HEADERS.len()
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        RECENT_HEADERS.get(index).map(|&name| Cow::Borrowed(name))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.recent.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok(self
            .recent
            .get(row_index)
            .and_then(|recent| recent_cell(recent, self.home, self.now, col_index)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.recent.get(index).map_or(Ok(()), |recent| {
            f(&RecentRow {
                recent,
                home: self.home,
                now: self.now,
            })
        })
    }

    fn for_selected_rows(
        &self,
        state: &TableState,
        f: &mut RowCallback<'_>,
    ) -> Result<(), TableError> {
        for index in &state.selected_rows {
            self.for_row_at(usize::try_from(index).unwrap_or(usize::MAX), f)?;
        }
        Ok(())
    }

    fn for_all_rows(&self, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        for recent in self.recent {
            f(&RecentRow {
                recent,
                home: self.home,
                now: self.now,
            })?;
        }
        Ok(())
    }
}
