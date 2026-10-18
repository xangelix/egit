//! The merge editor: a file in conflict, in place of its diff.
//!
//! Its conflicts are stepped through (the buttons, or Alt+↓ and Alt+↑; selecting lines in one
//! selects it), each resolved by taking a side, both in either order, the base, or text edited
//! by hand, and the resolution saved once none is left open. A conflict that isn't two texts
//! (a side deleted, binary contents, a symbolic link, a submodule) offers whole sides instead.

use std::fmt::Write as _;
use std::ops::RangeInclusive;
use std::sync::Arc;

use bstr::BString;
use egit_git::{CancelToken, Merge, Operation, Piece, WholeOnly};
use egui::{Align, Frame, Key, Layout, Margin, Modifiers, RichText, TextEdit, Ui};
use egui_async::Bind;

use super::history::{ID_DIGITS, Shared};
use crate::app::Action;
use crate::grid::TextGrid;
use crate::merge::{Choice, ConflictFile, Labels, MergeDocument, Shown, build, read_conflict};
use crate::problem::Problem;
use crate::staging::{Resolve, ResolveRequest, StageRequest};
use crate::theme::icon;
use crate::widgets::{self, muted};

/// Lines kept above a conflict stepped to.
const ABOVE: u32 = 2;

/// The merge editor's state.
#[derive(Debug)]
pub(super) struct MergeView {
    path: BString,
    file: Bind<Option<Arc<ConflictFile>>, Problem>,
    /// The conflict the choices are for: they start over for another (its sides changed).
    chosen_for: Option<Arc<ConflictFile>>,
    choices: Vec<Choice>,
    /// Counts changes to the choices (or to showing the base); the document shown was made at
    /// `built`, the one being made at `building`.
    version: u64,
    building: u64,
    built: u64,
    selected: usize,
    show_base: bool,
    document: Bind<Arc<MergeDocument>, Problem>,
    pub(super) grid: TextGrid,
    editing: Option<Editing>,
    /// A conflict to scroll to once the document showing the latest choices lands.
    scroll_to: Option<usize>,
    /// The grid's selection last frame (a new one selects its conflict).
    selection_seen: Option<(usize, RangeInclusive<u32>)>,
    cancel: CancelToken,
}

/// A conflict's text being edited by hand.
#[derive(Debug)]
struct Editing {
    conflict: usize,
    text: String,
}

impl MergeView {
    /// The editor of the conflict at `path`, being read.
    pub(super) fn new(shared: &Shared<'_>, path: BString, wrap: bool) -> Self {
        let mut grid = TextGrid::new(("merge", shared.id));
        grid.set_wrap(wrap);
        let mut view = Self {
            path,
            file: Bind::new(true),
            chosen_for: None,
            choices: Vec::new(),
            version: 0,
            building: 0,
            built: 0,
            selected: 0,
            show_base: false,
            document: Bind::new(true),
            grid,
            editing: None,
            scroll_to: Some(0),
            selection_seen: None,
            cancel: CancelToken::new(),
        };
        view.read(shared);
        view
    }

