//! A changed file, in place of the commit table: its diff (unified or side by side, with a
//! little context or all of it) or the whole file, under a bar that says how it changed and
//! goes back.
//!
//! The change is a commit's, or one in the work tree not yet committed. A work tree's file
//! follows the status (its diff is read again whenever it changes) and can be staged or
//! unstaged whole, or the lines or hunks selected in its diff. A file in conflict shows the
//! merge editor instead, until it is resolved: then its staged changes. An image's change shows
//! its two sides side by side (`pictures`), a binary file's other changes say what they were.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, ChangeKind, DiffOptions, Direction, FileChange, ObjectId, Status, TreeEntry,
    Uncommitted, WHOLE_FILE,
};
use egui::{Align, Color32, Frame, Key, Label, Layout, Margin, Modifiers, RichText, Ui};
use egui_async::Bind;

use super::dialogs::Dialog;
use super::history::{ID_DIGITS, Shared};
use super::merge::{self, MergeView};
use crate::app::Action;
use crate::desktop::Reveal;
use crate::diff::{
    DiffDocument, LfsChange, SplitDiff, read_diff, read_uncommitted_diff, split_diff,
};
use crate::discarding::{Discard, DiscardLines};
use crate::format;
use crate::grid::TextGrid;
use crate::history::{
    CommitDetails, FileContent, LfsPointer, Objects, read_file, read_work_tree_file,
};
use crate::markdown::{Documents, Preview, is_markdown, read_documents};
use crate::pictures::{Picture, PictureSource, Pictures, is_image, read_pictures};
use crate::problem::Problem;
use crate::staging::{LinesRequest, StageRequest};
use crate::tables::change_name;
use crate::tasks::Task;
use crate::theme::Weighted as _;
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

impl Source {
    /// The path it shows.
    fn path(&self) -> &BStr {
        match self {
            Self::Commit { change, .. } => change.path.as_ref(),
            Self::WorkTree(work) => work.path.as_ref(),
        }
    }
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
    /// What `HEAD` holds at the path (what is staged replaces it).
    pub(super) head: Option<TreeEntry>,
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
            head: entry.head(),
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
    /// An image's sides, read the first time they are shown, and their textures.
    pictures: Bind<Arc<Pictures>, Problem>,
    textures: Option<Textures>,
    /// A Markdown file's sides, read the first time they are previewed, and each preview's
    /// state (before, after).
    documents: Bind<Arc<Documents>, Problem>,
    previews: (Preview, Preview),
    /// A submodule's move and the commits it brings, read the first time it is shown.
    moved: Bind<Arc<crate::submodules::Moved>, Problem>,
    /// What its bar asked to see of the file instead: its blame, or its history.
    pub(super) wanted: Option<FileWanted>,
    /// A dialog its bar asked for: whether to discard the file.
    pub(super) asked: Option<Dialog>,
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

    /// Whether its diff shows side by side.
    #[cfg(test)]
    pub(super) const fn is_split(&self) -> bool {
        self.split
    }

    /// `work`'s uncommitted changes, shown as `previous` was.
    pub(super) fn open_work(shared: &Shared<'_>, work: WorkFile, previous: Option<&Self>) -> Self {
        let mut file = Self::with_source(shared, Source::WorkTree(work), previous);
        file.status_seen = shared.work_status.cloned();
        file
    }

    fn with_source(shared: &Shared<'_>, source: Source, previous: Option<&Self>) -> Self {
        // As the view open before it, or as preferred.
        let preferred = shared.preferences;
        let wrap = previous.map_or(preferred.files.wrap, |open| open.diff_grid.wraps());
        // A preview is a Markdown file's alone.
        let shown = match previous.map_or(Shown::Diff, |open| open.shown) {
            Shown::Preview if !is_markdown(source.path()) => Shown::Diff,
            shown => shown,
        };
        let whole = previous.is_some_and(|open| open.whole);
        let split = previous.map_or(preferred.files.split_diffs, |open| open.split);
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
            pictures: Bind::new(true),
            textures: None,
            documents: Bind::new(true),
            previews: (Preview::default(), Preview::default()),
            moved: Bind::new(true),
            wanted: None,
            asked: None,
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
        self.pictures = Bind::new(true);
        self.textures = None;
        self.documents = Bind::new(true);
        self.moved = Bind::new(true);
        self.read_diff(shared);
    }

