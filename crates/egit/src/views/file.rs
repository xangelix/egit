//! A file a commit changed, in place of the commit table: its diff (unified or side by side,
//! with a little context or all of it) or the whole file, under a bar that says how it
//! changed and goes back.

use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{CancelToken, ChangeKind, DiffOptions, FileChange, ObjectId, WHOLE_FILE};
use egui::{Align, Frame, Key, Label, Margin, Modifiers, RichText, Ui};
use egui_async::Bind;

use super::history::{ID_DIGITS, Shared};
use crate::diff::{DiffDocument, LfsChange, SplitDiff, read_diff, split_diff};
use crate::format;
use crate::grid::TextGrid;
use crate::history::{CommitDetails, FileContent, LfsPointer, Objects, read_file};
use crate::problem::Problem;
use crate::tables::change_name;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// A file a commit changed: its diff, or the file as the commit left it (as it was, if the
/// commit deleted it).
#[derive(Debug)]
pub(super) struct FileView {
    /// The commit that changed it, and that commit's first parent.
    commit: ObjectId,
    parent: Option<ObjectId>,
    change: FileChange,
    shown: Shown,
    /// Whether the diff shows the whole file, not just what changed and a little around it.
    whole: bool,
    /// Whether the diff's sides are shown side by side.
    split: bool,
    /// Whether a Git LFS pointer's own changes are shown, rather than what it stands for.
    pointer: bool,
    diff: Bind<Arc<DiffDocument>, Problem>,
    /// The diff split in two: made the first time it is shown so, and again for each diff
    /// read after (the last split shows until then).
    sides: Bind<Arc<SplitDiff>, Problem>,
    sides_grid: TextGrid,
    /// Read the first time the file is shown.
    content: Bind<Arc<FileContent>, Problem>,
    diff_grid: TextGrid,
    file_grid: TextGrid,
    /// Stops the diff's git when the view closes first.
    cancel: CancelToken,
}

impl FileView {
    /// `change` of the commit `details` describes, its diff being read; shown as `previous`
    /// (the view open before it) was: wrapped, side by side, whole, or the file.
    pub(super) fn open(
        shared: &Shared<'_>,
        details: &CommitDetails,
        change: FileChange,
        previous: Option<&Self>,
    ) -> Self {
        let wrap = previous.is_some_and(|open| open.diff_grid.wraps());
        let shown = previous.map_or(Shown::Diff, |open| open.shown);
        let whole = previous.is_some_and(|open| open.whole);
        let split = previous.is_some_and(|open| open.split);
        let mut diff_grid = TextGrid::new(("diff", shared.id));
        let mut sides_grid = TextGrid::new(("sides", shared.id));
        let mut file_grid = TextGrid::new(("file", shared.id));
        diff_grid.set_wrap(wrap);
        sides_grid.set_wrap(wrap);
        file_grid.set_wrap(wrap);
        let mut file = Self {
            commit: details.id,
            parent: details.commit.parents.first().copied(),
            change,
            shown,
            whole,
            split,
            pointer: false,
            diff: Bind::new(true),
            sides: Bind::new(true),
            sides_grid,
            content: Bind::new(true),
            diff_grid,
            file_grid,
            cancel: CancelToken::new(),
        };
        file.read_diff(shared);
        file
    }

    /// Reads the diff (again), with as much context as asked for; a read under way stops.
    fn read_diff(&mut self, shared: &Shared<'_>) {
        let Some(git) = shared.git else {
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        let mut paths = Vec::with_capacity(2);
        if let ChangeKind::Renamed { from, .. } | ChangeKind::Copied { from, .. } =
            &self.change.kind
        {
            paths.push(from.clone());
        }
        paths.push(self.change.path.clone());
        // A split that failed is tried again for this diff; a good one shows until it lands.
        if self.sides.is_err() {
            self.sides = Bind::new(true);
        }
        let options = DiffOptions {
            context: if self.whole { WHOLE_FILE } else { 3 },
            ..DiffOptions::default()
        };
        self.diff.request(read_diff(
            git.clone(),
            Arc::clone(shared.repository),
            self.commit,
            self.parent,
            paths,
            options,
            self.cancel.clone(),
        ));
    }
}

impl FileView {
    /// The commit whose change it shows.
    pub(super) const fn commit(&self) -> ObjectId {
        self.commit
    }

