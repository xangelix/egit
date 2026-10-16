//! The working tree's changes beside the history: what isn't staged and what is, each list with
//! a button to move all of it and each file with its own, a file opened in the file view.
//!
//! The lists are made once per status read (indices into it, so a tree with a hundred thousand
//! untracked files costs a few hundred kilobytes), and drawn by egui-table-kit, only the rows on
//! screen.

use std::borrow::Cow;
use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_git::{Change, Direction, EntryKind, Status, StatusEntry, Uncommitted};
use egui::{Align, Label, Layout, Response, RichText, Ui, WidgetInfo, WidgetType};
use egui_async::Bind;
use egui_table_kit::error::TableError;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::operations::{HeaderIter, Row, RowCallback, TableCell, TableProvider};
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use super::file::WorkFile;
use crate::format::count;
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::staging::StageRequest;
use crate::tables::elided_start;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// What the user asked of the panel.
#[derive(Debug)]
pub(super) enum WorkEvent {
    /// Look at a file's changes.
    Open(WorkFile),
    /// Stage or unstage.
    Stage(StageRequest),
}

/// The panel's state between frames.
#[derive(Debug)]
pub(super) struct WorkPanel {
    unstaged: List,
    staged: List,
    /// The status the lists were made from.
    made_from: Option<Arc<Status>>,
}

/// One of the lists: its entries (indices into the status) and its table's state.
#[derive(Debug)]
struct List {
    entries: Vec<u32>,
    state: TableState,
}

impl List {
    fn new(id: String) -> Self {
        Self {
            entries: Vec::new(),
            state: TableState::new(id, 0),
        }
    }

    /// Takes `entries`, keeping the table's place (its scrolling is egui's, by its id).
    fn set(&mut self, entries: Vec<u32>) {
        self.state.active_rows = (0..entries.len()).collect();
        self.state.selected_rows.clear();
        self.entries = entries;
    }
}

impl WorkPanel {
    pub(super) fn new(repo: RepoId) -> Self {
        Self {
            unstaged: List::new(format!("work-unstaged-{repo}")),
            staged: List::new(format!("work-staged-{repo}")),
            made_from: None,
        }
    }

    /// Draws the lists for `status`, with `staging`'s problem if the last one failed; returns
    /// what the user asked for.
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        status: &Arc<Status>,
        staging: &mut Bind<(), Problem>,
    ) -> Option<WorkEvent> {
        if !self
            .made_from
            .as_ref()
            .is_some_and(|made| Arc::ptr_eq(made, status))
        {
            let (mut unstaged, mut staged) = (Vec::new(), Vec::new());
            for (index, entry) in (0_u32..).zip(status.entries()) {
                if entry.is_unstaged() {
                    unstaged.push(index);
                }
                if entry.is_staged() {
                    staged.push(index);
                }
            }
            self.unstaged.set(unstaged);
            self.staged.set(staged);
            self.made_from = Some(Arc::clone(status));
        }
        let mut event = None;
        if let Some(problem) = staging.err_ref() {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
            ui.add_space(8.0);
        }
        let busy = staging.is_pending();
        let height = (ui.available_height() - 80.0).max(120.0) / 2.0;
        for (list, side) in [
            (&mut self.unstaged, Side::Unstaged),
            (&mut self.staged, Side::Staged),
        ] {
            // Each table's scroll area is named by where it is drawn: apart, then.
            let found = ui
                .push_id(side, |ui| list_ui(ui, status, list, side, busy, height))
                .inner;
            if found.is_some() {
                event = found;
            }
            ui.add_space(12.0);
        }
        event
    }
}

/// Which list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Side {
    Unstaged,
    Staged,
}

impl Side {
    const fn direction(self) -> Direction {
        match self {
            Self::Unstaged => Direction::Stage,
            Self::Staged => Direction::Unstage,
        }
    }