    /// Where a submodule's change takes it, when the file is one (a gitlink on either side): a
    /// commit's two commits, or for the work tree `HEAD`'s and the index's (staged), the
    /// index's and its `HEAD` now (`None`: not staged).
    fn submodule_move(&self) -> Option<(Option<ObjectId>, Option<ObjectId>)> {
        let link =
            |entry: Option<TreeEntry>| entry.filter(|entry| entry.mode == TreeEntry::SUBMODULE);
        match &self.source {
            Source::Commit { change, .. } => {
                (link(change.old).is_some() || link(change.new).is_some()).then(|| {
                    (
                        change.old.map(|entry| entry.id),
                        change.new.map(|entry| entry.id),
                    )
                })
            }
            Source::WorkTree(work) => {
                if link(work.entry).is_none() && link(work.head).is_none() {
                    return None;
                }
                match work.which {
                    Uncommitted::Staged => Some((
                        work.head.map(|entry| entry.id),
                        work.entry.map(|entry| entry.id),
                    )),
                    Uncommitted::Unstaged => Some((work.entry.map(|entry| entry.id), None)),
                    Uncommitted::Untracked => None,
                }
            }
        }
    }

    /// Where an image's sides are: a commit's blobs, or for the work tree, `HEAD`'s and the
    /// index's (staged), the index's and the file's (not staged), or the file alone (new).
    fn picture_sources(&self) -> (Option<PictureSource>, Option<PictureSource>) {
        let blob = |entry: Option<TreeEntry>| {
            entry
                .filter(|entry| entry.mode & 0o170_000 == 0o100_000)
                .map(|entry| PictureSource::Blob(entry.id))
        };
        match &self.source {
            Source::Commit { change, .. } => (blob(change.old), blob(change.new)),
            Source::WorkTree(work) => {
                let file = || Some(PictureSource::WorkTree(work.path.clone()));
                match work.which {
                    Uncommitted::Staged => (blob(work.head), blob(work.entry)),
                    Uncommitted::Unstaged => (blob(work.entry), file()),
                    Uncommitted::Untracked => (None, file()),
                }
            }
        }
    }

    /// The path it shows.
    fn path(&self) -> &BStr {
        self.source.path()
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
            // A preview has no grid.
            Shown::Preview => return None,
        };
        grid.spot_position(0, line, cell)
    }

    /// Its path, and whether its contents are shown yet.
    #[cfg(test)]
    pub(super) fn shown(&mut self) -> (String, bool) {
        let ready = match self.shown {
            Shown::Diff => self.diff.is_ok() && (!self.split || self.sides.is_ok()),
            Shown::File => self.content.is_ok(),
            Shown::Preview => self.documents.is_ok(),
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
    /// A Markdown file as it reads, rendered.
    Preview,
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
    file_bar(ui, shared, file, &mut open, actions);
    work_bar(ui, shared, file, actions);
    if let Some(merging) = &mut file.merge {
        merge::show(ui, shared, merging, actions);
        return open;
    }
    match file.shown {
        Shown::Diff => show_diff(ui, shared, objects, file, actions),
        Shown::File => show_file(ui, shared, objects, file),
        Shown::Preview => show_preview(ui, shared, objects, file, actions),
    }
    open
}

/// The bar over a file view: back, how the file changed (and its path), and at the right the
/// work tree's file opened or shown in its folder, its blame and history; under them what to
/// show and how.
fn file_bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    file: &mut FileView,
    open: &mut bool,
    actions: &mut Vec<Action>,
) {
    let palette = shared.palette;
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT))
                .on_hover_text("Back to the history (Escape)")
                .clicked()
            {
                *open = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::tool(
                    ui,
                    icon::CLOCK_COUNTER_CLOCKWISE,
                    "File history",
                    false,
                    widgets::Look::Full,
                )
                .on_hover_text("Every commit that changed it")
                .clicked()
                {
                    file.wanted = Some(FileWanted::History(file.path().to_owned()));
                }
                if widgets::tool(ui, icon::USER_LIST, "Blame", false, widgets::Look::Full)
                    .on_hover_text("Which commit each of its lines came from")
                    .clicked()
                {
                    file.wanted = Some(file.blame_target(shared));
                }
                let reveals = [
                    (
                        icon::FOLDER_OPEN,
                        "Show in folder",
                        Reveal::InFolder,
                        "Show the work tree's file in its folder",
                    ),
                    (
                        icon::ARROW_SQUARE_OUT,
                        "Open file",
                        Reveal::Open,
                        "Open the work tree's file in its app (as it is now, not as a commit \
                         has it)",
                    ),
                ];
                for (glyph, name, how, hover) in reveals {
                    if widgets::icon_tool(ui, true, glyph, name)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        let work_tree = shared.repository.work_tree();
                        let path = crate::desktop::in_work_tree(work_tree, file.path());
                        actions.push(Action::Reveal(path, how));
                    }
                }
                // A conflict's view has no other choices: its wrapping goes here.
                if file.merge.is_some() {
                    wrap_toggle(ui, file);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    change_line(ui, palette, file);
                });
            });
        });
        if file.merge.is_none() {
            ui.horizontal_wrapped(|ui| shown_controls(ui, shared, file));
        }
    });
}

