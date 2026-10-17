//! A file's blame, in place of the commit table: the file on the text grid, each run of lines
//! labelled in the gutter with the commit it came from, filling in as git finds them. Selecting
//! a line marks its commit's lines and shows that commit below, from where the history shows
//! it, or the blame of what the file was before it.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use bstr::{BString, ByteSlice as _};
use egit_git::{CancelToken, Moves, ObjectId};
use egui::{Frame, Key, Margin, Modifiers, RichText, Ui};
use egui_async::Bind;

use super::history::{ID_DIGITS, Shared};
use crate::blame::{BlameSource, Blaming, digits_of, read_blamed_file, start_blame, when};
use crate::format;
use crate::grid::{GridSource as _, TextGrid};
use crate::history::{FileContent, Objects};
use crate::problem::Problem;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// The blame view's state.
#[derive(Debug)]
pub(super) struct BlameView {
    path: BString,
    /// The commit blamed at (`None`: the work tree).
    at: Option<ObjectId>,
    /// Whose lines moved within the file are.
    moves: Moves,
    content: Bind<Arc<FileContent>, Problem>,
    blaming: Arc<Blaming>,
    grid: TextGrid,
    cancel: CancelToken,
}

impl Drop for BlameView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the blame view asks of the history view.
pub(super) enum BlameEvent {
    /// Close it.
    Close,
    /// Show this commit in the history.
    ShowCommit(ObjectId),
    /// Blame the file at another commit (and path).
    Blame { at: ObjectId, path: BString },
}

impl BlameView {
    /// The blame of `path` at `at` (`None`: the work tree), its file read and its lines blamed.
    pub(super) fn new(
        shared: &Shared<'_>,
        objects: &Objects,
        at: Option<ObjectId>,
        path: BString,
    ) -> Self {
        let mut content = Bind::new(true);
        if let Some(git) = shared.git {
            content.request(read_blamed_file(
                git.clone(),
                Arc::clone(shared.repository),
                objects.clone(),
                at,
                path.clone(),
            ));
        }
        let mut grid = TextGrid::new(("blame", shared.id));
        grid.set_wrap(false);
        let mut view = Self {
            path,
            at,
            moves: Moves::default(),
            content,
            blaming: Arc::default(),
            grid,
            cancel: CancelToken::new(),
        };
        view.restart(shared);
        view
    }

    /// Blames the file (again), stopping the blame before.
    fn restart(&mut self, shared: &Shared<'_>) {
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.blaming = shared.git.map_or_else(Arc::default, |git| {
            start_blame(
                git.clone(),
                Arc::clone(shared.repository),
                self.at,
                self.path.clone(),
                self.moves,
                self.cancel.clone(),
            )
        });
    }

    /// Where on screen the place before cell `cell` of line `line` was drawn.
    #[cfg(test)]
    pub(super) fn spot(&self, line: u32, cell: u32) -> Option<egui::Pos2> {
        self.grid.spot_position(0, line, cell)
    }

    /// Whether the blame is done.
    #[cfg(test)]
    pub(super) fn done(&self) -> bool {
        self.blaming.done.load(Ordering::Acquire)
    }

    /// The commit of the line selected, if any (its place in the blame's commits).
    fn selected(&self) -> Option<u32> {
        let (_, lines) = self.grid.selected_lines()?;
        self.blaming.state.lock().commit_of(*lines.start())
    }
}

/// Draws the blame; returns what it asks of the history view.
pub(super) fn show(ui: &mut Ui, shared: &Shared<'_>, view: &mut BlameView) -> Option<BlameEvent> {
    let escape = !view.grid.has_selection()
        && !view.grid.is_finding()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(BlameEvent::Close);
    let done = view.blaming.done.load(Ordering::Acquire);
    if !done {
        // The blame fills in on its own.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    if let Some(asked) = bar(ui, shared, view, done) {
        event = Some(asked);
    }
    let selected = view.selected();
    if let Some(commit) = selected
        && let Some(asked) = details(ui, shared, view, commit)
    {
        event = Some(asked);
    }
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| content(ui, view, selected));
    event
}

