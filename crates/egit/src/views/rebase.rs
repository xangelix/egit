//! The interactive rebase editor, in place of the commit table: the current branch's commits
//! from a chosen one up to `HEAD`, newest first as the graph shows them, each picked, reworded,
//! squashed or fixed up into the commit below it, or dropped, and moved up and down; under them,
//! the selected commit's message (edited when it is reworded). "Start rebase" runs the plan as
//! one of the repository's tasks.
//!
//! Keys, while no text field has the focus: ↑ and ↓ select, Alt+↑ and Alt+↓ move the selected
//! commit, and P, R, S, F, D pick, reword, squash, fix up, or drop it. The list draws only the
//! rows on screen (one height each), so a long plan costs what shows.
//!
//! A range holding merges is rebased keeping them, in git's own order: its rows don't move,
//! nothing is melded, and a merge is kept or reworded, never dropped.

use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{CancelToken, ObjectId, Planned, RebaseRange, Step, rebase_range};
use egui::{Align, Frame, Id, Key, Layout, Margin, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;

use super::history::{ID_DIGITS, Shared};
use crate::jobs;
use crate::problem::Problem;
use crate::tasks::{Task, range_problem};
use crate::theme::Weighted as _;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// A row's height in the plan.
const ROW_HEIGHT: f32 = 30.0;

/// What becomes of a commit, as the editor offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl Kind {
    /// Its help topic.
    const fn topic(self) -> &'static str {
        match self {
            Self::Pick => "rebase-pick",
            Self::Reword => "rebase-reword",
            Self::Squash => "rebase-squash",
            Self::Fixup => "rebase-fixup",
            Self::Drop => "drop",
        }
    }

    const ALL: [Self; 5] = [
        Self::Pick,
        Self::Reword,
        Self::Squash,
        Self::Fixup,
        Self::Drop,
    ];

    /// Its name in the plan.
    const fn name(self) -> &'static str {
        match self {
            Self::Pick => "Pick",
            Self::Reword => "Reword",
            Self::Squash => "Squash",
            Self::Fixup => "Fix up",
            Self::Drop => "Drop",
        }
    }

    /// The key that sets it.
    const fn key(self) -> Key {
        match self {
            Self::Pick => Key::P,
            Self::Reword => Key::R,
            Self::Squash => Key::S,
            Self::Fixup => Key::F,
            Self::Drop => Key::D,
        }
    }

    /// Whether it melds its commit into the one below.
    const fn melds(self) -> bool {
        matches!(self, Self::Squash | Self::Fixup)
    }

    /// Whether a commit may be given it: a merge (`merge`) or not, in a range holding merges
    /// (`keeping`, rebased in git's order) or not.
    const fn allowed(self, merge: bool, keeping: bool) -> bool {
        match self {
            Self::Pick | Self::Reword => true,
            Self::Drop => !merge,
            Self::Squash | Self::Fixup => !keeping,
        }
    }
}

/// A commit in the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    /// Its place in the range (oldest first).
    commit: usize,
    kind: Kind,
    /// Its new message, once reworded (kept if it is picked again, in case).
    message: Option<String>,
}

/// The editor's state.
#[derive(Debug)]
pub(super) struct RebaseView {
    /// The commit to reword at first (opened to edit its message).
    reword: Option<ObjectId>,
    range: Bind<Arc<RebaseRange>, Problem>,
    /// The plan, newest first; empty until the range is read.
    rows: Vec<Row>,
    selected: usize,
    /// Scroll the selected row into view next frame.
    reveal: bool,
    cancel: CancelToken,
}

impl Drop for RebaseView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the editor asks of the history view.
pub(super) enum RebaseEvent {
    /// Close it.
    Close,
    /// Run this task (and close it).
    Start(Task),
}

impl RebaseView {
    /// The editor of the commits from `from` up to `HEAD` (with `reword` set to be reworded),
    /// its range being read.
    pub(super) fn new(shared: &Shared<'_>, from: ObjectId, reword: Option<ObjectId>) -> Self {
        let mut view = Self {
            reword,
            range: Bind::new(true),
            rows: Vec::new(),
            selected: 0,
            reveal: true,
            cancel: CancelToken::new(),
        };
        if let Some(git) = shared.git {
            let (git, repository, cancel) = (
                git.clone(),
                Arc::clone(shared.repository),
                view.cancel.clone(),
            );
            view.range.request(jobs::blocking(async move {
                rebase_range(&git, &repository, from, &cancel)
                    .await
                    .map(Arc::new)
                    .map_err(|error| range_problem(&error))
            }));
        }
        view
    }