/// What the file view shows: its diff or the file, (for a diff) unified or side by side and
/// with the whole file around it, and long lines wrapped or not.
fn shown_controls(ui: &mut Ui, shared: &Shared<'_>, file: &mut FileView) {
    if file.merge.is_none() {
        let choices: &[(Shown, &str)] = if is_markdown(file.path()) {
            &[
                (Shown::Diff, "Diff"),
                (Shown::File, "File"),
                (Shown::Preview, "Preview"),
            ]
        } else {
            &[(Shown::Diff, "Diff"), (Shown::File, "File")]
        };
        widgets::segmented(ui, &mut file.shown, choices);
        // A preview has no lines to wrap.
        if file.shown == Shown::Preview {
            return;
        }
        if file.shown == Shown::Diff {
            widgets::segmented(
                ui,
                &mut file.split,
                &[(false, "Unified"), (true, "Side by side")],
            );
            if widgets::checkbox(ui, &mut file.whole, "Whole file").changed() {
                file.read_diff(shared);
            }
        }
    }
    wrap_toggle(ui, file);
}

/// Whether long lines wrap, in every grid the view has.
fn wrap_toggle(ui: &mut Ui, file: &mut FileView) {
    let mut wrap = file.diff_grid.wraps();
    if widgets::checkbox(ui, &mut wrap, "Wrap lines").changed() {
        file.diff_grid.set_wrap(wrap);
        file.sides_grid.set_wrap(wrap);
        file.file_grid.set_wrap(wrap);
        if let Some(merging) = &mut file.merge {
            merging.grid.set_wrap(wrap);
        }
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
    if let Some(Ok(diff)) = file.diff.read_as_ref() {
        let (added, removed) = diff.changed();
        ui.label(RichText::new(format!("+{added}")).color(palette.ok));
        ui.label(RichText::new(format!("−{removed}")).color(palette.danger));
    }
    ui.add(Label::new(RichText::new(file.path().to_str_lossy()).medium()).truncate());
    if let Some(from) = from {
        ui.add(Label::new(muted(ui, from)).truncate());
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
/// selected in its diff; changes not staged are discarded the same ways (the whole file once
/// asked).
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
    let mut discard = None;
    // Changes not staged are demoted by discarding them, as staged ones are by unstaging.
    let discards = direction == Direction::Stage && !work.conflict;
    let can_discard = discard_refusal(shared, &work);
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let whole = widgets::secondary(ui, true, &format!("{verb} file"));
            // A file in conflict: staging it is what resolves it.
            if work.conflict {
                crate::help::note_topic(ui, whole.rect, "conflict-stage-file");
            }
            if whole.clicked() {
                request = Some(StageRequest::Files {
                    paths: work.paths(),
                    direction,
                });
            }
            if work.conflict {
                ui.label(muted(ui, "Marks it resolved as it is now"));
                return;
            }
            if widgets::secondary(ui, has_lines, &format!("{verb} lines")).clicked() {
                request = lines.take().map(|picked| lines_request(&work, picked));
            }
            if widgets::secondary(ui, hunks.is_some(), &format!("{verb} hunk")).clicked() {
                request = hunks.take().map(|picked| lines_request(&work, picked));
            }
            if discards {
                widgets::toolbar_separator(ui);
                let button = |ui: &mut Ui, enabled: bool, words: &str| {
                    let allowed = enabled && can_discard.is_ok();
                    let response = widgets::secondary(ui, allowed, words);
                    match can_discard {
                        Err(why) if enabled => response.on_disabled_hover_text(why),
                        _ => response,
                    }
                    .clicked()
                };
                if button(ui, has_lines, "Discard lines") {
                    discard = lines
                        .take()
                        .map(|picked| discard_lines(&work, picked, false));
                }
                if button(ui, hunks.is_some(), "Discard hunk") {
                    discard = hunks
                        .take()
                        .map(|picked| discard_lines(&work, picked, true));
                }
                if button(ui, true, "Discard file…") {
                    let path = vec![work.path.clone()];
                    let (tracked, untracked) = if work.which == Uncommitted::Untracked {
                        (Vec::new(), path)
                    } else {
                        (path, Vec::new())
                    };
                    file.asked = Some(Dialog::Discard(Discard::Files { tracked, untracked }));
                }
            }
            let hint = if file.split && file.shown == Shown::Diff {
                "Lines are picked in the unified diff (side by side off).".to_owned()
            } else if !has_lines && discards {
                "Select lines in the diff to stage or discard only those.".to_owned()
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
    if let Some(discard) = discard {
        file.diff_grid.clear_selection();
        actions.push(Action::Task(shared.id, Task::Discard(discard)));
    }
}

/// Why `work`'s changes not staged can't be discarded now: a task is running (discarding is
/// one), or it is a submodule or a link (whose lines aren't a file's).
fn discard_refusal(shared: &Shared<'_>, work: &WorkFile) -> Result<(), &'static str> {
    if shared.busy {
        return Err("Another task is running");
    }
    match work.entry.map(|entry| entry.mode) {
        Some(0o160_000) => Err("A submodule: update it to the commit recorded from its page"),
        _ => Ok(()),
    }
}

/// Discarding `picked` lines (or, `hunks`, whole hunks) of `work`'s diff.
fn discard_lines(
    work: &WorkFile,
    (diff, picked): (Arc<DiffDocument>, Vec<bool>),
    hunks: bool,
) -> Discard {
    Discard::Lines(Box::new(DiscardLines {
        path: work.path.clone(),
        diff: Arc::new(diff.file_diff()),
        picked,
        entry: work.entry,
        hunks,
    }))
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
    let shown = Frame::new()
        .inner_margin(Margin::same(16))
        .show(ui, |ui| ui.label(muted(ui, text)));
    crate::help::note_topic(ui, shown.response.rect, "file-placeholder");
}

/// A submodule's change: from which commit to which, and the commits that brings (as far as it
/// is here to tell), with the way to open it.
fn show_submodule_move(
    ui: &mut Ui,
    shared: &Shared<'_>,
    file: &mut FileView,
    sides: (Option<ObjectId>, Option<ObjectId>),
) {
    crate::help::note_topic(ui, ui.max_rect(), "submodule-change");
    let path = file.path().to_owned();
    if file.moved.is_idle()
        && let Some(git) = shared.git
    {
        file.moved.request(crate::submodules::read_move(
            git.clone(),
            Arc::clone(shared.repository),
            path.clone(),
            sides,
        ));
    }
    let palette = shared.palette;
    // Where it is now, for a change not staged: its HEAD, once read.
    let now = file.moved.ok_ref().and_then(|moved| moved.to);
    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
        let short = |id: Option<ObjectId>| id.map(|id| id.abbreviated(ID_DIGITS).to_string());
        let headline = match (short(sides.0), short(sides.1)) {
            (Some(from), None) if file.work().is_some() && now.is_some() => format!(
                "The submodule {} moved from the commit recorded, {from}, to {} (its HEAD now).",
                path.to_str_lossy(),
                short(now).unwrap_or_default()
            ),
            (None, Some(to)) => format!("The submodule {} was added, at {to}.", path.to_str_lossy()),
            (Some(from), None) if file.work().is_none() => {
                format!("The submodule {} was removed (it was at {from}).", path.to_str_lossy())
            }
            (Some(from), None) => format!(
                "The submodule {} moved from the commit recorded, {from}, to where it is now.",
                path.to_str_lossy()
            ),
            (Some(from), Some(to)) => {
                format!("The submodule {} moved from {from} to {to}.", path.to_str_lossy())
            }
            (None, None) => format!("The submodule {}.", path.to_str_lossy()),
        };
        ui.add(Label::new(RichText::new(headline).semibold()).wrap());
        ui.add(
            Label::new(muted(
                ui,
                "A submodule is another repository: this one records which of its commits to check out, and changing that is all a change here holds.",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        match file.moved.read_as_ref() {
            None => widgets::working(ui, "Reading its commits…"),
            Some(Err(problem)) => {
                let problem = problem.clone();
                widgets::problem_box(ui, &problem, |_| {});
            }
            Some(Ok(moved)) => match &moved.commits {
                None => {
                    ui.add(
                        Label::new(muted(
                            ui,
                            "Its commits can't be listed: it isn't set up here, or doesn't have them. Set it \
                             up (the Submodules menu) to see them.",
                        ))
                        .wrap(),
                    );
                }
                Some(commits) if commits.is_empty() => {
                    ui.label(muted(ui, "No commit of its own: it moved back, or to an older one."));
                }
                Some(commits) => {
                    ui.label(muted(ui, match commits.len() {
                        1 => "The commit it brings:".to_owned(),
                        n => format!("The {n} commits it brings, newest first:"),
                    }));
                    for (id, subject) in commits {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(id.abbreviated(ID_DIGITS).to_string())
                                    .monospace()
                                    .color(palette.muted),
                            );
                            ui.add(Label::new(subject.to_str_lossy().as_ref()).truncate());
                        });
                    }
                }
            },
        }
    });
}

/// An image's sides as textures, made once decoded (egui frees them with the view), and a
/// checkerboard drawn behind them so transparency shows.
struct Textures {
    before: Option<egui::TextureHandle>,
    after: Option<egui::TextureHandle>,
    checks: egui::TextureHandle,
}

impl std::fmt::Debug for Textures {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Textures").finish_non_exhaustive()
    }
}

impl Textures {
    fn of(ctx: &egui::Context, pictures: &Pictures, palette: Palette) -> Self {
        let texture = |name: &str, side: &crate::pictures::Side| {
            side.as_ref()
                .and_then(|side| side.as_ref().ok())
                .map(|picture| {
                    ctx.load_texture(
                        name,
                        Arc::clone(&picture.pixels),
                        egui::TextureOptions::LINEAR,
                    )
                })
        };
        // A patch of four by four tiles, laid side by side across the image's place.
        // Apart enough to see through a translucent image, in either theme.
        let (light, dark) = if palette.dark {
            (
                Color32::from_rgb(0x4A, 0x51, 0x5C),
                Color32::from_rgb(0x2E, 0x33, 0x3B),
            )
        } else {
            (
                Color32::from_rgb(0xFF, 0xFF, 0xFF),
                Color32::from_rgb(0xD9, 0xDE, 0xE4),
            )
        };
        let mut texels = Vec::with_capacity(CHECKS_PATCH.saturating_mul(CHECKS_PATCH));
        for y in 0..CHECKS_PATCH {
            for x in 0..CHECKS_PATCH {
                let odd = ((x >> CHECK_SHIFT) ^ (y >> CHECK_SHIFT)) & 1 == 1;
                texels.push(if odd { dark } else { light });
            }
        }
        let checks = egui::ColorImage::new([CHECKS_PATCH, CHECKS_PATCH], texels);
        let checks = ctx.load_texture("image-checks", checks, egui::TextureOptions::NEAREST);
        Self {
            before: texture("image-before", &pictures.before),
            after: texture("image-after", &pictures.after),
            checks,
        }
    }
}

/// A checkerboard's patch behind an image, in texels (a point each), and its tiles' size as a
/// shift (8 texels).
const CHECKS_PATCH: usize = 32;
const CHECK_SHIFT: usize = 3;

/// An image's change: its sides side by side (`both`), or the image as the change left it,
/// each with its size in pixels and bytes, over a checkerboard, scaled down to fit.
fn show_pictures(
    ui: &mut Ui,
    shared: &Shared<'_>,
    objects: &Objects,
    file: &mut FileView,
    both: bool,
) {
    if file.pictures.is_idle()
        && let Some(git) = shared.git
    {
        file.pictures.request(read_pictures(
            git.clone(),
            Arc::clone(shared.repository),
            objects.clone(),
            file.picture_sources(),
        ));
    }
    let pictures = match file.pictures.read_as_ref() {
        None => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Reading the images…"));
            return;
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return;
        }
        Some(Ok(pictures)) => Arc::clone(pictures),
    };
    let textures = file
        .textures
        .get_or_insert_with(|| Textures::of(ui.ctx(), &pictures, shared.palette));
    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
        crate::help::note_topic(ui, ui.max_rect(), "image-diff");
        let sides = [
            ("Before", &pictures.before, textures.before.as_ref()),
            ("After", &pictures.after, textures.after.as_ref()),
        ];
        let shown: Vec<_> = if both {
            sides.into_iter().collect()
        } else {
            sides.into_iter().skip(1).collect()
        };
        let columns = shown.len();
        ui.columns(columns, |columns| {
            for (column, (title, side, texture)) in columns.iter_mut().zip(shown) {
                picture_side(
                    column,
                    (title, side, texture),
                    &textures.checks,
                    shared.palette,
                );
            }
        });
    });
}

