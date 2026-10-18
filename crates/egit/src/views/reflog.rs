//! The reflog in the commit table's place: where `HEAD` (or a branch) has been, newest first,
//! each move with when, what made it, and its commit; the selected move's commit in the details
//! beside it (its files opened in the reflog's place), shown in the history (when it is there),
//! a branch made there (keeping commits nothing else reaches), the current branch reset there,
//! or `HEAD` switched there. Escape or History goes back.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{CancelToken, ObjectId, ReflogEntry, ResetMode, SwitchTo, reflog};
use egui::{Frame, Id, Key, Margin, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;

use super::actions::{self, Ask};
use super::dialogs::Dialog;
use super::file::{self, FileView, FileWanted};
use super::history::{ID_DIGITS, Shared};
use crate::app::Action;
use crate::format::{self, count};
use crate::history::{History, Objects};
use crate::jobs;
use crate::problem::Problem;
use crate::tasks::Task;
use crate::theme::Weighted as _;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// The most moves read.
const MOST: usize = 10_000;

/// A row's height.
const ROW_HEIGHT: f32 = 24.0;

/// What the view asks of the history view.
#[derive(Debug)]
pub(super) enum ReflogEvent {
    Close,
    ShowCommit(ObjectId),
    Ask(Ask),
    /// A file's blame or history, from a file opened in its place.
    Wanted(FileWanted),
}

/// The reflog view's state.
#[derive(Debug)]
pub(super) struct ReflogView {
    /// The ref whose reflog: `HEAD`, or a branch's full name.
    reference: BString,
    entries: Bind<Arc<Vec<ReflogEntry>>, Problem>,
    asked: bool,
    cancel: CancelToken,
    selected: Option<usize>,
    /// The selected move (its commit and time), found again in a reflog read again.
    selected_move: Option<(ObjectId, i64)>,
    /// The moves the selection was last placed in.
    placed: Option<Arc<Vec<ReflogEntry>>>,
    reveal: bool,
    /// Where `HEAD` was last frame (on a branch, at a commit): when it moves, the reflog is
    /// read again.
    head: Option<(Option<BString>, Option<ObjectId>)>,
    /// Whether the selected move's commit is in the history shown (worked out once a
    /// selection: a scan).
    shown: Option<(usize, Option<u64>)>,
    /// A file of the selected move's commit, opened from the details, in the reflog's place.
    file: Option<FileView>,
}

impl Drop for ReflogView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl ReflogView {
    /// The reflog of `reference` (`HEAD`, or a branch's full name).
    pub(super) fn new(reference: BString) -> Self {
        Self {
            reference,
            entries: Bind::new(true),
            asked: false,
            cancel: CancelToken::new(),
            selected: None,
            selected_move: None,
            placed: None,
            reveal: false,
            head: None,
            shown: None,
            file: None,
        }
    }

    /// The selected move's commit, which the details show.
    pub(super) fn selected_commit(&self) -> Option<ObjectId> {
        let entries = self.placed.as_ref()?;
        entries.get(self.selected?).map(|entry| entry.commit)
    }

    /// The file open in the reflog's place, if any.
    pub(super) const fn file(&self) -> Option<&FileView> {
        self.file.as_ref()
    }

    /// Opens a file of the selected move's commit in the reflog's place.
    pub(super) fn open_file(&mut self, file: FileView) {
        self.file = Some(file);
    }

    /// Reads the reflog again (what is shown stays until it lands), and looks for the selected
    /// move's commit in the history again.
    pub(super) const fn refresh(&mut self) {
        self.asked = false;
        self.shown = None;
    }

    /// Selects the move at `at`.
    fn select(&mut self, entries: &[ReflogEntry], at: usize) {
        self.selected = Some(at);
        self.selected_move = entries.get(at).map(|entry| (entry.commit, entry.time));
    }

    /// Keeps the selection on the same move in a reflog read again (a move made since comes
    /// first, so the same place is another move), or on the first.
    fn place(&mut self, entries: &Arc<Vec<ReflogEntry>>) {
        if self
            .placed
            .as_ref()
            .is_some_and(|placed| Arc::ptr_eq(placed, entries))
        {
            return;
        }
        self.placed = Some(Arc::clone(entries));
        self.shown = None;
        let found = self.selected_move.and_then(|(commit, time)| {
            entries
                .iter()
                .position(|entry| entry.commit == commit && entry.time == time)
        });
        match found {
            Some(at) => self.selected = Some(at),
            None if entries.is_empty() => self.selected = None,
            None => self.select(entries, 0),
        }
    }

    /// Whose reflog it is, for people.
    fn whose(&self) -> String {
        let name = self.reference.as_slice();
        name.strip_prefix(b"refs/heads/")
            .or_else(|| name.strip_prefix(b"refs/remotes/"))
            .unwrap_or(name)
            .to_str_lossy()
            .into_owned()
    }
}

/// Draws the view in the commit table's place (or the file opened in it); returns what it
/// asks.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (history, objects): (&History, &Objects),
    view: &mut ReflogView,
    actions: &mut Vec<Action>,
) -> Option<ReflogEvent> {
    if let Some(open) = &mut view.file {
        // Escape (which the file takes) and its History go back to the reflog.
        let shown = file::show(ui, shared, objects, open, actions);
        let wanted = open.wanted.take();
        if !shown {
            view.file = None;
        }
        return wanted.map(ReflogEvent::Wanted);
    }
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(ReflogEvent::Close);
    let head = Some((shared.branch().map(BStr::to_owned), shared.head_commit()));
    if view.head != head {
        if view.head.is_some() {
            view.refresh();
        }
        view.head = head;
    }
    if !view.asked
        && let Some(git) = shared.git
    {
        view.asked = true;
        let (git, repository, cancel) = (
            git.clone(),
            Arc::clone(shared.repository),
            view.cancel.clone(),
        );
        let reference = view.reference.clone();
        view.entries.request(jobs::blocking(move || {
            reflog(&git, &repository, reference.as_bstr(), MOST, &cancel)
                .map(Arc::new)
                .map_err(|error| Problem::git("The reflog couldn't be read", &error))
        }));
    }
    let entries = match view.entries.read_as_ref() {
        Some(Ok(entries)) => Arc::clone(entries),
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return event;
        }
        None => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Reading the reflog…"));
            return event;
        }
    };
    view.place(&entries);
    keys(ui, view, &entries);
    if let Some(at) = view.selected
        && view.shown.is_none_or(|(known, _)| known != at)
        && let Some(entry) = entries.get(at)
    {
        view.shown = Some((at, history.row_of(&entry.commit, 0)));
    }
    if let Some(asked) = bar(ui, shared, history, view, &entries) {
        event = Some(asked);
    }
    list(ui, shared, view, &entries);
    event
}