    /// Whether the keyboard belongs to a text field (the message), not to the plan.
    fn typing(ui: &Ui) -> bool {
        ui.ctx().text_edit_focused()
    }

    /// Makes the plan from the range read: every commit picked, newest first.
    fn plan_from(&mut self, range: &RebaseRange) {
        self.rows = (0..range.commits.len())
            .rev()
            .map(|commit| Row {
                commit,
                kind: Kind::Pick,
                message: None,
            })
            .collect();
        if let Some(reword) = self.reword
            && let Some(at) = self.rows.iter().position(|row| {
                range.commits.get(row.commit).map(|commit| commit.id) == Some(reword)
            })
        {
            self.selected = at;
            self.set_kind(range, Kind::Reword);
        }
    }

    /// Sets the selected commit's step (a reword starts from its message), when it is one
    /// that commit may have.
    fn set_kind(&mut self, range: &RebaseRange, kind: Kind) {
        let Some(row) = self.rows.get_mut(self.selected) else {
            return;
        };
        let merge = range
            .commits
            .get(row.commit)
            .is_some_and(|commit| commit.merge);
        if !kind.allowed(merge, range.merges() > 0) {
            return;
        }
        row.kind = kind;
        if kind == Kind::Reword && row.message.is_none() {
            row.message = range
                .commits
                .get(row.commit)
                .map(|commit| commit.message.clone());
        }
    }

    /// Moves the selected commit up (toward `HEAD`) or down (never in a range holding merges:
    /// the order is git's).
    fn shift(&mut self, range: &RebaseRange, up: bool) {
        if range.merges() > 0 {
            return;
        }
        let other = if up {
            self.selected.checked_sub(1)
        } else {
            self.selected
                .checked_add(1)
                .filter(|&below| below < self.rows.len())
        };
        if let Some(other) = other {
            self.rows.swap(self.selected, other);
            self.selected = other;
            self.reveal = true;
        }
    }

    /// The plan for git (oldest first), or why it won't do.
    fn plan(&self, range: &RebaseRange) -> Result<Vec<Planned>, &'static str> {
        let unchanged = self
            .rows
            .iter()
            .rev()
            .enumerate()
            .all(|(at, row)| row.commit == at && row.kind == Kind::Pick);
        if unchanged {
            return Err("Nothing changes yet");
        }
        let mut kept = false;
        let mut plan = Vec::with_capacity(self.rows.len());
        for row in self.rows.iter().rev() {
            let commit = range
                .commits
                .get(row.commit)
                .ok_or("The plan is out of date")?;
            let step = match row.kind {
                Kind::Pick => Step::Pick,
                Kind::Drop => Step::Drop,
                Kind::Reword => {
                    let message = row.message.as_deref().unwrap_or_default();
                    if message.trim().is_empty() {
                        return Err("A reworded commit needs a message");
                    }
                    Step::Reword(message.to_owned())
                }
                Kind::Squash | Kind::Fixup if !kept => {
                    return Err("The lowest commit kept can't meld into one below it");
                }
                Kind::Squash => Step::Squash,
                Kind::Fixup => Step::Fixup,
            };
            kept |= matches!(step, Step::Pick | Step::Reword(_));
            plan.push(Planned {
                id: commit.id,
                step,
            });
        }
        if !kept {
            return Err("Every commit is dropped (reset the branch instead)");
        }
        Ok(plan)
    }
}

/// Draws the editor; returns what it asks of the history view.
pub(super) fn show(ui: &mut Ui, shared: &Shared<'_>, view: &mut RebaseView) -> Option<RebaseEvent> {
    let escape = !RebaseView::typing(ui)
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let range = match view.range.read_as_ref() {
        Some(Ok(range)) => Arc::clone(range),
        Some(Err(problem)) => {
            let problem = problem.clone();
            return failed(ui, &problem, escape);
        }
        None => {
            let mut event = escape.then_some(RebaseEvent::Close);
            Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked()
                {
                    event = Some(RebaseEvent::Close);
                }
                widgets::working(ui, "Reading the commits…");
            });
            return event;
        }
    };
    if view.rows.is_empty() {
        view.plan_from(&range);
    }
    keys(ui, view, &range);
    let event = bar(ui, shared, view, &range).or_else(|| escape.then_some(RebaseEvent::Close));
    egui::Panel::bottom(Id::new(("rebase-message", shared.id)))
        .resizable(true)
        .default_size(190.0)
        .size_range(110.0..=480.0)
        .frame(
            Frame::new()
                .fill(shared.palette.panel)
                .inner_margin(Margin::same(10)),
        )
        .show(ui, |ui| message(ui, view, &range));
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| rows(ui, shared, view, &range));
    event
}