/// A Markdown file as it reads: before and after side by side, or the one side there is (a new
/// file's, a deleted one's), each read the first time and rendered in a scroll area of its
/// own; the pages its links ask for go to egit's opener.
fn show_preview(
    ui: &mut Ui,
    shared: &Shared<'_>,
    objects: &Objects,
    file: &mut FileView,
    actions: &mut Vec<Action>,
) {
    if file.documents.is_idle()
        && let Some(git) = shared.git
    {
        file.documents.request(read_documents(
            git.clone(),
            Arc::clone(shared.repository),
            objects.clone(),
            file.picture_sources(),
        ));
    }
    let documents = match file.documents.read_as_ref() {
        None => {
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::working(ui, "Reading the document…"));
            return;
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::same(16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return;
        }
        Some(Ok(documents)) => Arc::clone(documents),
    };
    let (before, after) = &mut file.previews;
    let sides: Vec<_> = [
        ("Before", &documents.before, before),
        ("After", &documents.after, after),
    ]
    .into_iter()
    .filter(|(_, side, _)| side.is_some())
    .collect();
    let path = file.source.path().to_owned();
    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
        crate::help::note_topic(ui, ui.max_rect(), "markdown-preview");
        ui.columns(sides.len().max(1), |columns| {
            for (column, (title, side, preview)) in columns.iter_mut().zip(sides) {
                let pages = document_side(
                    column,
                    (title, side),
                    preview,
                    (path.as_ref(), shared.palette),
                );
                actions.extend(pages.into_iter().map(Action::OpenUrl));
            }
        });
    });
}

