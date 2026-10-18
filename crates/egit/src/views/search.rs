//! The history's search bar, over the commit table (⌘F, or the top bar's Search): what to look
//! at (a commit's message, its author, a path it changed, its name), the query, what was found
//! and which of it is selected, and moving through it (Enter and Shift+Enter, Previous and
//! Next), each find selected in turn. Escape or Close ends it.
//!
//! A search starts once typing pauses, replacing the one before; its first find is selected as
//! it comes. It runs again on a history read again, keeping the selection.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use egit_git::ConfigSnapshot;
use egui::{Align, Key, Layout, RichText, Ui};

use crate::format::count;
use crate::history::History;
use crate::search::{self, Asked, Found, RowIndex, Search, SearchKind};
use crate::theme::{Palette, icon};
use crate::widgets;

use super::history::Shared;

/// How long typing must pause before a search starts, in seconds.
const PAUSE: f64 = 0.3;

/// How often a running search's progress is drawn.
const PROGRESS_EVERY: Duration = Duration::from_millis(60);

/// The search bar's state.
#[derive(Debug, Default)]
pub(super) struct HistorySearch {
    open: bool,
    /// Whether the field takes the focus next frame.
    focus: bool,
    kind: SearchKind,
    query: String,
    /// When the query or what it looks at last changed (egui's time), until its search starts.
    edited: Option<f64>,
    search: Option<Search>,
    /// The rows by name for the history shown: built by the first search needing it.
    index: Arc<OnceLock<RowIndex>>,
    /// Whether the search's first find has been selected (or needn't be).
    moved: bool,
}

/// Which way to move through what was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Next,
    Previous,
}

impl HistorySearch {
    /// Opens the bar, its field focused.
    pub(super) const fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    /// Whether the bar is open.
    pub(super) const fn is_open(&self) -> bool {
        self.open
    }

    /// Closes the bar: its search stops, and nothing is marked any more.
    pub(super) fn close(&mut self) {
        self.open = false;
        self.search = None;
        self.edited = None;
    }

    /// What the search has found so far, to mark the rows.
    pub(super) fn found(&self) -> Option<&Found> {
        self.search.as_ref().map(|search| &**search.found())
    }

    /// The history was read again: its rows moved, so the search runs again, the selection
    /// kept where it is.
    pub(super) fn history_replaced(&mut self) {
        self.index = Arc::new(OnceLock::new());
        if self.search.take().is_some() {
            self.edited = Some(f64::NEG_INFINITY);
            self.moved = true;
        }
    }

    /// Starts the search typed once typing has paused and the history is read whole.
    fn search_when_ready(&mut self, ui: &Ui, shared: &Shared<'_>, history: &History) {
        let Some(edited) = self.edited else {
            return;
        };
        let query = self.query.trim();
        if query.is_empty() {
            self.search = None;
            self.edited = None;
            return;
        }
        let now = ui.input(|input| input.time);
        let waited = now - edited;
        if waited < PAUSE {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(PAUSE - waited));
            return;
        }
        // Rows still coming, or (for a path) whether the trees are all here not yet known.
        let Some(git) = shared.git else {
            return;
        };
        if history.is_loading() || (self.kind == SearchKind::Path && shared.config.is_none()) {
            return;
        }
        let asked = Asked {
            git,
            repository: shared.repository,
            names: history.names(),
            index: &self.index,
            every_tree: shared.config.is_some_and(ConfigSnapshot::has_every_tree),
            kind: self.kind,
            query: if self.kind == SearchKind::Message || self.kind == SearchKind::Author {
                &self.query
            } else {
                query
            },
        };
        self.search = Some(search::start(asked, ui.ctx()));
        self.edited = None;
    }

    /// The row to move to, a step from `selected`.
    fn step(&self, step: Step, selected: Option<u64>) -> Option<u64> {
        let found = self.found()?;
        match step {
            Step::Next => found.next(selected),
            Step::Previous => found.previous(selected),
        }
    }

    /// Where things stand: "3 of 1,204", "1,204 found", "Searching… 45%", "No commits match".
    fn status(&self, history: &History, selected: Option<u64>) -> String {
        if self.query.trim().is_empty() {
            return String::new();
        }
        let Some(found) = self.found() else {
            return if history.is_loading() {
                "Reading the history…".to_owned()
            } else {
                "Searching…".to_owned()
            };
        };
        let total = found.count();
        let place = selected
            .filter(|&row| found.contains(row))
            .map(|row| found.rank(row).saturating_add(1));
        let so_far = match (place, total) {
            (Some(place), _) => format!("{} of {}", count(place), count(total)),
            (None, 0) => String::new(),
            (None, total) => format!("{} found", count(total)),
        };
        if found.is_done() {
            return if total == 0 {
                "No commits match".to_owned()
            } else {
                so_far
            };
        }
        let progress = found.progress().map(|(read, rows)| {
            let percent = read
                .saturating_mul(100)
                .checked_div(rows.max(1))
                .unwrap_or(0);
            format!(" {percent}%")
        });
        let searching = format!("Searching…{}", progress.unwrap_or_default());
        if so_far.is_empty() {
            searching
        } else {
            format!("{searching} · {so_far}")
        }
    }
}

