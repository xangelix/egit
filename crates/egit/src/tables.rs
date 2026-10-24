//! What egit's tables show, as `egui-table-kit` providers: configuration entries, recent
//! repositories, and the files a commit changed (in a list, or a tree of their folders:
//! `tree_table`). Cells are produced on demand for the rows drawn, borrowing where the data is
//! already text.

use std::borrow::Cow;
use std::path::Path;

use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{ChangeKind, ConfigEntry, ConfigSnapshot, FileChange, LineCounts, ObjectId};
use egui::{Align, Color32, Label, Layout, Response, RichText, Ui};
use egui_table_kit::error::TableError;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::operations::{HeaderIter, Row, RowCallback, TableCell, TableProvider};
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use crate::format::{ago, count, display_path};
use crate::history::{ChangeCounts, CommitDetails};
use crate::preferences::Preferences;
use crate::settings::Recent;
use crate::theme::Palette;
use crate::tree_table::{TreeMenu, TreeRow, TreeSource, TreeTable};
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

/// The repositories suggested: name, location, and when each was last active.
pub(crate) struct SuggestedRows<'a> {
    suggested: &'a [crate::suggested::Suggestion],
    home: Option<&'a Path>,
    now: u64,
}

impl<'a> SuggestedRows<'a> {
    pub(crate) const fn new(
        suggested: &'a [crate::suggested::Suggestion],
        home: Option<&'a Path>,
        now: u64,
    ) -> Self {
        Self {
            suggested,
            home,
            now,
        }
    }

    fn row(&self, at: usize) -> Option<SuggestedRow<'_>> {
        self.suggested.get(at).map(|suggestion| SuggestedRow {
            suggestion,
            home: self.home,
            now: self.now,
        })
    }
}

/// The columns of [`SuggestedRows`].
const SUGGESTED_HEADERS: [&str; 3] = ["Repository", "Location", "Active"];

struct SuggestedRow<'a> {
    suggestion: &'a crate::suggested::Suggestion,
    home: Option<&'a Path>,
    now: u64,
}

impl SuggestedRow<'_> {
    fn text(&self, col_index: usize) -> Option<String> {
        let path = &self.suggestion.path;
        Some(match col_index {
            0 => path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            1 => path
                .parent()
                .map_or_else(String::new, |parent| display_path(parent, self.home)),
            2 => self
                .suggestion
                .active
                .and_then(|active| u64::try_from(active).ok())
                .map_or_else(|| "never".to_owned(), |active| ago(active, self.now)),
            _ => return None,
        })
    }
}

impl Row for SuggestedRow<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        self.text(col_index).map(|text| (Cow::Owned(text), None))
    }

    fn column_count(&self) -> usize {
        SUGGESTED_HEADERS.len()
    }
}