/// One side of a Markdown file's change: its title and size, then the document as it reads, or
/// why it couldn't be read; returns the pages its links asked for.
fn document_side(
    ui: &mut Ui,
    (title, side): (&str, &crate::markdown::Side),
    preview: &mut Preview,
    (path, palette): (&BStr, Palette),
) -> Vec<String> {
    let facts = match side {
        Some(Ok(document)) => format::size(document.bytes),
        Some(Err(why)) => format!("couldn't be read: {why}"),
        None => String::new(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).semibold());
        ui.label(muted(ui, facts));
    });
    ui.add_space(8.0);
    let Some(Ok(document)) = side else {
        return Vec::new();
    };
    let mut pages = Vec::new();
    Frame::new()
        .fill(palette.background)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            pages = crate::markdown::show(ui, preview, document, (title, path));
        });
    pages
}

/// One side of an image's change: its title and size, then the image, or why there is none.
fn picture_side(
    ui: &mut Ui,
    (title, side, texture): (&str, &crate::pictures::Side, Option<&egui::TextureHandle>),
    checks: &egui::TextureHandle,
    palette: Palette,
) {
    let facts = match side {
        Some(Ok(Picture {
            width,
            height,
            bytes,
            ..
        })) => format!("{width} × {height} pixels · {}", format::size(*bytes)),
        Some(Err(_)) => "couldn't be read".to_owned(),
        None if title == "Before" => "none: the image is new".to_owned(),
        None => "none: the image is deleted".to_owned(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).semibold());
        ui.label(muted(ui, facts));
    });
    ui.add_space(8.0);
    match (side, texture) {
        (Some(Ok(picture)), Some(texture)) => {
            // As large as it is, in points, or smaller to fit; never larger.
            let natural = egui::vec2(
                f32::from(u16::try_from(picture.width).unwrap_or(u16::MAX)),
                f32::from(u16::try_from(picture.height).unwrap_or(u16::MAX)),
            );
            let room = ui.available_size();
            let scale = (room.x / natural.x).min(room.y / natural.y).min(1.0);
            let scaled = egui::vec2(natural.x * scale, natural.y * scale);
            let (rect, _) = ui.allocate_exact_size(scaled, egui::Sense::hover());
            checkerboard(ui, rect, checks);
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            ui.painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, palette.border),
                egui::StrokeKind::Outside,
            );
        }
        (Some(Err(why)), _) => {
            ui.add(Label::new(muted(ui, format!("It couldn't be shown: {why}"))).wrap());
        }
        _ => {}
    }
}