/// ↑ and ↓ step through the moves, while no text field has the focus.
fn keys(ui: &Ui, view: &mut ReflogView, entries: &[ReflogEntry]) {
    if ui.ctx().text_edit_focused() || entries.is_empty() {
        return;
    }
    let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
    let at = view.selected.unwrap_or(0);
    let to = if pressed(Key::ArrowUp) {
        at.saturating_sub(1)
    } else if pressed(Key::ArrowDown) {
        at.saturating_add(1).min(entries.len().saturating_sub(1))
    } else {
        return;
    };
    view.select(entries, to);
    view.reveal = true;
}

/// The bar: back, whose reflog, how many moves, and what can be done from the one selected.
fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &History,
    view: &ReflogView,
    entries: &[ReflogEntry],
) -> Option<ReflogEvent> {
    let mut event = None;
    let menus = shared.menus(history.refs());
    let current = shared.branch().map_or_else(
        || "HEAD".to_owned(),
        |branch| branch.to_str_lossy().into_owned(),
    );
    let selected = view.selected.and_then(|at| entries.get(at));
    let in_history = view.shown.and_then(|(_, row)| row).is_some();
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT))
                .on_hover_text("Back to the history (Escape)")
                .clicked()
            {
                event = Some(ReflogEvent::Close);
            }
            ui.label(RichText::new(format!("Reflog of {}", view.whose())).semibold());
            let moves = match entries.len() {
                1 => "1 move".to_owned(),
                moves => format!("{} moves", count(u64::try_from(moves).unwrap_or(u64::MAX))),
            };
            ui.label(muted(ui, moves));
        });
        // What can be done from the move selected, on a row of its own.
        ui.horizontal_wrapped(|ui| {
            let Some(entry) = selected else {
                return;
            };
            ui.label(muted(ui, "This move:"));
            let commit = entry.commit;
            let show = widgets::secondary(ui, in_history, "Show the commit");
            let show = show.on_disabled_hover_text(
                "It isn't in the history shown: no branch or tag reaches it. Create a \
                     branch here to keep it.",
            );
            if show.clicked() {
                event = Some(ReflogEvent::ShowCommit(commit));
            }
            let mut ask = |asked| event = Some(ReflogEvent::Ask(asked));
            if widgets::secondary(ui, menus.idle().is_ok(), "Create a branch here…").clicked() {
                ask(Ask::Dialog(Dialog::CreateBranch {
                    at: commit,
                    name: String::new(),
                    // Kept, where nothing else reaches: no need to go there.
                    switch: false,
                }));
            }
            let can = menus.can_change();
            ui.add_enabled_ui(can.is_ok(), |ui| {
                let name = format!("Reset {current} to here");
                let button = egui::Button::new(format!("{name}  {}", icon::CARET_DOWN))
                    .wrap_mode(egui::TextWrapMode::Extend);
                let (response, _) =
                    egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                        for (label, mode) in [
                            ("Soft: keep the changes staged", ResetMode::Soft),
                            ("Mixed: keep the changes, unstaged", ResetMode::Mixed),
                            (
                                "Keep: keep uncommitted changes (refused if in the way)",
                                ResetMode::Keep,
                            ),
                        ] {
                            if actions::item(ui, (icon::ARROW_U_UP_LEFT, label), Ok(())) {
                                ask(Ask::Task(Task::Reset(commit, mode)));
                            }
                        }
                        let hard = (icon::TRASH, "Hard: discard the changes…");
                        if actions::danger_item(ui, hard, Ok(())) {
                            ask(Ask::Dialog(Dialog::HardReset { at: commit }));
                        }
                    });
                crate::help::note(ui, response.rect, &name);
                // Named in words: the caret only shows it opens a menu.
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, can.is_ok(), &name)
                });
                response.on_disabled_hover_text(can.err().unwrap_or_default());
            });
            if widgets::secondary(ui, can.is_ok(), "Switch here, without a branch").clicked() {
                ask(Ask::Task(Task::Switch(SwitchTo::Detached(commit))));
            }
        });
    });
    event
}

