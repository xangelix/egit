//! A changed file, in place of the commit table: its diff (unified or side by side, with a
//! little context or all of it) or the whole file, under a bar that says how it changed and
//! goes back.
//!
//! The change is a commit's, or one in the work tree not yet committed. A work tree's file
//! follows the status (its diff is read again whenever it changes) and can be staged or
//! unstaged whole, or the lines or hunks selected in its diff. A file in conflict shows the
//! merge editor instead, until it is resolved: then its staged changes.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, ChangeKind, DiffOptions, Direction, FileChange, ObjectId, Status, TreeEntry,
    Uncommitted, WHOLE_FILE,
};
use egui::{Frame, Key, Label, Margin, Modifiers, RichText, Ui};
use egui_async::Bind;

use super::history::{ID_DIGITS, Shared};
use super::merge::{self, MergeView};
use crate::app::Action;
use crate::diff::{
    DiffDocument, LfsChange, SplitDiff, read_diff, read_uncommitted_diff, split_diff,
};
use crate::format;
use crate::grid::TextGrid;
use crate::history::{
    CommitDetails, FileContent, LfsPointer, Objects, read_file, read_work_tree_file,
};
use crate::problem::Problem;
use crate::staging::{LinesRequest, StageRequest};
use crate::tables::change_name;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// Whose change a file view shows.
#[derive(Debug)]
enum Source {
    /// A commit's, against its first parent.
    Commit {
        commit: ObjectId,
        parent: Option<ObjectId>,
        change: FileChange,
    },
    /// One in the work tree, not committed yet.
    WorkTree(WorkFile),
}

/// A file with uncommitted changes, and which of them a view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WorkFile {
    pub(super) path: BString,
    /// A staged rename's or copy's old path.
    pub(super) from: Option<BString>,
    /// Which changes: staged, not staged, or a file not tracked.
    pub(super) which: Uncommitted,
    /// How it changed, in a word or two.
    pub(super) change: &'static str,
    /// What the index held at the path when its diff was read (lines are staged against it).
    pub(super) entry: Option<TreeEntry>,
    /// Whether it is in conflict, mid-merge (its diff isn't one egit reads).
    pub(super) conflict: bool,
}

impl WorkFile {
    /// The way staging moves this side's changes.
    const fn direction(&self) -> Direction {
        match self.which {
            Uncommitted::Unstaged | Uncommitted::Untracked => Direction::Stage,
            Uncommitted::Staged => Direction::Unstage,
        }
    }

    /// Its paths, a rename's old one first.
    fn paths(&self) -> Vec<BString> {
        self.from
            .iter()
            .cloned()
            .chain(std::iter::once(self.path.clone()))
            .collect()
    }

    /// This side of the file in `status`, if it still has changes there.
    pub(super) fn find(status: &Status, path: &BStr, which: Uncommitted) -> Option<Self> {
        let entry = status.entries().find(|entry| entry.path == path)?;
        let side = match which {
            Uncommitted::Staged => entry.is_staged(),
            Uncommitted::Unstaged => {
                entry.is_unstaged() && entry.kind != egit_git::EntryKind::Untracked
            }
            Uncommitted::Untracked => entry.kind == egit_git::EntryKind::Untracked,
        };
        side.then(|| Self::of(entry, which))
    }

    /// The `which` side of `entry`.
    pub(super) fn of(entry: egit_git::StatusEntry<'_>, which: Uncommitted) -> Self {
        Self {
            path: entry.path.to_owned(),
            from: (which == Uncommitted::Staged)
                .then(|| entry.from.map(ToOwned::to_owned))
                .flatten(),
            which,
            change: super::work::change_name(entry, which),
            entry: entry.index(),
            conflict: entry.kind == egit_git::EntryKind::Unmerged,
        }
    }
}