/// The checkerboard's patch laid side by side across `rect` (the last ones cut at its edges).
fn checkerboard(ui: &Ui, rect: egui::Rect, checks: &egui::TextureHandle) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let patch = f32::from(u8::try_from(CHECKS_PATCH).unwrap_or(u8::MAX));
    let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    let mut mesh = egui::Mesh::with_texture(checks.id());
    let mut y = rect.top();
    loop {
        if y >= rect.bottom() {
            break;
        }
        let mut x = rect.left();
        loop {
            if x >= rect.right() {
                break;
            }
            let place = egui::Rect::from_min_size(egui::pos2(x, y), egui::Vec2::splat(patch));
            mesh.add_rect_with_uv(place, full, Color32::WHITE);
            x += patch;
        }
        y += patch;
    }
    painter.add(mesh);
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
            let button = ui.button("Show the pointer's changes");
            crate::help::note(ui, button.rect, "Show the pointer's changes");
            button.clicked()
        })
        .inner
}

/// What the commit (or the work tree) changed in the file.
fn show_diff(
    ui: &mut Ui,
    shared: &Shared<'_>,
    objects: &Objects,
    file: &mut FileView,
    actions: &mut Vec<Action>,
) {
    // (A file in conflict shows the merge editor instead.)
    if let Some(sides) = file.submodule_move() {
        show_submodule_move(ui, shared, file, sides);
        return;
    }
    let work = file.work().map(|work| work.which);
    let image = is_image(file.path());
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
        Some(Ok(diff)) if diff.binary() && image => {
            show_pictures(ui, shared, objects, file, true);
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
            let response = file.diff_grid.show(ui, &*diff);
            if let Some(work) = file.work().filter(|work| !work.conflict).cloned() {
                lines_menu(&response, shared, file, (&work, &diff), actions);
            }
        }
    }
}