impl TableProvider for SuggestedRows<'_> {
    fn column_count(&self) -> usize {
        SUGGESTED_HEADERS.len()
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        SUGGESTED_HEADERS
            .get(index)
            .map(|&name| Cow::Borrowed(name))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.suggested.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok(self
            .row(row_index)
            .and_then(|row| row.text(col_index))
            .map(|text| (Cow::Owned(text), None)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.row(index).map_or(Ok(()), |row| f(&row))
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
        for index in 0..self.suggested.len() {
            self.for_row_at(index, f)?;
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
        // On another computer: which, then where there.
        1 => recent.host.as_ref().map_or_else(
            || {
                path.parent()
                    .map_or_else(String::new, |parent| display_path(parent, home))
            },
            |host| {
                path.parent().map_or_else(
                    || host.clone(),
                    |parent| format!("{host}: {}", parent.display()),
                )
            },
        ),
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

/// The table of a commit's changed files: its state starts over for another commit.
#[derive(Debug)]
pub(crate) struct ChangesTable {
    id: String,
    shown: Option<ObjectId>,
    state: TableState,
    /// The same files as a tree of their folders, when the preferences ask for one.
    tree: TreeTable<CommitDetails>,
    /// A row right-clicked: its menu, while open, and whether it opens this frame.
    menu: Option<TreeMenu>,
    open_menu: bool,
}

/// What a changed file's row asks: its entry, and what of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeAsk {
    /// Its changes, in the file view (a click).
    Show(usize),
    /// Its blame as the commit left it (or, deleted, as it was before).
    Blame(usize),
    /// The commits that changed it.
    History(usize),
    /// The work tree's file (as it is now), opened in its app or shown in its folder.
    Reveal(usize, crate::desktop::Reveal),
}

impl TreeSource for CommitDetails {
    fn files(&self) -> usize {
        self.changes.len()
    }

    fn path(&self, entry: usize) -> &[u8] {
        self.changes
            .get(entry)
            .map_or(&[][..], |change| change.path.as_slice())
    }

    fn change(&self, entry: usize) -> &'static str {
        self.changes
            .get(entry)
            .map_or("Changed", |change| change_name(&change.kind))
    }
}

impl ChangesTable {
    pub(crate) fn new(id: String) -> Self {
        let state = TableState::new(id.as_str(), 0);
        let tree = TreeTable::new(format!("{id}-tree"));
        Self {
            id,
            shown: None,
            state,
            tree,
            menu: None,
            open_menu: false,
        }
    }

    /// Shows the files `details`' commit changed, and the lines each gained and lost once
    /// `counts` has them, `height` tall at most; returns what a click or a row's menu asked
    /// (the work tree's file revealed only where it is `here`).
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        details: &Arc<CommitDetails>,
        counts: Option<&ChangeCounts>,
        (height, here): (f32, bool),
    ) -> Option<ChangeAsk> {
        if self.shown != Some(details.id) {
            self.state = TableState::new(self.id.as_str(), details.changes.len());
            self.shown = Some(details.id);
            self.menu = None;
        }
        let clicked = self.rows(ui, details, counts, height);
        let asked = self.row_menu(ui, details, here);
        clicked.map(ChangeAsk::Show).or(asked)
    }

    /// The rows, as a list or a tree; returns the one clicked, and notes one right-clicked.
    fn rows(
        &mut self,
        ui: &mut Ui,
        details: &Arc<CommitDetails>,
        counts: Option<&ChangeCounts>,
        height: f32,
    ) -> Option<usize> {
        let counts = counts
            .filter(|counts| counts.id == details.id)
            .map(|counts| counts.files.as_slice());
        if Preferences::of(ui.ctx()).files.lists.is_tree() {
            let palette = Palette::of(ui.ctx());
            let shown = self.tree.show(ui, details, height, |ui, _, row, _| {
                if let TreeRow::File(entry) = row {
                    lines_ui(ui, lines_of(counts, entry), palette);
                }
                None::<()>
            });
            if let Some(menu) = shown.menu {
                self.menu = Some(menu);
                self.open_menu = true;
            }
            return shown.opened;
        }
        let provider = ChangeRows {
            changes: &details.changes,
        };
        let palette = Palette::of(ui.ctx());
        let state = &mut self.state;
        let shown = list(TableKit::new(self.id.as_str(), &provider, state), ui)
            .with_max_height(Some(height))
            .show_with_output(ui, |ui, cell, row, text_color| {
                if cell.col_nr != FILE_COLUMN {
                    return status_cell(ui, row, palette);
                }
                let row = usize::try_from(cell.row_nr).ok()?;
                let change = details.changes.get(row)?;
                let lines = lines_of(counts, row);
                Some(file_cell(ui, change, lines, text_color, palette))
            });
        match shown {
            Ok(output) => {
                if let Some(right) = output
                    .events
                    .iter()
                    .find(|event| event.kind == RowEventKind::SecondaryClicked)
                {
                    self.menu = Some(TreeMenu::File(right.row_index));
                    self.open_menu = true;
                }
                output
                    .events
                    .iter()
                    .find(|event| {
                        matches!(event.kind, RowEventKind::Clicked | RowEventKind::Activated)
                    })
                    .map(|event| event.row_index)
            }
            Err(error) => {
                ui.label(
                    RichText::new(format!("The table couldn't be drawn: {error}"))
                        .color(Palette::of(ui.ctx()).danger),
                );
                None
            }
        }
    }

    /// A row's menu, while one is open: a file's changes, blame, and history, the work tree's
    /// file opened or shown, its path copied; a folder's path copied.
    fn row_menu(&mut self, ui: &Ui, details: &CommitDetails, here: bool) -> Option<ChangeAsk> {
        use crate::views::actions::item;
        let menu = self.menu.as_ref()?;
        let open = std::mem::take(&mut self.open_menu);
        let mut asked = None;
        let shown = egui::Popup::new(
            ui.id().with((&self.id, "row-menu")),
            ui.ctx().clone(),
            egui::PopupAnchor::PointerFixed,
            ui.layer_id(),
        )
        .kind(egui::PopupKind::Menu)
        .layout(Layout::top_down_justified(Align::Min))
        .style(egui::containers::menu::menu_style)
        .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
        .show(|ui| {
            ui.set_min_width(220.0);
            let (path, entry) = match menu {
                TreeMenu::File(entry) => match details.changes.get(*entry) {
                    Some(change) => (change.path.as_slice(), Some(*entry)),
                    None => return,
                },
                TreeMenu::Folder { path, .. } => (path.as_slice(), None),
            };
            if let Some(entry) = entry {
                asked = file_items(ui, entry, here);
                ui.separator();
            }
            if item(ui, (crate::theme::icon::COPY, "Copy path"), Ok(())) {
                ui.ctx().copy_text(path.to_str_lossy().into_owned());
            }
        });
        if shown.is_none() {
            self.menu = None;
        }
        asked
    }
}

/// A changed file's menu's own items: its changes, blame, and history; the work tree's file
/// opened or shown in its folder (only `here`: its apps and file manager are this computer's).
fn file_items(ui: &mut Ui, entry: usize, here: bool) -> Option<ChangeAsk> {
    use crate::desktop::Reveal;
    use crate::theme::icon;
    use crate::views::actions::item;
    let mut asked = None;
    for (glyph, words, ask) in [
        (icon::FILE_TEXT, "Show changes", ChangeAsk::Show(entry)),
        (icon::USER_LIST, "Blame", ChangeAsk::Blame(entry)),
        (
            icon::CLOCK_COUNTER_CLOCKWISE,
            "File history",
            ChangeAsk::History(entry),
        ),
    ] {
        if item(ui, (glyph, words), Ok(())) {
            asked = Some(ask);
        }
    }
    ui.separator();
    let revealing = if here {
        Ok(())
    } else {
        Err(crate::views::ELSEWHERE)
    };
    for (glyph, words, how) in [
        (icon::ARROW_SQUARE_OUT, "Open file", Reveal::Open),
        (icon::FOLDER_OPEN, "Show in folder", Reveal::InFolder),
    ] {
        if item(ui, (glyph, words), revealing) {
            asked = Some(ChangeAsk::Reveal(entry, how));
        }
    }
    asked
}

/// The row height of egit's tables.
pub(crate) const ROW_HEIGHT: f32 = 24.0;

/// Where a row's content starts in its cell: table-kit's cell padding.
pub(crate) const CELL_INSET: f32 = 8.0;

/// A list of files as egit draws them: a narrow column for how each changed (a coloured letter:
/// `status_cell`), its path filling the rest, no header (the letters and paths explain
/// themselves), and selected rows in the text's own colour, without table-kit's outline.
pub(crate) fn list<'a>(table: TableKit<'a>, ui: &Ui) -> TableKit<'a> {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    table
        .with_columns(vec![
            egui_table_kit::layout::Column::new(STATUS_WIDTH)
                .range(STATUS_WIDTH..=STATUS_WIDTH)
                .resizable(false),
            egui_table_kit::layout::Column::new((width - STATUS_WIDTH).max(120.0))
                .range(100.0..=f32::INFINITY)
                .remainder(true)
                .resizable(false),
        ])
        .with_header(false)
        .with_row_height(ROW_HEIGHT)
        .with_cell_align(Align::Center)
        .with_selected_hover_color(palette.selected_hover)
        .with_selected_text_color(palette.text)
        .with_focus_outline(false)
        // Inside a rounded box: the rows' corners follow its.
        .with_row_rounding(crate::theme::RADIUS_INNER)
}