/// Draws the bar over the commit table, when open; returns a row to select (the search's first
/// find, or a step through what it found).
pub(super) fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &History,
    search: &mut HistorySearch,
    selected: Option<u64>,
) -> Option<u64> {
    if !search.open {
        return None;
    }
    search.search_when_ready(ui, shared, history);
    let palette = shared.palette;
    let mut step = None;
    let mut close = false;
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            let label = ui.label(
                RichText::new(format!("{}  Search", icon::MAGNIFYING_GLASS)).color(palette.muted),
            );
            let before = search.kind;
            let kind = egui::ComboBox::from_id_salt(("history-search-kind", shared.id))
                .width(96.0)
                .selected_text(search.kind.label())
                .show_ui(ui, |ui| {
                    for kind in SearchKind::ALL {
                        ui.selectable_value(&mut search.kind, kind, kind.label());
                    }
                })
                .response
                .labelled_by(label.id);
            crate::help::note_topic(ui, kind.rect, "search");
            // From the right: closing, stepping, how many; the field fills what is left.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_tool(ui, true, icon::X, "Close")
                    .on_hover_text("Close the search (Escape)")
                    .clicked()
                {
                    close = true;
                }
                let any = search.found().is_some_and(|found| found.count() > 0);
                if widgets::icon_tool(ui, any, icon::ARROW_DOWN, "Next")
                    .on_hover_text("Next (Enter)")
                    .clicked()
                {
                    step = Some(Step::Next);
                }
                if widgets::icon_tool(ui, any, icon::ARROW_UP, "Previous")
                    .on_hover_text("Previous (Shift+Enter)")
                    .clicked()
                {
                    step = Some(Step::Previous);
                }
                status(ui, search, history, selected, palette);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let field = ui
                        .add(
                            widgets::line_field(&mut search.query)
                                .hint_text(search.kind.hint())
                                .desired_width(ui.available_width().max(120.0)),
                        )
                        .labelled_by(label.id);
                    if field.changed() || search.kind != before {
                        search.edited = Some(ui.input(|input| input.time));
                        search.search = None;
                        search.moved = false;
                    }
                    if std::mem::take(&mut search.focus) {
                        field.request_focus();
                    }
                    if field.lost_focus() {
                        if ui.input(|input| input.key_pressed(Key::Escape)) {
                            close = true;
                        } else if ui.input(|input| input.key_pressed(Key::Enter)) {
                            let back = ui.input(|input| input.modifiers.shift);
                            step = Some(if back { Step::Previous } else { Step::Next });
                            search.focus = true;
                        }
                    }
                });
            });
        });
    });
    if close {
        search.close();
        return None;
    }
    if let Some(found) = search.found()
        && !found.is_done()
    {
        ui.ctx().request_repaint_after(PROGRESS_EVERY);
    }
    if let Some(step) = step {
        search.moved = true;
        return search.step(step, selected);
    }
    // The first find, from the selection on, once there is one.
    if !search.moved
        && let Some(row) = search
            .found()
            .and_then(|found| found.next(selected.and_then(|row| row.checked_sub(1))))
    {
        search.moved = true;
        return Some(row);
    }
    None
}

/// What the search found so far, or why it couldn't search: always one label, so the buttons
/// after it keep their ids.
fn status(
    ui: &mut Ui,
    search: &HistorySearch,
    history: &History,
    selected: Option<u64>,
    palette: Palette,
) {
    if let Some(problem) = search.found().and_then(Found::problem) {
        ui.label(
            RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, problem.title))
                .color(palette.danger),
        )
        .on_hover_text(problem.detail);
        return;
    }
    let text = search.status(history, selected);
    ui.add(egui::Label::new(RichText::new(text).color(palette.muted)).selectable(false));
}