    /// Where on screen the place before cell `cell` of line `line` was drawn, in the view shown.
    #[cfg(test)]
    pub(super) fn spot(&self, line: u32, cell: u32) -> Option<egui::Pos2> {
        let grid = match self.shown {
            Shown::File => &self.file_grid,
            Shown::Diff if self.split => &self.sides_grid,
            Shown::Diff => &self.diff_grid,
        };
        grid.spot_position(0, line, cell)
    }

    /// Its path, and whether its contents are shown yet.
    #[cfg(test)]
    pub(super) fn shown(&mut self) -> (String, bool) {
        let ready = match self.shown {
            Shown::Diff => self.diff.is_ok() && (!self.split || self.sides.is_ok()),
            Shown::File => self.content.is_ok(),
        };
        (self.change.path.to_string(), ready)
    }

    /// How many lines its diff has, once read.
    #[cfg(test)]
    pub(super) fn diff_lines(&mut self) -> Option<u32> {
        self.diff
            .ok_ref()
            .map(|diff| crate::grid::GridSource::measure(&**diff).lines())
    }

    /// How many lines the new side of its split diff has, once split.
    #[cfg(test)]
    pub(super) fn split_lines(&mut self) -> Option<u32> {
        self.sides
            .ok_ref()
            .map(|sides| crate::grid::GridSource::measure(&sides.side(false)).lines())
    }
}

impl Drop for FileView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What a file view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shown {
    /// What the commit changed.
    Diff,
    /// The whole file.
    File,
}

/// A changed file's diff or contents, under a bar to go back to the commit table; returns
/// whether it stays open.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    objects: &Objects,
    file: &mut FileView,
) -> bool {
    // Escape closes a find bar or clears a selection first (the grid takes it), then closes
    // the view.
    let selecting = [&file.diff_grid, &file.sides_grid, &file.file_grid]
        .iter()
        .any(|grid| grid.has_selection() || grid.is_finding());
    let mut open =
        selecting || !ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    file_bar(ui, shared, file, &mut open);
    match file.shown {
        Shown::Diff => show_diff(ui, file),
        Shown::File => show_file(ui, shared, objects, file),
    }
    open
}

/// The bar over a file view: back, how the file changed, and what to show.
fn file_bar(ui: &mut Ui, shared: &Shared<'_>, file: &mut FileView, open: &mut bool) {
    let palette = shared.palette;
    Frame::new()
        .fill(palette.panel)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked()
                {
                    *open = false;
                }
                ui.add_space(8.0);
                ui.label(RichText::new(change_name(&file.change.kind)).color(palette.muted));
                ui.add(
                    Label::new(RichText::new(file.change.path.to_str_lossy()).strong()).truncate(),
                );
                if let ChangeKind::Renamed { from, similarity }
                | ChangeKind::Copied { from, similarity } = &file.change.kind
                {
                    ui.label(muted(
                        ui,
                        format!("from {} ({similarity}% alike)", from.to_str_lossy()),
                    ));
                }
                if let Some(Ok(diff)) = file.diff.read_as_ref() {
                    let (added, removed) = diff.changed();
                    ui.label(RichText::new(format!("+{added}")).color(palette.ok));
                    ui.label(RichText::new(format!("−{removed}")).color(palette.danger));
                }
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let mut wrap = file.diff_grid.wraps();
                    if ui.checkbox(&mut wrap, "Wrap lines").changed() {
                        file.diff_grid.set_wrap(wrap);
                        file.sides_grid.set_wrap(wrap);
                        file.file_grid.set_wrap(wrap);
                    }
                    if file.shown == Shown::Diff {
                        ui.checkbox(&mut file.split, "Side by side");
                    }
                    if file.shown == Shown::Diff
                        && ui.checkbox(&mut file.whole, "Whole file").changed()
                    {
                        file.read_diff(shared);
                    }
                    ui.add_space(8.0);
                    ui.selectable_value(&mut file.shown, Shown::File, "File");
                    ui.selectable_value(&mut file.shown, Shown::Diff, "Diff");
                });
            });
        });
}

/// A placeholder in place of what can't be shown as text.
fn placeholder(ui: &mut Ui, text: String) {
    Frame::new()
        .inner_margin(Margin::same(16))
        .show(ui, |ui| ui.label(muted(ui, text)));
}