/// A file list shown as a tree (`tree_table`): one column, the rest as `list`'s.
pub(crate) fn tree_list<'a>(table: TableKit<'a>, ui: &Ui) -> TableKit<'a> {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    table
        .with_columns(vec![
            egui_table_kit::layout::Column::new(width.max(120.0))
                .range(100.0..=f32::INFINITY)
                .remainder(true)
                .resizable(false),
        ])
        .with_header(false)
        .with_row_height(ROW_HEIGHT)
        .with_cell_align(Align::Center)
        .with_selected_hover_color(palette.selected_hover)
        .with_selected_text_color(palette.text)
        .with_focus_outline(false)
        .with_row_rounding(crate::theme::RADIUS_INNER)
}

/// Every table egit draws, but its file lists: centred text, selected rows in the text's own
/// colour, without table-kit's outline.
pub(crate) fn styled<'a>(table: TableKit<'a>, ui: &Ui) -> TableKit<'a> {
    let palette = Palette::of(ui.ctx());
    table
        .with_cell_align(Align::Center)
        .with_selected_hover_color(palette.selected_hover)
        .with_selected_text_color(palette.text)
        .with_focus_outline(false)
}

/// The width of a file list's status column.
const STATUS_WIDTH: f32 = 30.0;

/// How a file changed, as a letter on its colour's tint (the cell is still named by the word,
/// `row`'s first cell): A added, M modified, D deleted, R renamed, C copied, T its type, U
/// untracked, ! in conflict.
pub(crate) fn status_cell(ui: &mut Ui, row: &dyn Row, palette: Palette) -> Option<Response> {
    let (word, _) = row.cell(0)?;
    let size = ui.available_size();
    let response = ui
        .allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            status_mark(ui, &word, palette)
        })
        .response;
    Some(response)
}

