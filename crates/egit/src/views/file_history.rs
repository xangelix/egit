//! A file's history, in place of the commit table: the commits that changed it, newest first,
//! following renames, listed as git finds them; the selected one's diff of the file below,
//! from where the history shows that commit, or the file's blame there.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bstr::{BString, ByteSlice as _};
use egit_git::{CancelToken, DiffOptions, FileCommit, file_log};
use egui::{Frame, Id, Key, Margin, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;
use parking_lot::Mutex;

use super::blame::BlameEvent;
use super::file::placeholder;
use super::history::{ID_DIGITS, Shared};
use crate::blame::compact_age;
use crate::diff::{DiffDocument, read_diff};
use crate::format;
use crate::grid::TextGrid;
use crate::jobs;
use crate::problem::Problem;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// A row's height in the list.
const ROW_HEIGHT: f32 = 24.0;

/// The commits found so far, and whether the walk is done (or what went wrong).
#[derive(Debug, Default)]
struct Found {
    commits: Mutex<Vec<FileCommit>>,
    done: AtomicBool,
    problem: Mutex<Option<Problem>>,
}

/// The file history view's state.
#[derive(Debug)]
pub(super) struct FileHistoryView {
    path: BString,
    found: Arc<Found>,
    selected: Option<usize>,
    /// Scroll the selected row into view next frame.
    reveal: bool,
    diff: Bind<Arc<DiffDocument>, Problem>,
    /// The commit whose diff is read (or being read).
    diff_of: Option<usize>,
    grid: TextGrid,
    cancel: CancelToken,
    diff_cancel: CancelToken,
}

impl Drop for FileHistoryView {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.diff_cancel.cancel();
    }
}

impl FileHistoryView {
    /// The history of `path`, being found.
    pub(super) fn new(shared: &Shared<'_>, path: BString) -> Self {
        let found = Arc::new(Found::default());
        let cancel = CancelToken::new();
        if let Some(git) = shared.git {
            let (git, repository) = (git.clone(), Arc::clone(shared.repository));
            let (finding, walked, stop) = (Arc::clone(&found), path.clone(), cancel.clone());
            jobs::detached(move || {
                let walk = file_log(&git, &repository, walked.as_ref(), &stop, |commit| {
                    finding.commits.lock().push(commit);
                });
                if let Err(error) = walk
                    && !stop.is_cancelled()
                {
                    *finding.problem.lock() =
                        Some(Problem::git("The file's history couldn't be read", &error));
                }
                finding.done.store(true, Ordering::Release);
            });
        }
        let mut grid = TextGrid::new(("file-history-diff", shared.id));
        grid.set_wrap(false);
        Self {
            path,
            found,
            selected: None,
            reveal: false,
            diff: Bind::new(true),
            diff_of: None,
            grid,
            cancel,
            diff_cancel: CancelToken::new(),
        }
    }

    /// The diff shown, as lines it adds and removes (`None` until it is read).
    #[cfg(test)]
    pub(super) fn diff_changed(&mut self) -> Option<(u32, u32)> {
        self.diff
            .read_as_ref()?
            .as_ref()
            .ok()
            .map(|diff| diff.changed())
    }

    /// Reads the selected commit's diff of the file, if it isn't read yet.
    fn read_selected(&mut self, shared: &Shared<'_>) {
        let Some(at) = self.selected else {
            return;
        };
        if self.diff_of == Some(at) {
            return;
        }
        let Some(commit) = self.found.commits.lock().get(at).cloned() else {
            return;
        };
        let Some(git) = shared.git else {
            return;
        };
        self.diff_cancel.cancel();
        self.diff_cancel = CancelToken::new();
        self.diff_of = Some(at);
        let paths: Vec<BString> = commit.from.into_iter().chain([commit.path]).collect();
        self.diff.request(read_diff(
            git.clone(),
            Arc::clone(shared.repository),
            commit.id,
            commit.parent,
            paths,
            DiffOptions::default(),
            self.diff_cancel.clone(),
        ));
    }
}

/// Draws the file's history; returns what it asks of the history view.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut FileHistoryView,
) -> Option<BlameEvent> {
    let escape = !view.grid.has_selection()
        && !view.grid.is_finding()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(BlameEvent::Close);
    let done = view.found.done.load(Ordering::Acquire);
    if !done {
        // The list grows on its own.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    let count = view.found.commits.lock().len();
    keys(ui, view, count);
    if view.selected.is_none() && count > 0 {
        view.selected = Some(0);
    }
    view.read_selected(shared);
    if let Some(asked) = bar(ui, shared, view, done, count) {
        event = Some(asked);
    }
    egui::Panel::top(Id::new(("file-history-list", shared.id)))
        .resizable(true)
        .default_size(220.0)
        .size_range(96.0..=600.0)
        .frame(Frame::new())
        .show(ui, |ui| list(ui, shared, view, count));
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| match view.diff.read_as_ref() {
            Some(Ok(diff)) if diff.binary() => {
                placeholder(
                    ui,
                    "A binary file: there are no lines to compare.".to_owned(),
                );
            }
            Some(Ok(diff)) if diff.changed() == (0, 0) => {
                let renamed = view
                    .diff_of
                    .and_then(|at| {
                        view.found
                            .commits
                            .lock()
                            .get(at)
                            .map(|commit| commit.from.clone())
                    })
                    .flatten();
                let text = renamed.map_or_else(
                    || "Nothing changed in the file's text (only its mode, say).".to_owned(),
                    |from| format!("Renamed from {}; its text unchanged.", from.to_str_lossy()),
                );
                placeholder(ui, text);
            }
            Some(Ok(diff)) => {
                let diff = Arc::clone(diff);
                view.grid.show(ui, &*diff);
            }
            Some(Err(problem)) => {
                let problem = problem.clone();
                Frame::new()
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            }
            None if view.selected.is_some() => {
                Frame::new()
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| widgets::working(ui, "Reading the diff…"));
            }
            None => {}
        });
    event
}