/// The range couldn't be read: why, and the way back.
fn failed(ui: &mut Ui, problem: &Problem, escape: bool) -> Option<RebaseEvent> {
    let mut event = escape.then_some(RebaseEvent::Close);
    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
        widgets::problem_box(ui, problem, |ui| {
            if widgets::secondary(ui, true, "Back to the history").clicked() {
                event = Some(RebaseEvent::Close);
            }
        });
    });
    event
}

/// The keys, while no text field has the focus.
fn keys(ui: &Ui, view: &mut RebaseView, range: &RebaseRange) {
    if RebaseView::typing(ui) {
        return;
    }
    let pressed = |key, modifiers| ui.input_mut(|input| input.consume_key(modifiers, key));
    if pressed(Key::ArrowUp, Modifiers::ALT) {
        view.shift(range, true);
    } else if pressed(Key::ArrowDown, Modifiers::ALT) {
        view.shift(range, false);
    } else if pressed(Key::ArrowUp, Modifiers::NONE) {
        view.selected = view.selected.saturating_sub(1);
        view.reveal = true;
    } else if pressed(Key::ArrowDown, Modifiers::NONE) {
        view.selected = view
            .selected
            .saturating_add(1)
            .min(view.rows.len().saturating_sub(1));
        view.reveal = true;
    }
    for kind in Kind::ALL {
        if pressed(kind.key(), Modifiers::NONE) {
            view.set_kind(range, kind);
        }
    }
}

/// The width of a row's step menu, the widest step's.
const STEP_WIDTH: f32 = 104.0;

/// The bar: back, what is rebased onto, how the plan stands, and Start rebase.
fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &RebaseView,
    range: &RebaseRange,
) -> Option<RebaseEvent> {
    let mut event = None;
    let branch = shared.branch().map_or_else(
        || "HEAD".to_owned(),
        |branch| branch.to_str_lossy().into_owned(),
    );
    let onto = range.base.map_or_else(
        || "from the root".to_owned(),
        |base| format!("onto {}", base.abbreviated(ID_DIGITS)),
    );
    let plan = view.plan(range);
    widgets::bar(ui, |ui| {
        // Start rebase at the right, where it stays; what the plan says and the keys on a line
        // of their own below.
        ui.horizontal(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked() {
                event = Some(RebaseEvent::Close);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let start = widgets::primary(ui, plan.is_ok(), "Start rebase");
                let start = match &plan {
                    Err(why) => start.on_disabled_hover_text(*why),
                    Ok(_) => start,
                };
                if start.clicked()
                    && let Ok(plan) = &plan
                {
                    event = Some(RebaseEvent::Start(Task::InteractiveRebase {
                        base: range.base,
                        plan: plan.clone(),
                        keep_merges: range.merges() > 0,
                    }));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Rebase {branch} interactively")).semibold(),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::Label::new(muted(
                            ui,
                            format!("{} commits, {onto}", range.commits.len()),
                        ))
                        .truncate(),
                    );
                });
            });
        });
        ui.horizontal_wrapped(|ui| {
            match &plan {
                Err(why) => {
                    ui.label(muted(ui, format!("{why}.")));
                }
                Ok(_) if range.merges() > 0 => {
                    ui.label(muted(ui, "Keys: P R D."));
                }
                Ok(_) => {
                    // The arrows are icons: the text font has none.
                    ui.label(muted(
                        ui,
                        format!(
                            "Keys: P R S F D; Alt+{} and Alt+{} move.",
                            icon::ARROW_UP,
                            icon::ARROW_DOWN
                        ),
                    ));
                }
            }
        });
        if range.merges() > 0 {
            ui.label(muted(
                ui,
                format!(
                    "{} {} kept, in git's order: commits are reworded or dropped, merges \
                         reworded; nothing moves or melds.",
                    crate::format::count(u64::try_from(range.merges()).unwrap_or(u64::MAX)),
                    if range.merges() == 1 {
                        "merge is"
                    } else {
                        "merges are"
                    }
                ),
            ));
        }
    });
    event
}