/// What a right click in a work tree's file's diff asked for.
enum LinesAsked {
    Stage(StageRequest),
    Discard(Discard),
}

/// A right click in a work tree's file's diff: a menu doing what its bar does to the lines
/// selected (the line under the pointer, when the click isn't on a selection): stage or
/// discard them, or unstage them on the staged side, a line or a hunk at a time; and copy them.
fn lines_menu(
    response: &egui::Response,
    shared: &Shared<'_>,
    file: &mut FileView,
    (work, diff): (&WorkFile, &Arc<DiffDocument>),
    actions: &mut Vec<Action>,
) {
    if response.secondary_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let on_selection = file
            .diff_grid
            .selected_lines()
            .zip(file.diff_grid.line_at(pointer))
            .is_some_and(|((_, lines), line)| lines.contains(&line));
        if !on_selection {
            file.diff_grid.select_line_at(pointer);
        }
    }
    let mut asked = None;
    let grid = &file.diff_grid;
    response.context_menu(|ui| {
        asked = lines_menu_items(ui, shared, (work, diff), grid);
    });
    let Some(asked) = asked else {
        return;
    };
    file.diff_grid.clear_selection();
    actions.push(match asked {
        LinesAsked::Stage(request) => Action::Stage(shared.id, request),
        LinesAsked::Discard(discard) => Action::Task(shared.id, Task::Discard(discard)),
    });
}