/// A changed file: its diff, or the file as the change left it (as it was, if it was deleted).
#[derive(Debug)]
pub(super) struct FileView {
    source: Source,
    /// The status a work tree's file was last read for.
    status_seen: Option<Arc<Status>>,
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
    /// The merge editor, for a work tree's file in conflict.
    merge: Option<MergeView>,
    /// What its bar asked to see of the file instead: its blame, or its history.
    pub(super) wanted: Option<FileWanted>,
}

/// Another way of looking at a file, asked from its bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FileWanted {
    /// Its blame at a commit (`None`: the work tree).
    Blame { at: Option<ObjectId>, path: BString },
    /// The commits that changed it.
    History(BString),
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
        let source = Source::Commit {
            commit: details.id,
            parent: details.commit.parents.first().copied(),
            change,
        };
        Self::with_source(shared, source, previous)
    }

    /// `work`'s uncommitted changes, shown as `previous` was.
    pub(super) fn open_work(shared: &Shared<'_>, work: WorkFile, previous: Option<&Self>) -> Self {
        let mut file = Self::with_source(shared, Source::WorkTree(work), previous);
        file.status_seen = shared.work_status.cloned();
        file
    }

    fn with_source(shared: &Shared<'_>, source: Source, previous: Option<&Self>) -> Self {
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
            source,
            status_seen: None,
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
            merge: None,
            wanted: None,
        };
        match &file.source {
            // A conflict's diff isn't one egit reads: it is merged instead.
            Source::WorkTree(work) if work.conflict => {
                file.merge = Some(MergeView::new(shared, work.path.clone(), wrap));
            }
            _ => file.read_diff(shared),
        }
        file
    }

    /// Reads the diff (again), with as much context as asked for; a read under way stops.
    fn read_diff(&mut self, shared: &Shared<'_>) {
        let Some(git) = shared.git else {
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        // A split that failed is tried again for this diff; a good one shows until it lands.
        if self.sides.is_err() {
            self.sides = Bind::new(true);
        }
        let options = DiffOptions {
            context: if self.whole { WHOLE_FILE } else { 3 },
            ..DiffOptions::default()
        };
        let (git, repository, cancel) = (
            git.clone(),
            Arc::clone(shared.repository),
            self.cancel.clone(),
        );
        match &self.source {
            Source::Commit {
                commit,
                parent,
                change,
            } => {
                let mut paths = Vec::with_capacity(2);
                if let ChangeKind::Renamed { from, .. } | ChangeKind::Copied { from, .. } =
                    &change.kind
                {
                    paths.push(from.clone());
                }
                paths.push(change.path.clone());
                self.diff.request(read_diff(
                    git, repository, *commit, *parent, paths, options, cancel,
                ));
            }
            Source::WorkTree(work) => {
                self.diff.request(read_uncommitted_diff(
                    git,
                    repository,
                    work.paths(),
                    work.which,
                    options,
                    cancel,
                ));
            }
        }
    }

    /// For a work tree's file, reads its diff again when the status changed (the file was
    /// edited, or staged), taking the index's entry it now has.
    fn follow_status(&mut self, shared: &Shared<'_>) {
        let Source::WorkTree(work) = &mut self.source else {
            return;
        };
        let Some(status) = shared.work_status else {
            return;
        };
        if self
            .status_seen
            .as_ref()
            .is_some_and(|seen| Arc::ptr_eq(seen, status))
        {
            return;
        }
        self.status_seen = Some(Arc::clone(status));
        let entry = status.entries().find(|entry| entry.path == work.path);
        let in_conflict = entry.is_some_and(|entry| entry.kind == egit_git::EntryKind::Unmerged);
        if work.conflict && in_conflict {
            // Still in conflict: read again (another program may have changed its sides).
            if let Some(merge) = &mut self.merge {
                merge.read(shared);
            }
            return;
        }
        if work.conflict {
            // Resolved: what it now stages.
            self.merge = None;
            if let Some(staged) = WorkFile::find(status, work.path.as_ref(), Uncommitted::Staged) {
                *work = staged;
            }
        } else if let Some(now) = WorkFile::find(status, work.path.as_ref(), work.which) {
            *work = now;
        } else {
            work.entry = entry.and_then(egit_git::StatusEntry::index);
        }
        self.content = Bind::new(true);
        self.read_diff(shared);
    }

    /// The path it shows.
    fn path(&self) -> &BStr {
        match &self.source {
            Source::Commit { change, .. } => change.path.as_ref(),
            Source::WorkTree(work) => work.path.as_ref(),
        }
    }

    /// Where its blame is: the commit it shows (or, deleted there, its parent); the work tree
    /// for a work tree's file (or `HEAD`, deleted there).
    fn blame_target(&self, shared: &Shared<'_>) -> FileWanted {
        let path = self.path().to_owned();
        let at = match &self.source {
            Source::Commit {
                commit,
                parent,
                change,
            } => {
                if change.kind == ChangeKind::Deleted {
                    *parent
                } else {
                    Some(*commit)
                }
            }
            Source::WorkTree(work) if work.change == "Deleted" => shared.head_commit(),
            Source::WorkTree(_) => None,
        };
        FileWanted::Blame { at, path }
    }

    /// The work tree's file it shows, if it shows one.
    pub(super) const fn work(&self) -> Option<&WorkFile> {
        match &self.source {
            Source::WorkTree(work) => Some(work),
            Source::Commit { .. } => None,
        }
    }
}