/// The plan's rows, newest first.
fn rows(ui: &mut Ui, shared: &Shared<'_>, view: &mut RebaseView, range: &RebaseRange) {
    let palette = shared.palette;
    let count = view.rows.len();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("rebase-rows", shared.id))
        .auto_shrink([false, false]);
    if std::mem::take(&mut view.reveal) {
        // Keep the selected row a row clear of the edges (rows are spaced as items are).
        let pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
        let row = f32::from(u16::try_from(view.selected).unwrap_or(u16::MAX));
        let top = (row - 1.0).max(0.0) * pitch;
        let wanted = ui.available_height().max(pitch);
        let offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(Id::new(("rebase-offset", shared.id))))
            .unwrap_or(0.0);
        let bottom = (row + 2.0) * pitch - wanted;
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if bottom > offset {
            scroll = scroll.vertical_scroll_offset(bottom);
        }
    }
    let shown = scroll.show_rows(ui, ROW_HEIGHT, count, |ui, visible| {
        for at in visible {
            row(ui, view, range, at, palette);
        }
    });
    ui.ctx().data_mut(|data| {
        data.insert_temp(Id::new(("rebase-offset", shared.id)), shown.state.offset.y);
    });
}

/// One row: moving it, its step, and its commit.
fn row(
    ui: &mut Ui,
    view: &mut RebaseView,
    range: &RebaseRange,
    at: usize,
    palette: crate::theme::Palette,
) {
    let Some(planned) = view.rows.get(at).cloned() else {
        return;
    };
    let Some(commit) = range.commits.get(planned.commit) else {
        return;
    };
    let selected = view.selected == at;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let response = ui.interact(rect, Id::new(("rebase-row", commit.id)), Sense::click());
    let subject = commit.subject();
    // A commit without a message is named by its name.
    let name = if subject.trim().is_empty() {
        commit.id.abbreviated(ID_DIGITS).to_string()
    } else {
        subject.to_owned()
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name)
    });
    crate::widgets::row_fill(ui, rect, 0.0, selected, response.hovered());
    if response.clicked() {
        view.selected = at;
    }
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(6.0, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt(("rebase-row-ui", commit.id)),
    );
    let ui = &mut inner;
    move_buttons(ui, view, range, at);
    let keeping = range.merges() > 0;
    let mut kind = planned.kind;
    // In a place of one width whichever step it says: the names after it line up.
    let step = crate::widgets::column(ui, (STEP_WIDTH, ui.available_height()), Align::Min, |ui| {
        egui::ComboBox::from_id_salt(("rebase-step", commit.id))
            .icon(crate::widgets::combo_caret)
            .width(84.0)
            .selected_text(kind.name())
            .show_ui(ui, |ui| {
                for choice in Kind::ALL {
                    if choice.allowed(commit.merge, keeping) {
                        ui.selectable_value(&mut kind, choice, choice.name());
                    }
                }
            })
    });
    crate::help::note_topic(ui, step.response.rect, kind.topic());
    if kind != planned.kind {
        view.selected = at;
        view.set_kind(range, kind);
    }
    // Not selectable: a click on it selects the row.
    ui.add(
        egui::Label::new(
            RichText::new(commit.id.abbreviated(ID_DIGITS).to_string())
                .monospace()
                .color(palette.muted),
        )
        .selectable(false),
    );
    if commit.merge {
        ui.add(
            egui::Label::new(RichText::new(icon::GIT_MERGE).color(palette.muted)).selectable(false),
        )
        .on_hover_text("A merge: kept, in git's order");
    }
    let mut text = RichText::new(subject);
    text = match kind {
        Kind::Drop => text.strikethrough().color(palette.muted),
        Kind::Squash | Kind::Fixup => {
            RichText::new(format!("{}  {subject}", icon::ARROW_ELBOW_DOWN_RIGHT))
                .color(palette.muted)
        }
        Kind::Reword => {
            let reworded = planned
                .message
                .as_deref()
                .and_then(|message| message.lines().next())
                .unwrap_or(subject);
            RichText::new(reworded).italics()
        }
        Kind::Pick => text,
    };
    ui.add(egui::Label::new(text).truncate().selectable(false));
    if kind.melds() {
        ui.add(egui::Label::new(muted(ui, "into the commit below")).selectable(false));
    }
}

/// A row's Move up and Move down (disabled in a range holding merges: its order is git's).
fn move_buttons(ui: &mut Ui, view: &mut RebaseView, range: &RebaseRange, at: usize) {
    let last = at.saturating_add(1) == view.rows.len();
    let keeping = range.merges() > 0;
    for (glyph, name, enabled, up) in [
        (icon::ARROW_UP, "Move up", at > 0 && !keeping, true),
        (icon::ARROW_DOWN, "Move down", !last && !keeping, false),
    ] {
        // Named in words, for those who don't see the arrow.
        let button = crate::widgets::icon_tool(ui, enabled, glyph, name);
        let button = if keeping {
            button.on_disabled_hover_text("The range holds merges: its order is git's")
        } else {
            button.on_hover_text(format!("{name} (Alt+{glyph})"))
        };
        if button.clicked() {
            view.selected = at;
            view.shift(range, up);
        }
    }
}