    /// Which changes of `entry` the list shows.
    fn which(self, entry: StatusEntry<'_>) -> Uncommitted {
        match self {
            Self::Staged => Uncommitted::Staged,
            Self::Unstaged if entry.kind == EntryKind::Untracked => Uncommitted::Untracked,
            Self::Unstaged => Uncommitted::Unstaged,
        }
    }
}

/// How `entry` changed on the side `which` shows, in a word or two.
pub(super) fn change_name(entry: StatusEntry<'_>, which: Uncommitted) -> &'static str {
    match entry.kind {
        EntryKind::Untracked => return "Untracked",
        EntryKind::Unmerged => return "Conflict",
        EntryKind::Changed | EntryKind::Renamed | EntryKind::Copied => {}
    }
    let change = match which {
        Uncommitted::Staged => entry.staged(),
        Uncommitted::Unstaged | Uncommitted::Untracked => entry.unstaged(),
    };
    match change {
        Change::Unmodified => "Unchanged",
        Change::Modified => "Modified",
        Change::TypeChanged => "Type changed",
        Change::Added => "Added",
        Change::Deleted => "Deleted",
        Change::Renamed => "Renamed",
        Change::Copied => "Copied",
        Change::Unmerged => "Conflict",
    }
}

/// The paths staging or unstaging `entry` on `side` names (a staged rename's both).
fn paths(entry: StatusEntry<'_>, side: Side) -> Vec<BString> {
    let from = (side == Side::Staged).then_some(entry.from).flatten();
    from.into_iter()
        .chain(std::iter::once(entry.path))
        .map(ToOwned::to_owned)
        .collect()
}

/// One list: its heading and its button for all, and its table.
fn list_ui(
    ui: &mut Ui,
    status: &Status,
    list: &mut List,
    side: Side,
    busy: bool,
    height: f32,
) -> Option<WorkEvent> {
    let mut event = None;
    let (title, all) = match side {
        Side::Unstaged => ("Not staged", "Stage all"),
        Side::Staged => ("Staged", "Unstage all"),
    };
    ui.horizontal(|ui| {
        widgets::heading(ui, title);
        ui.label(muted(
            ui,
            count(u64::try_from(list.entries.len()).unwrap_or(u64::MAX)),
        ));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::secondary(ui, !busy && !list.entries.is_empty(), all).clicked() {
                let paths = list
                    .entries
                    .iter()
                    .filter_map(|&index| status.entry(usize::try_from(index).ok()?))
                    .flat_map(|entry| paths(entry, side))
                    .collect();
                event = Some(WorkEvent::Stage(StageRequest::Files {
                    paths,
                    direction: side.direction(),
                }));
            }
        });
    });
    if list.entries.is_empty() {
        ui.label(muted(
            ui,
            match side {
                Side::Unstaged => "Nothing to stage.",
                Side::Staged => "Nothing staged yet.",
            },
        ));
        return event;
    }
    let provider = Entries {
        status,
        indices: &list.entries,
        side,
    };
    let width = ui.available_width();
    let columns = vec![
        egui_table_kit::layout::Column::new(90.0).range(70.0..=160.0),
        egui_table_kit::layout::Column::new((width - 90.0).max(120.0))
            .range(100.0..=f32::INFINITY)
            .remainder(true),
    ];
    let palette = Palette::of(ui.ctx());
    let mut button_event = None;
    let table_id = list.state.id.to_string();
    let shown = TableKit::new(table_id.as_str(), &provider, &mut list.state)
        .with_columns(columns)
        .with_row_height(22.0)
        .with_max_height(Some(height))
        .with_striped(true)
        .show_with_output(ui, |ui, cell, _, text_color| {
            if cell.col_nr != FILE_COLUMN {
                return None;
            }
            let entry = provider.entry(usize::try_from(cell.row_nr).ok()?)?;
            let (response, clicked) = file_cell(ui, entry, side, busy, text_color);
            if clicked {
                button_event = Some(WorkEvent::Stage(StageRequest::Files {
                    paths: paths(entry, side),
                    direction: side.direction(),
                }));
            }
            Some(response)
        });
    match shown {
        Ok(output) => {
            if let Some(clicked) = output
                .events
                .iter()
                .find(|event| matches!(event.kind, RowEventKind::Clicked | RowEventKind::Activated))
                && let Some(entry) = provider.entry(clicked.row_index)
            {
                event = Some(WorkEvent::Open(WorkFile::of(entry, side.which(entry))));
            }
        }
        Err(error) => {
            ui.label(
                RichText::new(format!("The list couldn't be drawn: {error}")).color(palette.danger),
            );
        }
    }
    button_event.or(event)
}