impl FileView {
    /// The commit whose change it shows (none for the work tree's).
    pub(super) const fn commit(&self) -> Option<ObjectId> {
        match &self.source {
            Source::Commit { commit, .. } => Some(*commit),
            Source::WorkTree(_) => None,
        }
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
        (self.path().to_string(), ready)
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

/// A changed file's diff or contents, under a bar to go back to the commit table (and, for the
/// work tree's, one to stage or unstage); returns whether it stays open.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    objects: &Objects,
    file: &mut FileView,
    actions: &mut Vec<Action>,
) -> bool {
    file.follow_status(shared);
    // Escape closes a find bar or clears a selection first (the grid takes it), then closes
    // the view.
    let selecting = [&file.diff_grid, &file.sides_grid, &file.file_grid]
        .iter()
        .any(|grid| grid.has_selection() || grid.is_finding())
        || file.merge.as_ref().is_some_and(MergeView::takes_escape);
    let mut open =
        selecting || !ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    file_bar(ui, shared, file, &mut open);
    work_bar(ui, shared, file, actions);
    if let Some(merging) = &mut file.merge {
        merge::show(ui, shared, merging, actions);
        return open;
    }
    match file.shown {
        Shown::Diff => show_diff(ui, file),
        Shown::File => show_file(ui, shared, objects, file),
    }
    open
}

/// The bar over a file view: back, the file's blame and history, and what to show; under
/// them how the file changed. Each line wraps, so nothing overlaps however narrow it is.
fn file_bar(ui: &mut Ui, shared: &Shared<'_>, file: &mut FileView, open: &mut bool) {
    let palette = shared.palette;
    Frame::new()
        .fill(palette.panel)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked()
                {
                    *open = false;
                }
                if widgets::secondary(ui, true, "Blame")
                    .on_hover_text("Which commit each of its lines came from")
                    .clicked()
                {
                    file.wanted = Some(file.blame_target(shared));
                }
                if widgets::secondary(ui, true, "File history")
                    .on_hover_text("Every commit that changed it")
                    .clicked()
                {
                    file.wanted = Some(FileWanted::History(file.path().to_owned()));
                }
                ui.add_space(8.0);
                shown_controls(ui, shared, file);
            });
            ui.horizontal_wrapped(|ui| change_line(ui, palette, file));
        });
}