/// The lines menu's items; what is chosen.
fn lines_menu_items(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (work, diff): (&WorkFile, &Arc<DiffDocument>),
    grid: &TextGrid,
) -> Option<LinesAsked> {
    use super::actions::{danger_item, item};
    let picked = |hunks: bool| {
        let (_, lines) = grid.selected_lines()?;
        let picked = diff.changes_in(lines, hunks);
        picked.contains(&true).then(|| (Arc::clone(diff), picked))
    };
    let none = Err("No changed line is selected");
    let allowed = |picked: &Option<_>| if picked.is_some() { Ok(()) } else { none };
    let (lines, hunks) = (picked(false), picked(true));
    let mut asked = None;
    let (glyph, verb) = match work.direction() {
        Direction::Stage => (icon::PLUS, "Stage"),
        Direction::Unstage => (icon::MINUS, "Unstage"),
    };
    if item(ui, (glyph, &format!("{verb} lines")), allowed(&lines)) {
        asked = lines
            .clone()
            .map(|lines| LinesAsked::Stage(lines_request(work, lines)));
    }
    if item(ui, (glyph, &format!("{verb} hunk")), allowed(&hunks)) {
        asked = hunks
            .clone()
            .map(|hunks| LinesAsked::Stage(lines_request(work, hunks)));
    }
    if work.direction() == Direction::Stage {
        ui.separator();
        let refused = discard_refusal(shared, work);
        let discards = |picked: &Option<_>| refused.and_then(|()| allowed(picked));
        if danger_item(ui, (icon::TRASH, "Discard lines"), discards(&lines)) {
            asked = lines.map(|lines| LinesAsked::Discard(discard_lines(work, lines, false)));
        }
        if danger_item(ui, (icon::TRASH, "Discard hunk"), discards(&hunks)) {
            asked = hunks.map(|hunks| LinesAsked::Discard(discard_lines(work, hunks, true)));
        }
    }
    ui.separator();
    if item(ui, (icon::COPY, "Copy"), Ok(()))
        && let Some(text) = grid.selected_text(&[&**diff])
    {
        ui.ctx().copy_text(text);
    }
    asked
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
    let image = is_image(file.path());
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
            FileContent::Binary { .. } if image => {
                show_pictures(ui, shared, objects, file, false);
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