/// A file's cell: its button to stage or unstage it on the right, its path (elided from the
/// start) in what is left; returns the cell and whether the button was clicked.
fn file_cell(
    ui: &mut Ui,
    entry: StatusEntry<'_>,
    side: Side,
    busy: bool,
    text_color: egui::Color32,
) -> (Response, bool) {
    let path = entry.path.to_str_lossy();
    let (glyph, verb) = match side {
        Side::Unstaged => (icon::PLUS, "Stage"),
        Side::Staged => (icon::MINUS, "Unstage"),
    };
    let mut clicked = false;
    let room = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let response = ui
        .allocate_ui_with_layout(room, Layout::right_to_left(Align::Center), |ui| {
            let button = ui
                .add_enabled(
                    busy.then_some(false).is_none(),
                    egui::Button::new(glyph).small(),
                )
                .on_hover_text(verb);
            // Named for what it does to which file: a screen reader hears more than "+".
            let name = format!("{verb} {path}");
            button.widget_info(|| WidgetInfo::labeled(WidgetType::Button, !busy, &name));
            clicked = button.clicked();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let shown = elided_start(ui, &path, &font, ui.available_width());
                let elided = matches!(shown, Cow::Owned(_));
                let label = ui.add(
                    Label::new(RichText::new(shown.into_owned()).color(text_color))
                        .selectable(false)
                        .truncate(),
                );
                if elided {
                    label.on_hover_text(path.as_ref());
                }
            });
        })
        .response;
    (response, clicked)
}

/// The column with a file's path and button.
const FILE_COLUMN: usize = 1;

/// The columns of a list.
const HEADERS: [&str; 2] = ["Change", "File"];

/// A list's rows, for egui-table-kit.
struct Entries<'a> {
    status: &'a Status,
    indices: &'a [u32],
    side: Side,
}

impl<'a> Entries<'a> {
    fn entry(&self, row: usize) -> Option<StatusEntry<'a>> {
        let index = usize::try_from(*self.indices.get(row)?).ok()?;
        self.status.entry(index)
    }

    fn cell(&self, entry: StatusEntry<'a>, col_index: usize) -> Option<TableCell<'a>> {
        let text = match col_index {
            0 => Cow::Borrowed(change_name(entry, self.side.which(entry))),
            FILE_COLUMN => match (self.side, entry.from) {
                (Side::Staged, Some(from)) => Cow::Owned(format!(
                    "{} → {}",
                    from.to_str_lossy(),
                    entry.path.to_str_lossy()
                )),
                _ => entry.path.to_str_lossy(),
            },
            _ => return None,
        };
        Some((text, None))
    }
}

/// A row of a list.
struct EntryRow<'a>(&'a Entries<'a>, StatusEntry<'a>);

impl Row for EntryRow<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        self.0.cell(self.1, col_index)
    }

    fn column_count(&self) -> usize {
        HEADERS.len()
    }
}

impl TableProvider for Entries<'_> {
    fn column_count(&self) -> usize {
        HEADERS.len()
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        HEADERS.get(index).map(|&name| Cow::Borrowed(name))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.indices.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok(self
            .entry(row_index)
            .and_then(|entry| self.cell(entry, col_index)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.entry(index)
            .map_or(Ok(()), |entry| f(&EntryRow(self, entry)))
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
        for row in 0..self.indices.len() {
            self.for_row_at(row, f)?;
        }
        Ok(())
    }
}