/// How a file changed (`word`: "Added", "Modified", …) as a letter on its colour's tint, its
/// word on hover.
pub(crate) fn status_mark(ui: &mut Ui, word: &str, palette: Palette) -> Response {
    let (letter, color) = match word {
        "Added" => ("A", palette.ok),
        "Modified" => ("M", palette.warn),
        "Deleted" => ("D", palette.danger),
        "Renamed" => ("R", palette.accent),
        "Copied" => ("C", palette.accent),
        "Type changed" => ("T", palette.muted),
        "Untracked" => ("U", palette.ok),
        "Conflict" | "Unmerged" => ("!", palette.danger),
        _ => ("·", palette.muted),
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    crate::help::note_topic(ui, rect, "status-letters");
    ui.painter()
        .rect_filled(rect, crate::theme::RADIUS_INNER, color.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::monospace(11.5),
        color,
    );
    response.on_hover_text(word)
}

/// The files a commit changed: how, and which.
struct ChangeRows<'a> {
    changes: &'a [FileChange],
}

/// The columns of [`ChangeRows`].
const CHANGE_HEADERS: [&str; 2] = ["Change", "File"];

/// The column of [`ChangeRows`] with the file's path, and its lines gained and lost.
const FILE_COLUMN: usize = 1;

/// A file's cell: its lines gained and lost on the right, and its path (its new one, for a
/// rename or copy, the old one on hover) in what is left, losing its start first: the name
/// matters most. The cell is named by `change_cell`; these are only labels.
fn file_cell(
    ui: &mut Ui,
    change: &FileChange,
    lines: Lines,
    text_color: Color32,
    palette: Palette,
) -> Response {
    let label =
        |text: String, color| Label::new(RichText::new(text).color(color)).selectable(false);
    // Across the cell's height, as table-kit centers its own cells' text.
    let size = ui.available_size();
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        lines_ui(ui, lines, palette);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let path = change.path.to_str_lossy();
            let font = egui::TextStyle::Body.resolve(ui.style());
            let shown = elided_start(ui, &path, &font, ui.available_width());
            let elided = matches!(shown, Cow::Owned(_));
            let shown = ui.add(label(shown.into_owned(), text_color).truncate());
            if let ChangeKind::Renamed { from, .. } | ChangeKind::Copied { from, .. } = &change.kind
            {
                shown.on_hover_text(format!("{path}\nfrom {}", from.to_str_lossy()));
            } else if elided {
                shown.on_hover_text(path.as_ref());
            }
        });
    })
    .response
}

/// A file's lines gained and lost (from the right: lost, then gained), "binary", or nothing
/// while uncounted, in a right-to-left row.
fn lines_ui(ui: &mut Ui, lines: Lines, palette: Palette) {
    let label =
        |text: String, color| Label::new(RichText::new(text).color(color)).selectable(false);
    match lines {
        Lines::Counted(lines) => {
            ui.add(label(
                format!("−{}", count(u64::from(lines.removed))),
                palette.danger,
            ));
            ui.add(label(
                format!("+{}", count(u64::from(lines.added))),
                palette.ok,
            ));
        }
        Lines::Binary => {
            ui.add(label("binary".to_owned(), palette.muted));
        }
        Lines::Unknown => {}
    }
}

/// How wide `text` is in `font`, glyph by glyph (no kerning).
fn width_of(fonts: &mut egui::epaint::FontsView<'_>, text: &str, font: &egui::FontId) -> f32 {
    text.chars().map(|c| fonts.glyph_width(font, c)).sum()
}