/// The file, blamed so far; or why it can't be.
fn content(ui: &mut Ui, view: &mut BlameView, selected: Option<u32>) {
    match view.content.read_as_ref() {
        None => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Reading the file…"));
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
        }
        Some(Ok(content)) => {
            let content = Arc::clone(content);
            if let FileContent::Text(file) = &*content {
                let state = view.blaming.state.lock();
                let source = BlameSource {
                    file,
                    state: &state,
                    selected,
                    now: format::now(),
                    digits: digits_of(file),
                };
                view.grid.show(ui, &source);
                // The job hands its next runs over once the frame is drawn.
                drop(state);
            } else {
                Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    ui.label(muted(ui, "It isn't text: it has no lines to blame."));
                });
            }
        }
    }
}

/// The bar: back, which file at which commit, and how far the blame has got.
fn bar(ui: &mut Ui, shared: &Shared<'_>, view: &mut BlameView, done: bool) -> Option<BlameEvent> {
    let mut event = None;
    let mut restart = false;
    let found = view.blaming.state.lock().found;
    let lines = match view.content.read_as_ref() {
        Some(Ok(content)) => match &**content {
            FileContent::Text(file) => Some(file.measure().lines()),
            _ => None,
        },
        _ => None,
    };
    let problem = view.blaming.problem.lock().clone();
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
                ui.label(RichText::new("Blame").color(shared.palette.muted));
                ui.add(
                    egui::Label::new(RichText::new(view.path.to_str_lossy()).strong()).truncate(),
                );
                let at = view.at.map_or_else(
                    || "in the work tree".to_owned(),
                    |at| format!("at {}", at.abbreviated(ID_DIGITS)),
                );
                ui.label(muted(ui, at));
                ui.add_space(8.0);
                let mut follow = view.moves == Moves::Writing;
                if ui
                    .checkbox(&mut follow, "Follow moved lines")
                    .on_hover_text(
                        "A line moved within the file is the commit's that wrote it, not the \
                         one that moved it. Slower on a large file.",
                    )
                    .changed()
                {
                    view.moves = if follow {
                        Moves::Writing
                    } else {
                        Moves::Moving
                    };
                    restart = true;
                }
                if let Some(problem) = problem {
                    ui.label(
                        RichText::new(format!("{}  {problem}", icon::WARNING_CIRCLE))
                            .color(shared.palette.danger),
                    );
                } else if !done {
                    ui.spinner();
                    let of = lines.map_or_else(String::new, |lines| {
                        format!(" of {}", format::count(u64::from(lines)))
                    });
                    ui.label(muted(
                        ui,
                        format!("Blaming… {}{of} lines", format::count(u64::from(found))),
                    ));
                }
            });
        });
    if restart {
        view.restart(shared);
    }
    event
}

/// The selected line's commit: what it is, and where to go from it.
fn details(ui: &mut Ui, shared: &Shared<'_>, view: &BlameView, commit: u32) -> Option<BlameEvent> {
    let blamed = view
        .blaming
        .state
        .lock()
        .commits
        .get(usize::try_from(commit).ok()?)?
        .clone();
    let mut event = None;
    egui::Panel::bottom(egui::Id::new(("blame-details", shared.id)))
        .frame(
            Frame::new()
                .fill(shared.palette.panel)
                .inner_margin(Margin::same(10)),
        )
        .show(ui, |ui| {
            let Some(id) = blamed.id else {
                ui.label("Not committed yet: these lines are only in the work tree.");
                return;
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(id.abbreviated(ID_DIGITS).to_string())
                        .monospace()
                        .color(shared.palette.accent),
                );
                ui.label(RichText::new(&blamed.summary).strong());
            });
            ui.label(muted(ui, format!("{}, {}", blamed.author, when(&blamed))));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if widgets::secondary(ui, true, "Show the commit").clicked() {
                    event = Some(BlameEvent::ShowCommit(id));
                }
                if let Some((before, path)) = &blamed.previous
                    && widgets::secondary(ui, true, "Blame before this commit")
                        .on_hover_text("The file as it was before this commit changed it")
                        .clicked()
                {
                    event = Some(BlameEvent::Blame {
                        at: *before,
                        path: path.clone(),
                    });
                }
                if blamed.boundary {
                    ui.label(muted(ui, "Where the history git read begins."));
                }
            });
        });
    event
}