    /// Reads the conflict (again: another program may have resolved it, or changed its sides).
    pub(super) fn read(&mut self, shared: &Shared<'_>) {
        let Some(git) = shared.git else {
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.file.request(read_conflict(
            git.clone(),
            Arc::clone(shared.repository),
            self.path.clone(),
            shared.operation,
            self.cancel.clone(),
        ));
    }

    /// Whether Escape belongs to it (a selection, the find bar, or text being edited).
    pub(super) const fn takes_escape(&self) -> bool {
        self.grid.has_selection() || self.grid.is_finding() || self.editing.is_some()
    }

    /// Takes in the conflict read: a new one (its sides changed) starts its choices over.
    fn take_file(&mut self) -> Option<Arc<ConflictFile>> {
        let file = Arc::clone(self.file.ok_ref()?.as_ref()?);
        let same = self
            .chosen_for
            .as_ref()
            .is_some_and(|chosen| chosen.sides == file.sides);
        if same {
            return self.chosen_for.clone();
        }
        let conflicts = file.text().map_or(0, egit_git::MergedText::conflicts);
        self.choices = vec![Choice::Open; conflicts];
        self.selected = 0;
        self.editing = None;
        self.scroll_to = Some(0);
        self.changed();
        self.chosen_for = Some(Arc::clone(&file));
        Some(file)
    }

    /// The choices (or what is shown) changed: the document is made again.
    const fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Resolves the selected conflict with `choice`, and moves on to the next open one.
    fn choose(&mut self, choice: Choice) {
        if let Some(slot) = self.choices.get_mut(self.selected) {
            *slot = choice;
            self.changed();
        }
        let count = self.choices.len();
        let next = (1..=count)
            .filter_map(|step| self.selected.saturating_add(step).checked_rem(count))
            .find(|&index| self.choices.get(index) == Some(&Choice::Open));
        if let Some(next) = next {
            self.selected = next;
        }
        self.scroll_to = Some(self.selected);
    }

    /// Selects conflict `index`, and scrolls to it.
    const fn select(&mut self, index: usize) {
        self.selected = index;
        self.scroll_to = Some(index);
    }

    fn open_conflicts(&self) -> usize {
        self.choices
            .iter()
            .filter(|choice| **choice == Choice::Open)
            .count()
    }
}

impl Drop for MergeView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the sides' headers say: `HEAD` and its branch; the commit coming in.
fn labels(shared: &Shared<'_>, file: &ConflictFile) -> Labels {
    let current = match (shared.operation, shared.branch()) {
        (Some(Operation::Rebase), _) => "HEAD, being rebased onto".to_owned(),
        (_, Some(branch)) => format!("HEAD ({branch})"),
        (_, None) => "HEAD".to_owned(),
    };
    let incoming = file.incoming.as_ref().map_or_else(String::new, |commit| {
        let mut what = commit.branches.join(", ");
        if !what.is_empty() {
            what.push_str(" · ");
        }
        let _ = write!(
            what,
            "{} {}",
            commit.id.abbreviated(ID_DIGITS),
            commit.subject
        );
        match shared.operation {
            Some(Operation::Revert) => format!("reverting {what}"),
            Some(Operation::Rebase) => format!("{what}, replayed"),
            _ => what,
        }
    });
    Labels { current, incoming }
}

/// Draws the merge editor (under the file's bars).
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut MergeView,
    actions: &mut Vec<Action>,
) {
    let read = match view.file.read_as_ref() {
        None => Err(None),
        Some(Err(problem)) => Err(Some(problem.clone())),
        Some(Ok(found)) => Ok(found.is_some()),
    };
    let file = match read {
        Ok(true) => match view.take_file() {
            Some(file) => file,
            None => return,
        },
        Ok(false) => {
            Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.label(muted(ui, "The file isn't in conflict any more."));
            });
            return;
        }
        Err(Some(problem)) => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return;
        }
        Err(None) => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Reading the conflict…"));
            return;
        }
    };
    match &file.merge {
        Merge::WholeOnly(why) => whole(ui, shared, &file, *why, actions),
        Merge::Text(_) => text(ui, shared, view, &file, actions),
    }
}

/// A conflict that only a whole side resolves: what happened, and the sides to keep.
fn whole(
    ui: &mut Ui,
    shared: &Shared<'_>,
    file: &Arc<ConflictFile>,
    why: WholeOnly,
    actions: &mut Vec<Action>,
) {
    let sides = &file.sides;
    let (current, incoming) = (sides.current.is_some(), sides.incoming.is_some());
    let said = match why {
        WholeOnly::Deleted if !current && !incoming => "Both sides deleted the file.",
        WholeOnly::Deleted if !current => {
            "The current side deleted the file, and the incoming side changed it."
        }
        WholeOnly::Deleted => {
            "The incoming side deleted the file, and the current side changed it."
        }
        WholeOnly::Binary => "A binary file both sides changed: keep one side's.",
        WholeOnly::Link => "A symbolic link, on one side or both: keep one side's.",
        WholeOnly::Submodule => "A submodule both sides moved: keep the commit one side records.",
    };
    let mut how = None;
    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
        ui.label(said);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if current && widgets::secondary(ui, true, "Keep the current side's").clicked() {
                how = Some(Resolve::Current);
            }
            if incoming && widgets::secondary(ui, true, "Keep the incoming side's").clicked() {
                how = Some(Resolve::Incoming);
            }
            if why != WholeOnly::Submodule
                && widgets::secondary(ui, true, "Delete the file").clicked()
            {
                how = Some(Resolve::Delete);
            }
        });
    });
    if let Some(how) = how {
        resolve(shared, file, how, actions);
    }
}

/// Asks for `file` to be resolved as `how` says.
fn resolve(shared: &Shared<'_>, file: &Arc<ConflictFile>, how: Resolve, actions: &mut Vec<Action>) {
    actions.push(Action::Stage(
        shared.id,
        StageRequest::Resolve(ResolveRequest {
            file: Arc::clone(file),
            how,
        }),
    ));
}