/// The moves, newest first, drawn as far as they show.
fn list(ui: &mut Ui, shared: &Shared<'_>, view: &mut ReflogView, entries: &[ReflogEntry]) {
    let palette = shared.palette;
    let now = format::now();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("reflog-rows", shared.id))
        .auto_shrink([false, false]);
    if std::mem::take(&mut view.reveal)
        && let Some(at) = view.selected
    {
        let pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
        let row = f32::from(u16::try_from(at).unwrap_or(u16::MAX));
        scroll = scroll.vertical_scroll_offset(((row - 2.0).max(0.0)) * pitch);
    }
    scroll.show_rows(ui, ROW_HEIGHT, entries.len(), |ui, visible| {
        for at in visible {
            let Some(entry) = entries.get(at) else {
                continue;
            };
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            let response = ui.interact(rect, Id::new(("reflog-entry", at)), Sense::click());
            let selected = view.selected == Some(at);
            let action = entry.action.to_str_lossy().into_owned();
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    &action,
                )
            });
            crate::widgets::row_fill(ui, rect, 0.0, selected, response.hovered());
            if response.clicked() {
                view.select(entries, at);
            }
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            row.add_sized(
                [64.0, ROW_HEIGHT],
                egui::Label::new(muted(&row, format!("@{{{}}}", entry.index))).selectable(false),
            );
            let age = u64::try_from(entry.time)
                .map_or_else(|_| String::new(), |time| format::ago(time, now));
            row.add_sized(
                [110.0, ROW_HEIGHT],
                egui::Label::new(muted(&row, age))
                    .selectable(false)
                    .truncate(),
            );
            row.add(
                egui::Label::new(action.as_str())
                    .truncate()
                    .selectable(false),
            );
            // Not selectable: a click on it selects the row.
            row.add(
                egui::Label::new(
                    RichText::new(entry.commit.abbreviated(ID_DIGITS).to_string())
                        .monospace()
                        .color(palette.muted),
                )
                .selectable(false),
            );
            row.add(
                egui::Label::new(muted(&row, entry.subject.to_str_lossy()))
                    .truncate()
                    .selectable(false),
            );
        }
    });
}