/// What the file view shows: its diff or the file, wrapped or not, and (for a diff) its whole
/// file around it and side by side. Those that come and go come last.
fn shown_controls(ui: &mut Ui, shared: &Shared<'_>, file: &mut FileView) {
    if file.merge.is_none() {
        ui.selectable_value(&mut file.shown, Shown::Diff, "Diff");
        ui.selectable_value(&mut file.shown, Shown::File, "File");
    }
    let mut wrap = file.diff_grid.wraps();
    if ui.checkbox(&mut wrap, "Wrap lines").changed() {
        file.diff_grid.set_wrap(wrap);
        file.sides_grid.set_wrap(wrap);
        file.file_grid.set_wrap(wrap);
        if let Some(merging) = &mut file.merge {
            merging.grid.set_wrap(wrap);
        }
    }
    if file.merge.is_some() || file.shown != Shown::Diff {
        return;
    }
    ui.checkbox(&mut file.split, "Side by side");
    if ui.checkbox(&mut file.whole, "Whole file").changed() {
        file.read_diff(shared);
    }
}

/// How the file changed: the change, its path (and where it came from), and its lines added
/// and removed.
fn change_line(ui: &mut Ui, palette: Palette, file: &mut FileView) {
    let (change, from) = match &file.source {
        Source::Commit { change, .. } => (
            change_name(&change.kind).to_owned(),
            match &change.kind {
                ChangeKind::Renamed { from, similarity }
                | ChangeKind::Copied { from, similarity } => Some(format!(
                    "from {} ({similarity}% alike)",
                    from.to_str_lossy()
                )),
                _ => None,
            },
        ),
        Source::WorkTree(work) => (
            format!("{} · {}", side_name(work.which), work.change),
            work.from
                .as_ref()
                .map(|from| format!("from {}", from.to_str_lossy())),
        ),
    };
    ui.label(RichText::new(change).color(palette.muted));
    ui.add(Label::new(RichText::new(file.path().to_str_lossy()).strong()).truncate());
    if let Some(from) = from {
        ui.label(muted(ui, from));
    }
    if let Some(Ok(diff)) = file.diff.read_as_ref() {
        let (added, removed) = diff.changed();
        ui.label(RichText::new(format!("+{added}")).color(palette.ok));
        ui.label(RichText::new(format!("−{removed}")).color(palette.danger));
    }
}

/// Which uncommitted changes, for people.
const fn side_name(which: Uncommitted) -> &'static str {
    match which {
        Uncommitted::Unstaged => "Not staged",
        Uncommitted::Staged => "Staged",
        Uncommitted::Untracked => "Untracked",
    }
}

/// A work tree's file's second bar: staging or unstaging it whole, or the lines or hunks
/// selected in its diff.
fn work_bar(ui: &mut Ui, shared: &Shared<'_>, file: &mut FileView, actions: &mut Vec<Action>) {
    let Some(work) = file.work().cloned() else {
        return;
    };
    let direction = work.direction();
    let verb = match direction {
        Direction::Stage => "Stage",
        Direction::Unstage => "Unstage",
    };
    let mut lines = picked(file, false);
    let mut hunks = picked(file, true);
    let has_lines = lines.is_some();
    let mut request = None;
    Frame::new()
        .fill(shared.palette.panel)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                if widgets::secondary(ui, true, &format!("{verb} file")).clicked() {
                    request = Some(StageRequest::Files {
                        paths: work.paths(),
                        direction,
                    });
                }
                if work.conflict {
                    ui.label(muted(
                        ui,
                        "Staging it as it is marks it resolved (as after resolving it in an \
                         editor of your own).",
                    ));
                    return;
                }
                if widgets::secondary(ui, has_lines, &format!("{verb} lines")).clicked() {
                    request = lines.take().map(|picked| lines_request(&work, picked));
                }
                if widgets::secondary(ui, hunks.is_some(), &format!("{verb} hunk")).clicked() {
                    request = hunks.take().map(|picked| lines_request(&work, picked));
                }
                let hint = if file.split && file.shown == Shown::Diff {
                    "Lines are picked in the unified diff (side by side off).".to_owned()
                } else if !has_lines {
                    format!(
                        "Select lines in the diff to {} only those.",
                        verb.to_lowercase()
                    )
                } else {
                    String::new()
                };
                if !hint.is_empty() {
                    ui.label(muted(ui, hint));
                }
            });
        });
    if let Some(request) = request {
        file.diff_grid.clear_selection();
        actions.push(Action::Stage(shared.id, request));
    }
}