/// Two texts' conflicts: the bars, the text being edited, and the merged file.
fn text(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut MergeView,
    file: &Arc<ConflictFile>,
    actions: &mut Vec<Action>,
) {
    let count = view.choices.len();
    // Alt+↓ and Alt+↑ step through the conflicts, before the grid takes the arrows.
    let (next, previous) = ui.input_mut(|input| {
        (
            input.consume_key(Modifiers::ALT, Key::ArrowDown),
            input.consume_key(Modifiers::ALT, Key::ArrowUp),
        )
    });
    if count > 0 && view.editing.is_none() {
        if next {
            view.select(view.selected.saturating_add(1).min(count.saturating_sub(1)));
        }
        if previous {
            view.select(view.selected.saturating_sub(1));
        }
    }
    bars(ui, shared, view, file, actions);
    if view.editing.is_some() {
        editor(ui, view, file);
    }
    // The document showing the latest choices, made in a job.
    if view.building != view.version {
        view.building = view.version;
        view.document.request(build(
            Arc::clone(file),
            view.choices.clone().into(),
            view.show_base,
            labels(shared, file),
        ));
    }
    if !view.document.is_pending() && view.document.is_ok() {
        view.built = view.building;
    }
    let document = match view.document.read_as_ref() {
        Some(Ok(document)) => Arc::clone(document),
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return;
        }
        None => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Merging the sides…"));
            return;
        }
    };
    if view.built == view.version
        && let Some(index) = view.scroll_to.take()
        && let Some(start) = document.start(index)
    {
        view.grid.scroll_to_line(start.saturating_sub(ABOVE));
    }
    // Lines selected in a conflict select it.
    let selection = view.grid.selected_lines();
    if selection != view.selection_seen {
        if let Some(conflict) = selection
            .as_ref()
            .and_then(|(_, lines)| document.conflict_at(*lines.start()))
        {
            view.selected = conflict;
        }
        view.selection_seen = selection;
    }
    let shown = Shown {
        document: &document,
        selected: Some(view.selected),
    };
    view.grid.show(ui, &shown);
}

/// The bars over the merged file: where the selected conflict is, stepping through them, and
/// saving; what to take for it, and for all of them.
fn bars(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut MergeView,
    file: &Arc<ConflictFile>,
    actions: &mut Vec<Action>,
) {
    Frame::new()
        .fill(shared.palette.panel)
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| step_bar(ui, shared, view, file, actions));
            ui.horizontal_wrapped(|ui| pick_bar(ui, view, file));
        });
}

/// Where the selected conflict is, stepping through them, and saving.
fn step_bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut MergeView,
    file: &Arc<ConflictFile>,
    actions: &mut Vec<Action>,
) {
    let count = view.choices.len();
    let open = view.open_conflicts();
    let editing = view.editing.is_some();
    let first = view.selected == 0;
    let last = view.selected.saturating_add(1) >= count;
    if widgets::icon_tool(ui, !first && !editing, icon::ARROW_UP, "Previous")
        .on_hover_text(format!("The conflict before (Alt+{})", icon::ARROW_UP))
        .clicked()
    {
        view.select(view.selected.saturating_sub(1));
    }
    if widgets::icon_tool(ui, !last && !editing, icon::ARROW_DOWN, "Next")
        .on_hover_text(format!("The conflict after (Alt+{})", icon::ARROW_DOWN))
        .clicked()
    {
        view.select(view.selected.saturating_add(1));
    }
    let choice = view.choices.get(view.selected).map_or("", Choice::name);
    ui.label(
        RichText::new(format!(
            "Conflict {} of {count}",
            view.selected.saturating_add(1).min(count)
        ))
        .strong(),
    );
    ui.label(muted(ui, choice));
    let left = match open {
        0 => "· all resolved".to_owned(),
        open => format!("· {open} left"),
    };
    ui.label(muted(ui, left));
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::primary(
            ui,
            open == 0 && !editing,
            &format!("{}  Save resolution", icon::CHECK),
        )
        .on_disabled_hover_text("Resolve every conflict first")
        .clicked()
        {
            let how = Resolve::Merged(view.choices.clone().into());
            resolve(shared, file, how, actions);
        }
        if widgets::checkbox(ui, &mut view.show_base, "Show the base").changed() {
            view.changed();
        }
    });
}

