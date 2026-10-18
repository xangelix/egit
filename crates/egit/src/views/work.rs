//! The working tree's changes beside the history: what isn't staged and what is, each list with
//! a button to move all of it and each file with its own, a file opened in the file view; and
//! the form that commits what is staged.
//!
//! The lists are made once per status read (indices into it, so a tree with a hundred thousand
//! untracked files costs a few hundred kilobytes), and drawn by egui-table-kit, only the rows on
//! screen.

use std::borrow::Cow;
use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_git::{
    Change, Committed, Direction, EntryKind, Git, NewCommit, ObjectId, Repository, Status,
    StatusEntry, Uncommitted,
};
use egui::{
    Align, Key, Label, Layout, Modifiers, Response, RichText, TextEdit, Ui, WidgetInfo, WidgetType,
};
use egui_async::Bind;
use egui_table_kit::error::TableError;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::operations::{HeaderIter, Row, RowCallback, TableCell, TableProvider};
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use super::file::WorkFile;
use crate::format::count;
use crate::history::{Objects, read_message};
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
    /// Commit what is staged.
    Commit(NewCommit),
}

/// What the commit form needs of the repository.
pub(super) struct CommitSide<'a> {
    /// The last commit made, or being made.
    pub(super) committing: &'a mut Bind<Committed, Problem>,
    /// How many commits egit has made here (the form clears when it grows).
    pub(super) commits_made: u64,
    /// Whether commits are signed unless asked otherwise (`commit.gpgSign`).
    pub(super) sign_by_default: bool,
    /// `HEAD`'s commit, to amend (none before the first commit).
    pub(super) head: Option<ObjectId>,
    pub(super) git: Option<&'a Git>,
    pub(super) repository: &'a Arc<Repository>,
    pub(super) objects: &'a Objects,
}

/// The commit form's state.
#[derive(Debug)]
struct CommitForm {
    summary: String,
    description: String,
    amend: bool,
    sign_off: bool,
    /// Sign or not, once the user chose; until then, the configuration's default.
    sign: Option<bool>,
    /// How many commits had been made when the form was last cleared.
    made_seen: u64,
    /// The last commit's message, read to amend it.
    amended: Bind<String, Problem>,
}

impl CommitForm {
    const fn new() -> Self {
        Self {
            summary: String::new(),
            description: String::new(),
            amend: false,
            sign_off: false,
            sign: None,
            made_seen: 0,
            amended: Bind::new(true),
        }
    }

    /// The message: the summary, and the description after a blank line.
    fn message(&self) -> String {
        let mut message = self.summary.trim().to_owned();
        let description = self.description.trim_end();
        if !description.trim().is_empty() {
            message.push_str("\n\n");
            message.push_str(description);
        }
        message
    }

    fn is_blank(&self) -> bool {
        self.summary.trim().is_empty() && self.description.trim().is_empty()
    }
}

/// The panel's state between frames.
#[derive(Debug)]
pub(super) struct WorkPanel {
    unstaged: List,
    staged: List,
    /// The status the lists were made from.
    made_from: Option<Arc<Status>>,
    form: CommitForm,
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
            form: CommitForm::new(),
        }
    }

    /// Draws the lists for `status` (with `staging`'s problem if the last one failed) and the
    /// commit form; returns what the user asked for.
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        status: &Arc<Status>,
        staging: &mut Bind<(), Problem>,
        commit: CommitSide<'_>,
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
        let busy = staging.is_pending() || commit.committing.is_pending();
        // The form below keeps the room it needs; the lists share the rest.
        let height = (ui.available_height() - 380.0).max(160.0) / 2.0;
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
        let staged = self.staged.entries.len();
        let mut commit = commit;
        if let Some(made) = commit_form(ui, &mut self.form, &mut commit, staged, busy) {
            event = Some(WorkEvent::Commit(made));
        }
        event
    }
}