/// The selected commit's message: editable when it is reworded.
fn message(ui: &mut Ui, view: &mut RebaseView, range: &RebaseRange) {
    let Some(row) = view.rows.get_mut(view.selected) else {
        return;
    };
    let Some(commit) = range.commits.get(row.commit) else {
        return;
    };
    ui.label(muted(
        ui,
        format!("{} by {}", commit.id.abbreviated(ID_DIGITS), commit.author),
    ));
    ui.add_space(4.0);
    match row.kind {
        Kind::Reword => {
            let label = ui.label("Its new message:");
            let message = row.message.get_or_insert_with(|| commit.message.clone());
            crate::widgets::bounded_text_area(
                ui,
                crate::widgets::text_area(message)
                    .desired_width(f32::INFINITY)
                    .desired_rows(5),
                240.0,
                "rebase-reword",
            )
            .labelled_by(label.id);
        }
        kind => {
            let note = match kind {
                Kind::Squash => "Squashed: its message is added to the one below's.",
                Kind::Fixup => {
                    "Fixed up: its changes go into the commit below, its message doesn't."
                }
                Kind::Drop => "Dropped: its changes go from the branch.",
                Kind::Pick | Kind::Reword => "Reword it (R) to change its message.",
            };
            ui.label(muted(ui, note));
            egui::ScrollArea::vertical()
                .id_salt("rebase-message-text")
                .show(ui, |ui| {
                    ui.add(egui::Label::new(commit.message.as_str()).wrap());
                });
        }
    }
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use egit_git::RangeCommit;

    use super::*;

    fn range(count: u8) -> RebaseRange {
        RebaseRange {
            base: None,
            commits: (0..count)
                .map(|index| RangeCommit {
                    id: ObjectId::from_hex(format!("{index:040x}").as_bytes()).unwrap(),
                    author: "Ada".to_owned(),
                    message: format!("Commit {index}"),
                    merge: false,
                })
                .collect(),
        }
    }

    fn view(range: &RebaseRange) -> RebaseView {
        let mut view = RebaseView {
            reword: None,
            range: Bind::new(true),
            rows: Vec::new(),
            selected: 0,
            reveal: false,
            cancel: CancelToken::new(),
        };
        view.plan_from(range);
        view
    }

    #[test]
    fn a_plan_is_made_newest_first_and_handed_over_oldest_first() {
        let range = range(3);
        let mut view = view(&range);
        assert_eq!(view.plan(&range), Err("Nothing changes yet"));
        // The newest moved below the middle one, which is squashed into the oldest.
        view.shift(&range, false);
        view.selected = 0;
        view.set_kind(&range, Kind::Squash);
        let plan = view.plan(&range).unwrap();
        let steps: Vec<_> = plan
            .iter()
            .map(|planned| (planned.id, planned.step.clone()))
            .collect();
        assert_eq!(
            steps,
            [
                (range.commits[0].id, Step::Pick),
                (range.commits[2].id, Step::Pick),
                (range.commits[1].id, Step::Squash),
            ]
        );
    }

    #[test]
    fn what_git_would_refuse_is_said() {
        let range = range(2);
        let mut view = view(&range);
        view.selected = 1;
        view.set_kind(&range, Kind::Fixup);
        assert_eq!(
            view.plan(&range),
            Err("The lowest commit kept can't meld into one below it")
        );
        view.set_kind(&range, Kind::Reword);
        assert_eq!(view.rows[1].message.as_deref(), Some("Commit 0"));
        view.rows[1].message = Some("  ".to_owned());
        assert_eq!(view.plan(&range), Err("A reworded commit needs a message"));
        view.set_kind(&range, Kind::Drop);
        view.selected = 0;
        view.set_kind(&range, Kind::Drop);
        assert_eq!(
            view.plan(&range),
            Err("Every commit is dropped (reset the branch instead)")
        );
    }

    #[test]
    fn a_commit_opened_to_reword_starts_from_its_message() {
        let range = range(3);
        let mut view = view(&range);
        view.rows.clear();
        view.reword = Some(range.commits[1].id);
        view.plan_from(&range);
        assert_eq!(view.selected, 1);
        assert_eq!(view.rows[1].kind, Kind::Reword);
        assert_eq!(view.rows[1].message.as_deref(), Some("Commit 1"));
    }
}