/// What to take for the selected conflict, and for all of them.
fn pick_bar(ui: &mut Ui, view: &mut MergeView, file: &ConflictFile) {
    let count = view.choices.len();
    let editing = view.editing.is_some();
    let ready = count > 0 && !editing;
    ui.label(muted(ui, "Take"));
    for (label, choice, hover) in [
        ("Current", Choice::Current, "This branch's side"),
        ("Incoming", Choice::Incoming, "The side coming in"),
        (
            "Both",
            Choice::CurrentThenIncoming,
            "Both sides, this branch's first",
        ),
        (
            "Base",
            Choice::Base,
            "As it was before either side changed it",
        ),
    ] {
        if widgets::secondary(ui, ready, label)
            .on_hover_text(hover)
            .clicked()
        {
            view.choose(choice);
        }
    }
    let editable = edit_text(view, file);
    if widgets::secondary(ui, editable.is_some() && !editing, "Edit…")
        .on_disabled_hover_text("Its lines aren't UTF-8 text")
        .clicked()
        && let Some(text) = editable
    {
        view.editing = Some(Editing {
            conflict: view.selected,
            text,
        });
    }
    let resolved = view
        .choices
        .get(view.selected)
        .is_some_and(|choice| *choice != Choice::Open);
    // The rarer choices, a menu away.
    widgets::tool_menu(ui, icon::DOTS_THREE, "More", widgets::Look::Full, |ui| {
        ui.set_min_width(240.0);
        if ui
            .add_enabled(ready, egui::Button::new("Both, incoming first"))
            .clicked()
        {
            view.choose(Choice::IncomingThenCurrent);
        }
        if ui
            .add_enabled(
                resolved && !editing,
                egui::Button::new("Reopen this conflict"),
            )
            .clicked()
            && let Some(slot) = view.choices.get_mut(view.selected)
        {
            *slot = Choice::Open;
            view.changed();
            view.scroll_to = Some(view.selected);
        }
        ui.separator();
        for (label, choice) in [
            ("All current", Choice::Current),
            ("All incoming", Choice::Incoming),
        ] {
            if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                view.choices.fill(choice);
                view.changed();
                view.scroll_to = Some(view.selected);
            }
        }
    });
}

/// The selected conflict's text to edit: what it resolves to, or both sides while open;
/// `None` when that isn't UTF-8.
fn edit_text(view: &MergeView, file: &ConflictFile) -> Option<String> {
    let text = file.text()?;
    let choice = view.choices.get(view.selected)?;
    let pick = choice.pick().unwrap_or(egit_git::Pick::CurrentThenIncoming);
    let bytes = text.picked(view.selected, pick)?;
    String::from_utf8(bytes).ok()
}

/// The text of a conflict being edited, with Apply and Cancel.
fn editor(ui: &mut Ui, view: &mut MergeView, file: &ConflictFile) {
    let Some(editing) = &mut view.editing else {
        return;
    };
    let conflict = editing.conflict;
    let mut done = None;
    Frame::new()
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            let label = ui.label(muted(
                ui,
                format!(
                    "Conflict {} resolves to (Escape cancels):",
                    conflict.saturating_add(1)
                ),
            ));
            let field = ui
                .add(
                    TextEdit::multiline(&mut editing.text)
                        .code_editor()
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            if field.has_focus() && ui.input(|input| input.key_pressed(Key::Escape)) {
                done = Some(false);
            }
            ui.horizontal(|ui| {
                if widgets::primary(ui, true, "Apply").clicked() {
                    done = Some(true);
                }
                if widgets::secondary(ui, true, "Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
    match done {
        Some(true) => {
            let mut text = std::mem::take(&mut editing.text).into_bytes();
            // Lines after it stay lines of their own.
            if !text.is_empty() && !text.ends_with(b"\n") && !is_last(file, conflict) {
                text.push(b'\n');
            }
            view.editing = None;
            view.selected = conflict;
            view.choose(Choice::Edited(text.into()));
        }
        Some(false) => view.editing = None,
        None => {}
    }
}

/// Whether conflict `index` ends the file.
fn is_last(file: &ConflictFile, index: usize) -> bool {
    file.text().is_some_and(|text| {
        matches!(text.pieces().last(), Some(Piece::Conflict(_)))
            && text.conflicts() == index.saturating_add(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_moves_on_to_the_next_open_conflict() {
        let mut choices = vec![Choice::Open; 3];
        choices[1] = Choice::Current;
        let mut view = MergeView {
            path: BString::from("f"),
            file: Bind::new(true),
            chosen_for: None,
            choices,
            version: 0,
            building: 0,
            built: 0,
            selected: 0,
            show_base: false,
            document: Bind::new(true),
            grid: TextGrid::new("test"),
            editing: None,
            scroll_to: None,
            selection_seen: None,
            cancel: CancelToken::new(),
        };
        view.choose(Choice::Incoming);
        // Past the resolved one.
        assert_eq!(view.selected, 2);
        assert_eq!(view.scroll_to, Some(2));
        view.choose(Choice::Base);
        // None left: it stays.
        assert_eq!(view.selected, 2);
        assert_eq!(view.open_conflicts(), 0);
        assert_eq!(view.version, 2);
    }
}