/// ↑ and ↓ step through the commits, while no text field has the focus.
fn keys(ui: &Ui, view: &mut FileHistoryView, count: usize) {
    if ui.ctx().text_edit_focused() || count == 0 {
        return;
    }
    let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
    let at = view.selected.unwrap_or(0);
    if pressed(Key::ArrowUp) {
        view.selected = Some(at.saturating_sub(1));
        view.reveal = true;
    } else if pressed(Key::ArrowDown) {
        view.selected = Some(at.saturating_add(1).min(count.saturating_sub(1)));
        view.reveal = true;
    }
}

/// The bar: back, which file, how many commits so far, and where to go from the selected one.
fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &FileHistoryView,
    done: bool,
    count: usize,
) -> Option<BlameEvent> {
    let mut event = None;
    let selected = view
        .selected
        .and_then(|at| view.found.commits.lock().get(at).cloned());
    let problem = view.found.problem.lock().clone();
    Frame::new()
        .fill(shared.palette.panel)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked()
                {
                    event = Some(BlameEvent::Close);
                }
                ui.label(RichText::new("History of").color(shared.palette.muted));
                ui.add(
                    egui::Label::new(RichText::new(view.path.to_str_lossy()).strong()).truncate(),
                );
                let commits = match count {
                    1 => "1 commit".to_owned(),
                    count => format!(
                        "{} commits",
                        format::count(u64::try_from(count).unwrap_or(u64::MAX))
                    ),
                };
                ui.label(muted(ui, commits));
                if !done {
                    ui.spinner();
                }
                if let Some(problem) = problem {
                    ui.label(
                        RichText::new(format!("{}  {problem}", icon::WARNING_CIRCLE))
                            .color(shared.palette.danger),
                    );
                }
                if let Some(commit) = selected {
                    ui.add_space(12.0);
                    if widgets::secondary(ui, true, "Show the commit").clicked() {
                        event = Some(BlameEvent::ShowCommit(commit.id));
                    }
                    if commit.status != b'D'
                        && widgets::secondary(ui, true, "Blame it here").clicked()
                    {
                        event = Some(BlameEvent::Blame {
                            at: commit.id,
                            path: commit.path,
                        });
                    }
                }
            });
        });
    event
}

/// The commits, newest first, drawn as far as they show.
fn list(ui: &mut Ui, shared: &Shared<'_>, view: &mut FileHistoryView, count: usize) {
    let palette = shared.palette;
    let now = format::now();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("file-history-rows", shared.id))
        .auto_shrink([false, false]);
    if std::mem::take(&mut view.reveal)
        && let Some(at) = view.selected
    {
        let pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
        let row = f32::from(u16::try_from(at).unwrap_or(u16::MAX));
        let offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(Id::new(("file-history-offset", shared.id))))
            .unwrap_or(0.0);
        let top = (row - 1.0).max(0.0) * pitch;
        let bottom = (row + 2.0) * pitch - ui.available_height().max(pitch);
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if bottom > offset {
            scroll = scroll.vertical_scroll_offset(bottom);
        }
    }
    let commits = view.found.commits.lock();
    let shown = scroll.show_rows(ui, ROW_HEIGHT, count, |ui, visible| {
        for at in visible {
            let Some(commit) = commits.get(at) else {
                continue;
            };
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            let response = ui.interact(rect, Id::new(("file-commit", commit.id)), Sense::click());
            let selected = view.selected == Some(at);
            let subject = if commit.subject.trim().is_empty() {
                commit.id.abbreviated(ID_DIGITS).to_string()
            } else {
                commit.subject.clone()
            };
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    &subject,
                )
            });
            if selected {
                ui.painter()
                    .rect_filled(rect, 0.0, palette.accent.gamma_multiply(0.22));
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 0.0, palette.surface_alt);
            }
            if response.clicked() {
                view.selected = Some(at);
            }
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            // Not selectable: a click on it selects the row.
            row.add(
                egui::Label::new(
                    RichText::new(commit.id.abbreviated(ID_DIGITS).to_string())
                        .monospace()
                        .color(palette.muted),
                )
                .selectable(false),
            );
            let age = u64::try_from(commit.time)
                .map_or_else(|_| String::new(), |time| compact_age(time, now));
            row.add_sized(
                [36.0, ROW_HEIGHT],
                egui::Label::new(muted(&row, age)).selectable(false),
            );
            row.add(
                egui::Label::new(subject.as_str())
                    .truncate()
                    .selectable(false),
            );
            let mut note = commit.author.clone();
            if let Some(from) = &commit.from {
                let _ = write!(note, " · from {}", from.to_str_lossy());
            }
            row.add(
                egui::Label::new(muted(&row, note))
                    .truncate()
                    .selectable(false),
            );
        }
    });
    drop(commits);
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            Id::new(("file-history-offset", shared.id)),
            shown.state.offset.y,
        );
    });
}