/// The lines of the shown diff the selection picks (with `hunks`, all those of the hunks it
/// reaches into): its diff, and which of its lines; `None` when it picks no change.
fn picked(file: &mut FileView, hunks: bool) -> Option<(Arc<DiffDocument>, Vec<bool>)> {
    if file.shown != Shown::Diff || file.split {
        return None;
    }
    let (_, lines) = file.diff_grid.selected_lines()?;
    let Some(Ok(diff)) = file.diff.read_as_ref() else {
        return None;
    };
    let picked = diff.changes_in(lines, hunks);
    picked.contains(&true).then(|| (Arc::clone(diff), picked))
}

/// The request to move `picked` lines of `work`'s diff.
fn lines_request(work: &WorkFile, (diff, picked): (Arc<DiffDocument>, Vec<bool>)) -> StageRequest {
    StageRequest::Lines(Box::new(LinesRequest {
        path: work.path.clone(),
        direction: work.direction(),
        diff: diff.file_diff(),
        picked,
        entry: work.entry,
    }))
}

/// Starts reading the file's content unless it was: as the commit left it, or as the index or
/// the work tree holds it. Returns a note on which, or why there is nothing to read.
fn read_content(
    file: &mut FileView,
    git: &egit_git::Git,
    shared: &Shared<'_>,
    objects: &Objects,
) -> Result<Option<&'static str>, &'static str> {
    let repository = Arc::clone(shared.repository);
    let idle = file.content.is_idle();
    let (request, note) = match &file.source {
        Source::Commit { change, .. } => (
            change.new.or(change.old).map(|entry| {
                read_file(
                    git.clone(),
                    repository,
                    objects.clone(),
                    entry,
                    change.path.clone(),
                )
            }),
            change
                .new
                .is_none()
                .then_some("As it was before this commit deleted it."),
        ),
        Source::WorkTree(work) if work.which == Uncommitted::Staged => {
            let entry = work.entry.ok_or("Its deletion is staged.")?;
            let read = read_file(
                git.clone(),
                repository,
                objects.clone(),
                entry,
                work.path.clone(),
            );
            (Some(read), Some("As the index holds it."))
        }
        // The work tree's copy is read another way (a future of another type): at once.
        Source::WorkTree(work) => {
            if work.change == "Deleted" {
                return Err("It is deleted in the work tree.");
            }
            if idle {
                file.content
                    .request(read_work_tree_file(repository, work.path.clone()));
            }
            return Ok(Some("As the work tree has it."));
        }
    };
    if idle && let Some(read) = request {
        file.content.request(read);
    }
    Ok(note)
}

/// A placeholder in place of what can't be shown as text.
pub(super) fn placeholder(ui: &mut Ui, text: String) {
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

/// What the commit (or the work tree) changed in the file.
fn show_diff(ui: &mut Ui, file: &mut FileView) {
    // (A file in conflict shows the merge editor instead.)
    let work = file.work().map(|work| work.which);
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
            let text = match work {
                Some(Uncommitted::Staged) => "Nothing staged in the file's text.",
                Some(_) => "Nothing left to stage in the file's text.",
                None => "Nothing changed in the file's text (only its mode, say).",
            };
            placeholder(ui, text.to_owned());
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

/// The whole file, read the first time it is shown: as the commit left it, or as the work
/// tree or the index holds it.
fn show_file(ui: &mut Ui, shared: &Shared<'_>, objects: &Objects, file: &mut FileView) {
    let Some(git) = shared.git else {
        return;
    };
    match read_content(file, git, shared, objects) {
        Ok(Some(note)) => {
            Frame::new()
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(muted(ui, note));
                });
        }
        Ok(None) => {}
        Err(absent) => {
            placeholder(ui, absent.to_owned());
            return;
        }
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