/// The form committing what is staged: a summary, a description, amending, signing off,
/// signing; then what the last commit printed, or why it wasn't made.
fn commit_form(
    ui: &mut Ui,
    form: &mut CommitForm,
    side: &mut CommitSide<'_>,
    staged: usize,
    busy: bool,
) -> Option<NewCommit> {
    if side.commits_made != form.made_seen {
        *form = CommitForm {
            made_seen: side.commits_made,
            ..CommitForm::new()
        };
    }
    // Amending a blank form starts from the last commit's message.
    if form.amend
        && form.is_blank()
        && let Some(Ok(message)) = form.amended.read_as_ref()
    {
        let message = message.trim_end();
        let (summary, description) = message.split_once("\n\n").unwrap_or((message, ""));
        form.summary = summary.to_owned();
        form.description = description.to_owned();
    }
    ui.separator();
    widgets::heading(ui, "Commit");
    ui.add_space(4.0);
    let description = message_fields(ui, form);
    options(ui, form, side);
    let ready = !busy && !form.summary.trim().is_empty() && (staged > 0 || form.amend);
    let label = if form.amend {
        "Amend the last commit".to_owned()
    } else if staged == 1 {
        "Commit 1 file".to_owned()
    } else {
        format!(
            "Commit {} files",
            count(u64::try_from(staged).unwrap_or(u64::MAX))
        )
    };
    let shortcut = description.has_focus()
        && ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Enter));
    let mut made = None;
    ui.horizontal(|ui| {
        if widgets::primary(ui, ready, &label).clicked() || (shortcut && ready) {
            made = Some(NewCommit {
                message: form.message(),
                amend: form.amend,
                sign_off: form.sign_off,
                sign: Some(form.sign.unwrap_or(side.sign_by_default)),
            });
        }
        if side.committing.is_pending() {
            ui.spinner();
            ui.label(muted(ui, "Committing…"));
        }
    });
    outcome(ui, side.committing);
    made
}

/// The summary and the description; returns the description's field (⌘Enter there commits).
fn message_fields(ui: &mut Ui, form: &mut CommitForm) -> Response {
    let summary_label = ui.label(muted(ui, "Summary"));
    ui.add(
        crate::widgets::line_field(&mut form.summary)
            .hint_text("What it does, in a line")
            .desired_width(f32::INFINITY),
    )
    .labelled_by(summary_label.id);
    let length = form.summary.trim().chars().count();
    if length > 72 {
        ui.label(
            RichText::new(format!(
                "{length} characters: a summary over 72 is cut short in many places"
            ))
            .small()
            .color(Palette::of(ui.ctx()).warn),
        );
    }
    let description_label = ui.label(muted(ui, "Description"));
    ui.add(
        TextEdit::multiline(&mut form.description)
            .hint_text("Why, and anything else worth knowing")
            .desired_rows(4)
            .desired_width(f32::INFINITY),
    )
    .labelled_by(description_label.id)
}

/// Amending (which reads the last commit's message), signing off, signing.
fn options(ui: &mut Ui, form: &mut CommitForm, side: &CommitSide<'_>) {
    ui.horizontal_wrapped(|ui| {
        let amendable = side.head.is_some();
        if ui
            .add_enabled_ui(amendable, |ui| {
                widgets::checkbox(ui, &mut form.amend, "Amend")
            })
            .inner
            .on_hover_text("Replace the last commit: its message, with what is staged")
            .changed()
            && form.amend
            && let (Some(git), Some(head)) = (side.git, side.head)
        {
            form.amended.request(read_message(
                git.clone(),
                Arc::clone(side.repository),
                side.objects.clone(),
                head,
            ));
        }
        crate::widgets::checkbox(ui, &mut form.sign_off, "Sign off");
        let mut sign = form.sign.unwrap_or(side.sign_by_default);
        if crate::widgets::checkbox(ui, &mut sign, "Sign")
            .on_hover_text("With the key and program your git configuration names")
            .changed()
        {
            form.sign = Some(sign);
        }
    });
}

/// What the last commit printed (its hooks' output), or why it wasn't made.
fn outcome(ui: &mut Ui, committing: &mut Bind<Committed, Problem>) {
    let pending = committing.is_pending();
    match committing.read_as_ref() {
        Some(Ok(committed)) if !committed.output.is_empty() && !pending => {
            let output = committed.output.clone();
            ui.add(Label::new(muted(ui, output).small()).wrap());
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
        }
        _ => {}
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
    let entries = list.entries.len();
    widgets::section_header(ui, title, Some(entries), |ui| {
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
    let palette = Palette::of(ui.ctx());
    let mut button_event = None;
    let table_id = list.state.id.to_string();
    let table = TableKit::new(table_id.as_str(), &provider, &mut list.state);
    let shown = crate::tables::list(table, ui)
        .with_max_height(Some(height))
        .show_with_output(ui, |ui, cell, row, text_color| {
            if cell.col_nr != FILE_COLUMN {
                return crate::tables::status_cell(ui, row, palette);
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
    // Across the cell's height, as table-kit centers its own cells' text.
    let room = ui.available_size();
    let response = ui
        .allocate_ui_with_layout(room, Layout::right_to_left(Align::Center), |ui| {
            let button = ui
                .add_enabled(
                    busy.then_some(false).is_none(),
                    egui::Button::new(glyph)
                        .frame_when_inactive(false)
                        .min_size(egui::vec2(crate::widgets::TARGET, crate::widgets::TARGET)),
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