/// `path` as fits in `width` points of `font`: whole, or its end after "…", from a folder's
/// boundary where one is in what fits.
pub(crate) fn elided_start<'p>(
    ui: &Ui,
    path: &'p str,
    font: &egui::FontId,
    width: f32,
) -> Cow<'p, str> {
    ui.fonts_mut(|fonts| {
        if width_of(fonts, path, font) <= width {
            return Cow::Borrowed(path);
        }
        let budget = width - fonts.glyph_width(font, '…');
        let mut used = 0.0;
        let mut start = path.len();
        for (index, c) in path.char_indices().rev() {
            used += fonts.glyph_width(font, c);
            if used > budget {
                break;
            }
            start = index;
        }
        let tail = path.get(start..).unwrap_or_default();
        // "…/d000/f000000.c" rather than "…0/d000/f000000.c".
        let tail = tail
            .find('/')
            .filter(|&slash| slash.saturating_add(1) < tail.len())
            .and_then(|slash| tail.get(slash..))
            .unwrap_or(tail);
        Cow::Owned(format!("…{tail}"))
    })
}

/// A changed file's lines, as far as they are known.
#[derive(Clone, Copy, Debug)]
enum Lines {
    /// Not counted (yet).
    Unknown,
    /// A binary file: no lines.
    Binary,
    Counted(LineCounts),
}

/// Change `row`'s lines.
fn lines_of(counts: Option<&[Option<LineCounts>]>, row: usize) -> Lines {
    match counts.and_then(|counts| counts.get(row)) {
        None => Lines::Unknown,
        Some(None) => Lines::Binary,
        Some(Some(lines)) => Lines::Counted(*lines),
    }
}

/// How a commit changed a file, in a word or two.
pub(crate) const fn change_name(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "Added",
        ChangeKind::Copied { .. } => "Copied",
        ChangeKind::Deleted => "Deleted",
        ChangeKind::Modified => "Modified",
        ChangeKind::Renamed { .. } => "Renamed",
        ChangeKind::TypeChanged => "Type changed",
        ChangeKind::Unmerged => "Unmerged",
        ChangeKind::Unknown => "Changed",
    }
}

fn change_cell(change: &FileChange, col_index: usize) -> Option<TableCell<'_>> {
    let text = match col_index {
        0 => Cow::Borrowed(change_name(&change.kind)),
        1 => match &change.kind {
            ChangeKind::Renamed { from, .. } | ChangeKind::Copied { from, .. } => Cow::Owned(
                format!("{} → {}", from.to_str_lossy(), change.path.to_str_lossy()),
            ),
            _ => change.path.to_str_lossy(),
        },
        _ => return None,
    };
    Some((text, None))
}

struct ChangeRow<'a>(&'a FileChange);

impl Row for ChangeRow<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        change_cell(self.0, col_index)
    }

    fn column_count(&self) -> usize {
        CHANGE_HEADERS.len()
    }
}

impl TableProvider for ChangeRows<'_> {
    fn column_count(&self) -> usize {
        CHANGE_HEADERS.len()
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        CHANGE_HEADERS.get(index).map(|&name| Cow::Borrowed(name))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.changes.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok(self
            .changes
            .get(row_index)
            .and_then(|change| change_cell(change, col_index)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.changes
            .get(index)
            .map_or(Ok(()), |change| f(&ChangeRow(change)))
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
        for change in self.changes {
            f(&ChangeRow(change))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `elided_start` on a real context's fonts, given room for `chars` digits.
    fn elided(path: &str, chars: f32) -> String {
        let ctx = egui::Context::default();
        let mut shown = String::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::TextStyle::Body.resolve(ui.style());
            let digit = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
            shown = elided_start(ui, path, &font, digit * chars).into_owned();
        });
        // No renderer takes the font atlas.
        output.textures_delta.clear();
        shown
    }

    #[test]
    fn a_path_too_long_loses_its_start_from_a_folder() {
        assert_eq!(elided("src/lib.rs", 40.0), "src/lib.rs");
        assert_eq!(elided("src/new/d000/f000000.c", 17.0), "…/d000/f000000.c");
        // Where not even the name fits, its end.
        let end = elided("src/new/d000/f000000.c", 6.0);
        assert!(
            end.starts_with('…') && end.ends_with("0.c") && !end.contains('/'),
            "{end}"
        );
    }
}