/// What a Git LFS pointer's change stands for: the file's sizes before and after; returns
/// whether the pointer's own changes were asked for.
fn lfs_placeholder(ui: &mut Ui, lfs: LfsChange) -> bool {
    let size = |pointer: LfsPointer| {
        pointer
            .size
            .map_or_else(|| "size unknown".to_owned(), format::size)
    };
    let text = match (lfs.old, lfs.new) {
        (Some(old), Some(new)) => format!(
            "A Git LFS file, stored outside the repository: {} before, {} after.",
            size(old),
            size(new)
        ),
        (None, Some(new)) => format!(
            "A Git LFS file was added ({}): it is stored outside the repository.",
            size(new)
        ),
        (Some(old), None) => format!(
            "A Git LFS file was deleted ({}): it was stored outside the repository.",
            size(old)
        ),
        (None, None) => "A Git LFS file, stored outside the repository.".to_owned(),
    };
    Frame::new()
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.label(muted(ui, text));
            ui.add_space(8.0);
            ui.button("Show the pointer's changes").clicked()
        })
        .inner
}

/// What the commit changed in the file.
fn show_diff(ui: &mut Ui, file: &mut FileView) {
    match file.diff.read_as_ref() {
        None => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Reading the diff…"));
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
        }
        Some(Ok(diff)) if !file.pointer && diff.lfs().is_some() => {
            if let Some(lfs) = diff.lfs()
                && lfs_placeholder(ui, lfs)
            {
                file.pointer = true;
            }
        }
        Some(Ok(diff)) if diff.binary() => {
            placeholder(
                ui,
                "A binary file: there are no lines to compare.".to_owned(),
            );
        }
        Some(Ok(diff)) if diff.changed() == (0, 0) => {
            placeholder(
                ui,
                "Nothing changed in the file's text (only its mode, say).".to_owned(),
            );
        }
        Some(Ok(diff)) if file.split => {
            let diff = Arc::clone(diff);
            // The sides shown are another diff's after a re-read (more context, say): split
            // this one once it has landed, showing the old split meanwhile.
            let stale = file.sides.ok_ref().is_some_and(|sides| !sides.is_of(&diff));
            if (file.sides.is_idle() || stale) && !file.diff.is_pending() {
                file.sides.request(split_diff(diff));
            }
            match file.sides.read_as_ref() {
                Some(Ok(sides)) => {
                    let sides = Arc::clone(sides);
                    file.sides_grid
                        .show_panes(ui, &[&sides.side(true), &sides.side(false)]);
                }
                Some(Err(problem)) => {
                    let problem = problem.clone();
                    Frame::new()
                        .inner_margin(Margin::same(16))
                        .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
                }
                None => {
                    Frame::new()
                        .inner_margin(Margin::same(16))
                        .show(ui, |ui| widgets::working(ui, "Lining up the sides…"));
                }
            }
        }
        Some(Ok(diff)) => {
            let diff = Arc::clone(diff);
            file.diff_grid.show(ui, &*diff);
        }
    }
}

/// The whole file, read the first time it is shown.
fn show_file(ui: &mut Ui, shared: &Shared<'_>, objects: &Objects, file: &mut FileView) {
    if file.content.is_idle()
        && let Some(git) = shared.git
        && let Some(entry) = file.change.new.or(file.change.old)
    {
        file.content.request(read_file(
            git.clone(),
            Arc::clone(shared.repository),
            objects.clone(),
            entry,
            file.change.path.clone(),
        ));
    }
    if file.change.new.is_none() {
        Frame::new()
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.label(muted(ui, "As it was before this commit deleted it."));
            });
    }
    match file.content.read_as_ref() {
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
        Some(Ok(content)) => match &**content {
            FileContent::Text(text) => {
                file.file_grid.show(ui, &**text);
            }
            FileContent::Binary { size } => placeholder(
                ui,
                format!(
                    "A binary file ({}): there is no text to show.",
                    format::size(*size)
                ),
            ),
            FileContent::Lfs(LfsPointer { size }) => placeholder(
                ui,
                size.map_or_else(
                    || "A Git LFS pointer: the file itself is stored elsewhere.".to_owned(),
                    |bytes| {
                        format!(
                            "A Git LFS pointer: the file itself ({}) is stored elsewhere.",
                            format::size(bytes)
                        )
                    },
                ),
            ),
            FileContent::Submodule { commit } => placeholder(
                ui,
                format!(
                    "A submodule, at commit {}: its files are another repository's.",
                    commit.abbreviated(ID_DIGITS * 2)
                ),
            ),
            FileContent::TooLarge { size } => placeholder(
                ui,
                format!("A file of {}: too large to show.", format::size(*size)),
            ),
        },
    }
}
