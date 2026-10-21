//! A repository's history: the commit table (a painted graph column, ref badges, subjects,
//! authors, dates, names) and the selected commit's details and changed files.
//!
//! The table is egui-table-kit's layout `Table`, driven directly: only rows on screen are drawn,
//! their lanes replayed from the nearest chunk and their summaries read as they come into view,
//! so a frame costs the same at row 10 as at row 1,000,000. (Its `TableKit` keeps state for
//! every row, which a graph, never sorted or filtered, doesn't need.)

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    Bisection, CancelToken, Changes, Committed, ConfigSnapshot, FileChange, Git, Head, LfsTransfer,
    MailboxProgress, ObjectId, Operation, RefKind, Reference, Repository, SignatureCheck, Stash,
    Status, Uncommitted, Verdict, looks_like_patch,
};
use egit_graph::{Chunk, Level, Segment, for_each_row};
use egui::epaint::CubicBezierShape;
use egui::{
    Align, Color32, CornerRadius, Frame, Id, Key, KeyboardShortcut, Label, Layout, Margin,
    Modifiers, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, WidgetInfo, WidgetType, pos2,
};
use egui_async::Bind;
use egui_table_kit::layout::{
    CellInfo, Column, HeaderCellInfo, HeaderRow, PrefetchInfo, Table, TableDelegate,
};

use super::actions::{self, Ask, Menus};
use super::blame::{self, BlameEvent, BlameView};
use super::file::{self, FileView, FileWanted, WorkFile};
use super::file_history::{self, FileHistoryView};
use super::lfs::LfsEvent;
use super::patch::{self, PatchEvent, PatchSource, PatchView};
use super::picked::{self, Click, Exporting, Picked, PickedEvent};
use super::pipeline::{self, MarkEvent, PipelineEvent, PipelineView};
use super::pull_form::{self, PullRequestForm};
use super::pulls::{self, PullRequestEvent, PullRequestView};
use super::rebase::{self, RebaseEvent, RebaseView};
use super::reflog::{self, ReflogEvent, ReflogView};
use super::search::{self, HistorySearch};
use super::sidebar::{self, Lists, Sidebar, SidebarEvent};
use super::stats::{StatsEvent, StatsView};
use super::submodules::SubmodulesEvent;
use super::tasks::{self, TaskEvent, TaskPane, TasksView};
use super::work::{CommitSide, WorkEvent, WorkPanel};
use crate::app::{Action, OutputAsk};
use crate::commands::RepoCommand;
use crate::desktop::FileQuestion;
use crate::exporting::Export;
use crate::fonts;
use crate::forge::{Badge, RepoForge, request_noun, request_number};
use crate::format::{count, date_time};
use crate::history::{
    ChangeCounts, CommitDetails, History, HistoryState, Objects, Refs, Scope, Signatures,
    Summaries, read_change_counts, read_comparison, read_details,
};
use crate::preferences::Preferences;
use crate::problem::Problem;
use crate::repo::{RepoId, RepoPanel};
use crate::tables::ChangesTable;
use crate::tasks::{Task, Tasks};
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::undo::Journal;
use crate::widgets::{self, Look, muted};
use crate::worktree::WorkTree;

/// A row's height.
const ROW_HEIGHT: f32 = 24.0;

/// The header's height.
const HEADER_HEIGHT: f32 = widgets::HEAD;

/// A lane's width in the graph column.
const LANE_WIDTH: f32 = 14.0;

/// Space left of the first lane.
const GRAPH_MARGIN: f32 = 6.0;

/// A commit's node's radius.
const NODE_RADIUS: f32 = 4.5;

/// The lanes' lines' width.
const LINE_WIDTH: f32 = 2.0;

/// The most lanes the graph column is first given room for.
const FIRST_LANES: usize = 10;

/// The graph column's narrowest first width (its title fits).
const FIRST_GRAPH_WIDTH: f32 = 52.0;

/// Lanes drawn before the graph column's width is known.
const FIRST_LANES_SHOWN: u16 = 64;

/// The digits of a commit's name its row shows.
pub(super) const ID_DIGITS: usize = 8;

/// The most ref badges a row shows; more are counted.
const MAX_BADGES: usize = 3;

/// Histories at least this long without a commit-graph are offered one.
const OFFER_COMMIT_GRAPH_AT: u64 = 10_000;

/// The narrowest the message column gets.
const MIN_MESSAGE_WIDTH: f32 = 160.0;

/// What a repository's history pane works with while it is drawn.
pub(crate) struct HistoryPane<'a> {
    pub(crate) id: RepoId,
    /// Git, once found (reads start only then).
    pub(crate) git: Option<&'a Git>,
    pub(crate) repository: &'a Arc<Repository>,
    pub(crate) history: &'a mut HistoryState,
    pub(crate) view: &'a mut HistoryView,
    /// Where `HEAD` points, once read.
    pub(crate) head: Option<&'a Head>,
    /// Writing a commit-graph file.
    pub(crate) writing_commit_graph: &'a mut Bind<(), Problem>,
    /// Whether the repository is a partial clone, once its configuration is read: lines
    /// aren't counted there (git could fetch every blob a commit changed).
    pub(crate) partial_clone: Option<bool>,
    /// The working tree's status.
    pub(crate) work_tree: &'a mut WorkTree,
    /// Staging and committing.
    pub(crate) work: WorkSide<'a>,
    /// The operations git stopped in the middle of (a merge, a rebase, …).
    pub(crate) operations: &'a [Operation],
    /// How far a `git am` stopped part-way got.
    pub(crate) applying: Option<&'a MailboxProgress>,
    /// Where a bisection under way stands.
    pub(crate) bisect: Option<&'a Bisection>,
    /// The repository's configuration, once read.
    pub(crate) config: Option<&'a ConfigSnapshot>,
    /// What the user asked of the repository: fetching, switching, stashing, ….
    pub(crate) tasks: &'a mut Tasks,
    /// The stashes, once read, the latest first.
    pub(crate) stashes: &'a [Stash],
    /// Its remotes on GitHub and GitLab, and their requests.
    pub(crate) forge: &'a RepoForge,
    /// The user's home folder, shown as `~`.
    pub(crate) home: Option<&'a Path>,
    /// What egit did here, to take back, and what it took back, to do again.
    pub(crate) journal: &'a Journal,
    /// Whether its agents' panel shows.
    pub(crate) agents_shown: bool,
    /// The stashes, worktrees, submodules, and configuration, for the sidebar.
    pub(crate) lists: Lists<'a>,
    pub(crate) actions: &'a mut Vec<Action>,
}

/// What the history pane remembers between frames.
#[derive(Debug)]
pub(crate) struct HistoryView {
    /// What bringing commits in would do, foreseen for the menus.
    foresight: crate::foresight::Foresight,
    selected: Option<Selected>,
    /// A row to scroll to next frame, and where to put it.
    scroll_to: Option<(u64, Option<Align>)>,
    /// The rows on screen last frame.
    visible: Range<u64>,
    /// The first row on screen and its commit: kept in view when the history is read again.
    top: Option<(u64, ObjectId)>,
    /// How many lanes the graph column showed last frame.
    lanes_shown: u16,
    graph: GraphRows,
    details: Bind<Arc<CommitDetails>, Problem>,
    /// The commit whose details were last asked for.
    details_for: Option<ObjectId>,
    counting: Counting,
    changes: ChangesTable,
    /// A changed file being looked at, in place of the commit table.
    file: Option<FileView>,
    /// Whether the working tree's changes are shown beside the history, rather than a
    /// commit's.
    work_selected: bool,
    work: WorkPanel,
    tasks: TasksView,
    /// What a menu in the details asked for, carried out once the panels are drawn.
    asked: Option<Ask>,
    /// A tool in place of the commit table: the rebase editor, a blame, a file's history, the
    /// pull requests, patches to apply.
    tool: Option<Tool>,
    /// The repository it shows.
    repo: RepoId,
    /// The dialog opening a pull request, while it is open.
    pull_form: Option<PullRequestForm>,
    /// The search bar over the commit table, and what it found.
    search: HistorySearch,
    /// The sidebar beside the commit table.
    sidebar: Sidebar,
    /// A commit the details show in place of the selected one's, which the history may not
    /// have: a stash's, or a ref's hidden from the graph, picked in the sidebar.
    peek: Option<ObjectId>,
    /// Commands asked from the keyboard or the palette, carried out next frame.
    queued: Vec<RepoCommand>,
    /// A work tree's file asked for (an agent read or changed it), shown next frame.
    wanted_file: Option<BString>,
    /// What the top bar measured last frame.
    bar: BarRoom,
    /// The selected commit's message being edited in the details, to reword it.
    message_edit: Option<MessageEdit>,
    /// A rewrite of the selected commit was asked for: when it is gone from the next history
    /// read, the selection stays on its row (where its rewritten self is).
    keep_row: bool,
    /// How many files the work in progress's row counts, by the status read they came from.
    work_counts: WorkCounts,
    /// A ref dropped onto another, while what it can do is asked.
    dropped: Option<Dropped>,
    /// What the submodules' banner said when dismissed: it shows again when that changes.
    submodules_dismissed: Option<crate::submodules::OutOfStep>,
    /// Why the pipelines couldn't be read, when that notice was dismissed: it shows again for
    /// another reason.
    pipelines_dismissed: Option<String>,
    /// Commits picked together, for what acts on several.
    picked: Picked,
    /// Two of them compared, in the details' place.
    compare: Option<Comparison>,
    /// What their panel asked for this frame.
    picked_asked: Option<PickedEvent>,
    /// The Stats page, kept while it is closed: opened again, it shows at once (and reads
    /// again only for a history read again).
    stats_kept: Option<Box<StatsView>>,
}

/// A ref dropped onto another, where.
#[derive(Debug)]
struct Dropped {
    dragged: Reference,
    onto: Reference,
    at: Pos2,
}

/// What a ref badge being dragged carries.
struct DraggedRef(Reference);

/// The files with changes not yet committed, and how many of them are staged: counted once a
/// status read (100,000 untracked files are counted in a millisecond, too long for a frame).
#[derive(Debug, Default)]
struct WorkCounts {
    /// The status read counted (its address).
    of: usize,
    counts: Counted,
}

/// What [`WorkCounts`] counted.
#[derive(Clone, Copy, Debug, Default)]
struct Counted {
    files: usize,
    staged: usize,
}

impl WorkCounts {
    /// Counts `status`, unless it is the read counted last.
    fn of(&mut self, status: &egit_git::Status) {
        let address = std::ptr::from_ref(status).addr();
        if address == self.of {
            return;
        }
        self.of = address;
        self.counts = Counted {
            files: status.len(),
            staged: status.entries().filter(|entry| entry.is_staged()).count(),
        };
    }
}

/// A commit's message being edited in the details: its summary and description as typed.
#[derive(Debug)]
struct MessageEdit {
    commit: ObjectId,
    summary: String,
    description: String,
    /// The summary's field takes the keyboard's focus at first.
    focus: bool,
}

/// What can take the commit table's place, besides a changed file.
#[derive(Debug)]
enum Tool {
    Rebase(RebaseView),
    Blame(BlameView),
    FileHistory(FileHistoryView),
    PullRequests(PullRequestView),
    Pipeline(Box<PipelineView>),
    /// A job's log, in its pipeline's page's place (kept, to go back to).
    Job(Box<super::job::JobView>),
    Patch(PatchView),
    Reflog(Box<ReflogView>),
    Lfs(Box<super::lfs::LfsView>),
    Submodules(Box<super::submodules::SubmodulesView>),
    Stats(Box<StatsView>),
}

/// The lines a commit's files gained and lost, counted after its files are listed.
#[derive(Debug)]
struct Counting {
    counts: Bind<Arc<ChangeCounts>, Problem>,
    /// The commit last counted.
    of: Option<ObjectId>,
    cancel: CancelToken,
}

impl Default for Counting {
    fn default() -> Self {
        Self {
            counts: Bind::new(true),
            of: None,
            cancel: CancelToken::new(),
        }
    }
}

impl Counting {
    /// Counts `details`' lines unless they are counted or being counted (a count under way for
    /// another commit stops), and returns what is known for them.
    fn of(
        &mut self,
        shared: &Shared<'_>,
        details: &Arc<CommitDetails>,
    ) -> Option<Result<Arc<ChangeCounts>, Problem>> {
        if self.of != Some(details.id)
            && shared.partial_clone == Some(false)
            && let Some(git) = shared.git
        {
            self.cancel.cancel();
            self.cancel = CancelToken::new();
            self.counts.request(read_change_counts(
                git.clone(),
                Arc::clone(shared.repository),
                Arc::clone(details),
                self.cancel.clone(),
            ));
            self.of = Some(details.id);
        }
        if self.of != Some(details.id) || self.counts.is_pending() {
            return None;
        }
        match self.counts.read_as_ref()? {
            Ok(counts) => Some(Ok(Arc::clone(counts))),
            Err(problem) => Some(Err(problem.clone())),
        }
    }
}

impl Drop for Counting {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Two commits picked, compared: what changed from one to the other, in the details' place.
#[derive(Debug)]
struct Comparison {
    from: ObjectId,
    to: ObjectId,
    /// `to`'s details with `from` as its parent: its files, their lines, and their diffs run
    /// from one to the other.
    details: Bind<Arc<CommitDetails>, Problem>,
    counting: Counting,
    changes: ChangesTable,
}

impl Comparison {
    /// What changed from `from` to `to`, being read.
    fn new(shared: &Shared<'_>, objects: &Objects, from: ObjectId, to: ObjectId) -> Self {
        let mut details = Bind::new(true);
        if let Some(git) = shared.git {
            details.request(read_comparison(
                git.clone(),
                Arc::clone(shared.repository),
                objects.clone(),
                from,
                to,
            ));
        }
        Self {
            from,
            to,
            details,
            counting: Counting::default(),
            changes: ChangesTable::new(format!("comparison-changes-{}", shared.id)),
        }
    }
}

/// The selected commit and its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selected {
    id: ObjectId,
    row: u64,
}

impl HistoryView {
    pub(crate) fn new(repo: RepoId) -> Self {
        Self {
            repo,
            foresight: crate::foresight::Foresight::default(),
            selected: None,
            scroll_to: None,
            visible: 0..0,
            top: None,
            lanes_shown: FIRST_LANES_SHOWN,
            graph: GraphRows::default(),
            details: Bind::new(true),
            details_for: None,
            counting: Counting::default(),
            changes: ChangesTable::new(format!("history-changes-{repo}")),
            file: None,
            work_selected: false,
            work: WorkPanel::new(repo),
            tasks: TasksView::default(),
            asked: None,
            tool: None,
            pull_form: None,
            search: HistorySearch::default(),
            sidebar: Sidebar::default(),
            peek: None,
            queued: Vec::new(),
            wanted_file: None,
            bar: BarRoom::default(),
            message_edit: None,
            keep_row: false,
            work_counts: WorkCounts::default(),
            dropped: None,
            submodules_dismissed: None,
            pipelines_dismissed: None,
            picked: Picked::default(),
            compare: None,
            picked_asked: None,
            stats_kept: None,
        }
    }

    /// Asks it to carry `command` out (next frame, with everything its own buttons have).
    pub(crate) fn queue(&mut self, command: RepoCommand) {
        self.queued.push(command);
    }

    /// Asks it to show the work tree's file `path` (next frame): its changes, or the file in
    /// its app when it has none.
    pub(crate) fn show_work_file(&mut self, path: BString) {
        self.wanted_file = Some(path);
    }

    /// The commit the details show in place of the selected one's: the reflog's selected
    /// move's, while it is open, or the one picked in the sidebar (in no history, maybe).
    fn peeked(&self) -> Option<ObjectId> {
        match &self.tool {
            Some(Tool::Reflog(reflog)) => reflog.selected_commit(),
            _ => self.peek,
        }
    }

    /// Opens `change` of commit `details` in a file view, shown as the one open was.
    fn open_file(&mut self, shared: &Shared<'_>, details: &CommitDetails, change: FileChange) {
        self.file = Some(FileView::open(shared, details, change, self.file.as_ref()));
    }

    /// Where on screen the place before cell `cell` of line `line` was drawn, in the view shown.
    #[cfg(test)]
    pub(crate) fn file_spot(&self, line: u32, cell: u32) -> Option<egui::Pos2> {
        match (&self.file, &self.tool) {
            (Some(file), _) => file.spot(line, cell),
            (None, Some(Tool::Blame(blame))) => blame.spot(line, cell),
            _ => None,
        }
    }

    /// Whether the file open shows its diff side by side.
    #[cfg(test)]
    pub(crate) fn file_split(&self) -> Option<bool> {
        self.file.as_ref().map(FileView::is_split)
    }

    /// The title in the pull request dialog, while it is open.
    #[cfg(test)]
    pub(crate) fn pull_form_title(&self) -> Option<String> {
        self.pull_form.as_ref().map(PullRequestForm::title)
    }

    /// Whether a blame is shown, and done.
    #[cfg(test)]
    pub(crate) fn blame_done(&self) -> Option<bool> {
        match &self.tool {
            Some(Tool::Blame(blame)) => Some(blame.done()),
            _ => None,
        }
    }

    /// How many lines the blame shown has blamed so far, and whether it is done.
    #[cfg(test)]
    pub(crate) fn blame_progress(&self) -> Option<(u32, bool)> {
        match &self.tool {
            Some(Tool::Blame(blame)) => Some((blame.found(), blame.done())),
            _ => None,
        }
    }

    /// Scrolls the blame shown to line `line`.
    #[cfg(test)]
    pub(crate) const fn blame_scroll_to(&mut self, line: u32) {
        if let Some(Tool::Blame(blame)) = &mut self.tool {
            blame.scroll_to_line(line);
        }
    }

    /// The file history's diff shown, as lines it adds and removes (`None` until it is read).
    #[cfg(test)]
    pub(crate) fn file_history_diff(&mut self) -> Option<(u32, u32)> {
        match &mut self.tool {
            Some(Tool::FileHistory(history)) => history.diff_changed(),
            _ => None,
        }
    }

    /// The file being looked at: its path, and whether its contents are shown yet.
    #[cfg(test)]
    pub(crate) fn file_shown(&mut self) -> Option<(String, bool)> {
        self.file.as_mut().map(FileView::shown)
    }

    /// How many lines the open file's diff has, once read.
    #[cfg(test)]
    pub(crate) fn diff_lines(&mut self) -> Option<u32> {
        self.file.as_mut()?.diff_lines()
    }

    /// Which of the open work tree file's changes it shows.
    #[cfg(test)]
    pub(crate) fn work_side(&self) -> Option<egit_git::Uncommitted> {
        self.file.as_ref()?.work_side()
    }

    /// How many lines the new side of the open file's split diff has, once split.
    #[cfg(test)]
    pub(crate) fn split_lines(&mut self) -> Option<u32> {
        self.file.as_mut()?.split_lines()
    }

    /// Whether the open file's Markdown previews have settled: every block measured, and
    /// scrolled where asked.
    #[cfg(test)]
    pub(crate) fn previews_settled(&mut self) -> bool {
        self.file.as_mut().is_some_and(FileView::previews_settled)
    }

    /// The selected commit's row.
    #[cfg(test)]
    pub(crate) fn selected_row(&self) -> Option<u64> {
        self.selected.map(|selected| selected.row)
    }

    /// Whether the selected commit's lines are counted.
    #[cfg(test)]
    pub(crate) fn counts_shown(&mut self) -> bool {
        let selected = self
            .peeked()
            .or_else(|| self.selected.map(|selected| selected.id));
        !self.counting.counts.is_pending()
            && self
                .counting
                .counts
                .ok_ref()
                .is_some_and(|counts| Some(counts.id) == selected)
    }

    /// Whether the selected commit's details are shown.
    #[cfg(test)]
    pub(crate) fn details_shown(&mut self) -> bool {
        let selected = self
            .peeked()
            .or_else(|| self.selected.map(|selected| selected.id));
        !self.details.is_pending()
            && self
                .details
                .ok_ref()
                .is_some_and(|details| Some(details.id) == selected)
    }

    /// How many commits the search found, once it ended.
    #[cfg(test)]
    pub(crate) fn search_found(&self) -> Option<u64> {
        self.search
            .found()
            .filter(|found| found.is_done())
            .map(crate::search::Found::count)
    }

    /// Scrolls `row` to the top.
    #[cfg(test)]
    pub(crate) const fn scroll_to(&mut self, row: u64) {
        self.scroll_to = Some((row, Some(Align::Min)));
    }

    /// Selects the commit in `row`, scrolling it into view.
    fn select(&mut self, history: &History, row: u64, align: Option<Align>) {
        if let Some(id) = history.id(row) {
            self.selected = Some(Selected { id, row });
            self.scroll_to = Some((row, align));
            self.work_selected = false;
            self.peek = None;
        }
    }

    /// Shows `commit`: selected in the history when it is there, else in the details alone.
    fn show_commit(&mut self, history: &History, commit: ObjectId) {
        let near = self.selected.map_or(0, |selected| selected.row);
        if let Some(row) = history.row_of(&commit, near) {
            self.select(history, row, Some(Align::Center));
        } else {
            self.peek = Some(commit);
            self.work_selected = false;
        }
    }

    /// Closes the file looked at unless it belongs to what is selected: a commit's to that
    /// commit, the work tree's to the work tree.
    fn close_unselected_file(&mut self) {
        let compared = self.compare.as_ref().map(|compare| compare.to);
        let belongs = self.file.as_ref().is_none_or(|file| match file.commit() {
            Some(commit) => {
                !self.work_selected
                    && (self.selected.map(|selected| selected.id) == Some(commit)
                        || self.peek == Some(commit)
                        || compared == Some(commit))
            }
            None => self.work_selected,
        });
        if !belongs {
            self.file = None;
        }
    }

    /// Shows the working tree's changes beside the history.
    /// Opens a file's blame or history in the commit table's place.
    fn open_tool(&mut self, shared: &Shared<'_>, objects: &Objects, wanted: FileWanted) {
        self.file = None;
        self.tool = Some(match wanted {
            FileWanted::Blame { at, path } => {
                Tool::Blame(BlameView::new(shared, objects, at, path))
            }
            FileWanted::History(path) => Tool::FileHistory(FileHistoryView::new(shared, path)),
        });
    }

    /// Shows the pull requests in the commit table's place, `selected` (a repository's place
    /// and a number) selected if given.
    fn show_pull_request(&mut self, selected: Option<(usize, u32)>) {
        self.file = None;
        self.tool = Some(Tool::PullRequests(PullRequestView::new(selected)));
    }

    /// Shows `mark`'s pipeline in the commit table's place.
    /// The job's log shown in the commit table's place, as text, and whether it follows its
    /// end.
    #[cfg(test)]
    pub(crate) fn job_log(&self) -> Option<(String, bool)> {
        match &self.tool {
            Some(Tool::Job(view)) => view.shown(),
            _ => None,
        }
    }

    fn show_pipeline(&mut self, mark: crate::ci::CiMark) {
        self.file = None;
        self.tool = Some(Tool::Pipeline(Box::new(PipelineView::new(mark))));
    }

    /// Shows the patches in `files`, to apply, in the commit table's place.
    pub(crate) fn show_patches(&mut self, files: Vec<PathBuf>) {
        self.show_patch(PatchSource::Files(files));
    }

    /// Shows a patch (`source`'s) in the commit table's place.
    fn show_patch(&mut self, source: PatchSource) {
        self.file = None;
        self.tool = Some(Tool::Patch(PatchView::new(source, self.repo)));
    }

    /// Shows `reference`'s reflog (`HEAD`, or a branch's full name) in the commit table's place.
    fn show_reflog(&mut self, reference: bstr::BString) {
        self.file = None;
        self.tool = Some(Tool::Reflog(Box::new(ReflogView::new(reference))));
    }

    /// Shows Git LFS's page in the commit table's place.
    fn show_lfs(&mut self) {
        self.file = None;
        self.tool = Some(Tool::Lfs(Box::default()));
    }

    /// Shows the submodules' page in the commit table's place.
    fn show_submodules(&mut self) {
        self.file = None;
        self.tool = Some(Tool::Submodules(Box::default()));
    }

    /// Shows the Stats page (for tests: the top bar's button toggles it).
    #[cfg(test)]
    pub(crate) fn open_stats(&mut self) {
        if !matches!(self.tool, Some(Tool::Stats(_))) {
            self.toggle_stats();
        }
    }

    /// Whether the Stats page's figures have landed, and its lines (or they aren't counted).
    #[cfg(test)]
    pub(crate) fn stats_landed(&mut self) -> (bool, bool) {
        match &mut self.tool {
            Some(Tool::Stats(page)) => page.landed(),
            _ => (false, false),
        }
    }

    /// Shows the Stats page in the commit table's place, or (shown) the history again.
    fn toggle_stats(&mut self) {
        self.file = None;
        if matches!(self.tool, Some(Tool::Stats(_))) {
            self.close_tool();
        } else {
            self.tool = Some(Tool::Stats(self.stats_kept.take().unwrap_or_default()));
        }
    }

    /// Closes the tool in the commit table's place (the Stats page is kept).
    fn close_tool(&mut self) {
        if let Some(Tool::Stats(stats)) = self.tool.take() {
            self.stats_kept = Some(stats);
        }
    }

    /// Shows the sidebar or hides it.
    pub(crate) const fn show_sidebar(&mut self, open: bool) {
        self.sidebar.open = open;
    }

    /// Opens the search bar, over the commit table.
    fn open_search(&mut self) {
        self.file = None;
        self.tool = None;
        self.search.open();
    }

    fn select_work(&mut self) {
        self.work_selected = true;
        self.peek = None;
        if self.file.as_ref().is_some_and(|file| file.work().is_none()) {
            self.file = None;
        }
    }

    /// Opens `work`'s changes in the file view.
    fn open_work(&mut self, shared: &Shared<'_>, work: WorkFile) {
        self.file = Some(FileView::open_work(shared, work, self.file.as_ref()));
    }

    /// Finds its place again in a history that replaced the one shown: the selected commit,
    /// and the commit that was at the top of the screen; searches it again, and reads a reflog
    /// shown again.
    fn follow(&mut self, history: &History) {
        self.search.history_replaced();
        self.picked.follow(history);
        if let Some(Tool::Reflog(reflog)) = &mut self.tool {
            // Refs moved: where they have been is read again.
            reflog.refresh();
        }
        if let Some(selected) = self.selected {
            self.selected = match history.row_of(&selected.id, selected.row) {
                Some(row) => Some(Selected { row, ..selected }),
                // Reworded: the commit in its place is what it became.
                None if std::mem::take(&mut self.keep_row) => history
                    .id(selected.row)
                    .map(|id| Selected { id, ..selected }),
                None => None,
            };
        }
        if let Some((row, id)) = self.top.take()
            && row > 0
            && let Some(row) = history.row_of(&id, row)
        {
            self.scroll_to = Some((row, Some(Align::Min)));
        }
    }
}

/// The lanes of the rows on screen, worked out once a frame.
///
/// A column narrower than the graph shows the lanes around the nodes on screen: from the first
/// lane when they fit, else from the leftmost node's (deep in Linux's history, every commit on
/// screen may sit beyond lane 100).
#[derive(Debug, Default)]
struct GraphRows {
    first: u64,
    /// The first lane drawn.
    start: u16,
    /// The most lanes open on a row on screen.
    open: usize,
    rows: Vec<GraphRow>,
    segments: Vec<Segment>,
    /// Scratch for the replay.
    lanes: Vec<u16>,
}

/// A row's node, and its lines (a range of [`GraphRows::segments`]).
#[derive(Clone, Debug)]
struct GraphRow {
    lane: u16,
    color: u16,
    merge: bool,
    segments: Range<usize>,
}

impl GraphRows {
    /// Works out `rows`' nodes and lines, `fit` lanes of them.
    fn fill(&mut self, chunks: &[Chunk], rows: Range<u64>, fit: u16) {
        let Self {
            first,
            start,
            open,
            rows: out,
            segments,
            lanes: scratch,
        } = self;
        let (mut low, mut high) = (u16::MAX, 0_u16);
        for_each_row(chunks, rows.clone(), scratch, |_, view| {
            low = low.min(view.row.lane);
            high = high.max(view.row.lane);
        });
        *start = if low > high {
            0
        } else {
            high.saturating_add(1).saturating_sub(fit).min(low)
        };
        let window = *start..start.saturating_add(fit);
        *first = rows.start;
        *open = 0;
        out.clear();
        segments.clear();
        for_each_row(chunks, rows, scratch, |_, view| {
            let from = segments.len();
            view.segments_in(window.clone(), segments);
            *open = (*open).max(view.before.len());
            out.push(GraphRow {
                lane: view.row.lane,
                color: view.row.color,
                merge: view.is_merge(),
                segments: from..segments.len(),
            });
        });
    }

    fn row(&self, row: u64) -> Option<(&GraphRow, &[Segment])> {
        let index = usize::try_from(row.checked_sub(self.first)?).ok()?;
        let graph = self.rows.get(index)?;
        Some((graph, self.segments.get(graph.segments.clone())?))
    }

    /// The graph column's title: arrows say where lanes are out of view.
    fn title(&self, fit: u16) -> String {
        let left = if self.start > 0 { "‹ " } else { "" };
        let shown = usize::from(self.start).saturating_add(usize::from(fit));
        let right = if self.open > shown { " ›" } else { "" };
        format!("{left}Graph{right}")
    }
}

/// Draws a repository's history pane.
pub(crate) fn show(ui: &mut Ui, pane: HistoryPane<'_>) {
    let HistoryPane {
        id,
        git,
        repository,
        history,
        view,
        head,
        writing_commit_graph,
        partial_clone,
        work_tree,
        work,
        operations,
        applying,
        bisect,
        config,
        tasks,
        stashes,
        forge,
        home,
        journal,
        agents_shown,
        lists,
        actions,
    } = pane;
    if std::mem::take(&mut history.replaced) {
        view.follow(&history.shown);
    }
    let work_status = work_tree.status().and_then(Result::ok).cloned();
    let shared = Shared {
        id,
        git,
        repository,
        head: HeadAt::of(head),
        palette: Palette::of(ui.ctx()),
        partial_clone,
        foresight: view.foresight.clone(),
        work_status: work_status.as_ref(),
        operation: operations.first().copied(),
        bisect,
        preferences: Preferences::of(ui.ctx()),
        config,
        busy: tasks.is_busy(),
        committing: work.committing.is_pending(),
        forge,
        scope: history.scope().clone(),
        home,
        journal,
        agents_shown,
        lfs: lists.lfs.and_then(Result::ok),
        submodules: lists.submodules.and_then(Result::ok),
        submodule_states: lists.submodule_states.and_then(Result::ok),
    };
    // Before the first commit, the work tree is all there is to look at.
    if history.shown.rows() == 0 && !history.shown.is_loading() {
        view.work_selected = true;
    }
    dropped_patches(ui, (id, shared.agents_shown), view);

    let mut pane = TaskPane {
        id,
        work_tree: repository.work_tree(),
        home,
        head,
        operations,
        applying,
        bisect,
        config,
        stashes,
        tasks: &mut *tasks,
        actions: &mut *actions,
    };
    queued_commands(&shared, view, &mut pane);
    bar_panel(
        ui,
        &shared,
        history,
        work_tree,
        view,
        writing_commit_graph,
        &mut pane,
    );
    dialogs(ui.ctx(), &shared, &history.objects, view, &mut pane);

    let no_commits = history.shown.rows() == 0;
    if no_commits && work_status.as_ref().is_none_or(|status| status.is_empty()) {
        notices_panel(ui, &shared, history, view, &mut pane);
        nothing_yet(ui, history);
        return;
    }

    let sides = Sides {
        work,
        lists: &lists,
    };
    panels(ui, &shared, history, view, sides, &mut pane, no_commits);
    drop_menu(ui.ctx(), &shared, history, view);
}

/// What a ref dropped onto another can do, in a menu where it was dropped (`actions::drop_items`)
/// until one is picked, the pointer clicks elsewhere, or Escape.
fn drop_menu(
    ctx: &egui::Context,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
) {
    let Some(dropped) = &view.dropped else {
        return;
    };
    let menus = shared.menus(history.shown.refs());
    let mut asked = None;
    let area = egui::Area::new(Id::new(("drop-menu", shared.id)))
        .order(egui::Order::Foreground)
        .fixed_pos(dropped.at)
        .constrain(true)
        .show(ctx, |ui| {
            egui::containers::menu::menu_style(ui.style_mut());
            Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                actions::drop_items(ui, &menus, (&dropped.dragged, &dropped.onto), &mut |ask| {
                    asked = Some(ask);
                });
            });
        });
    let elsewhere =
        ctx.input(|input| input.pointer.any_click()) && !area.response.contains_pointer();
    let escape = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
    if asked.is_some() || elsewhere || escape {
        view.dropped = None;
    }
    if asked.is_some() {
        view.asked = asked;
    }
}

/// A repository without a commit or a change yet: says so, once the history is read.
fn nothing_yet(ui: &mut Ui, history: &HistoryState) {
    egui::CentralPanel::default()
        .frame(Frame::new().inner_margin(Margin::same(16)))
        .show(ui, |ui| {
            if !history.shown.is_loading() && history.shown.problem().is_none() {
                ui.label(muted(ui, "No commits yet."));
            }
        });
}

/// What the panels beside the commit table show: the working tree's side, and the sidebar's
/// lists.
struct Sides<'a> {
    work: WorkSide<'a>,
    lists: &'a Lists<'a>,
}

/// Carries out the commands asked from the keyboard or the palette, as the buttons and menus
/// doing the same would (the palette checked they can run).
fn queued_commands(shared: &Shared<'_>, view: &mut HistoryView, pane: &mut TaskPane<'_>) {
    if let Some(path) = view.wanted_file.take() {
        // Its changes not staged first, then a new file's, then those staged.
        let found = shared.work_status.and_then(|status| {
            [
                Uncommitted::Unstaged,
                Uncommitted::Untracked,
                Uncommitted::Staged,
            ]
            .into_iter()
            .find_map(|which| WorkFile::find(status, path.as_ref(), which))
        });
        match found {
            Some(work) => {
                view.tool = None;
                view.select_work();
                view.open_work(shared, work);
            }
            None => pane.actions.push(Action::Reveal(
                crate::desktop::in_work_tree(shared.repository.work_tree(), path.as_ref()),
                crate::desktop::Reveal::Open,
            )),
        }
    }
    for command in std::mem::take(&mut view.queued) {
        match command {
            RepoCommand::Fetch => pane.start(crate::tasks::Task::Fetch),
            RepoCommand::Pull => {
                pane.start(crate::tasks::Task::Pull(egit_git::PullMode::AsConfigured));
            }
            RepoCommand::Push => tasks::push_branch(&mut view.tasks, pane),
            RepoCommand::Search => view.open_search(),
            RepoCommand::FilterSidebar => {
                view.sidebar.open = true;
                view.sidebar.focus_filter();
            }
            RepoCommand::ToggleSidebar => view.sidebar.open = !view.sidebar.open,
            RepoCommand::ToggleOutput => {
                pane.actions
                    .push(Action::Output(shared.id, OutputAsk::Toggle));
            }
            RepoCommand::ToggleAgents => pane.actions.push(Action::ToggleAgents(shared.id)),
            RepoCommand::Reflog => view.show_reflog("HEAD".into()),
            RepoCommand::WorkTree => view.select_work(),
            RepoCommand::PullRequests => view.show_pull_request(None),
            RepoCommand::Stash => {
                view.tasks.dialog = Some(super::dialogs::Dialog::Stash {
                    message: String::new(),
                    untracked: true,
                });
            }
            RepoCommand::ApplyPatches => carry_out(Ask::ApplyFiles, shared, view, pane.actions),
            RepoCommand::PastePatch => view.show_patch(PatchSource::Pasted),
            RepoCommand::CreateBranch => {
                if let Some(head) = shared.head_commit() {
                    view.tasks.dialog = Some(super::dialogs::Dialog::CreateBranch {
                        at: head,
                        name: String::new(),
                        switch: true,
                    });
                }
            }
            RepoCommand::Switch(branch) => pane.start(crate::tasks::Task::Switch(
                egit_git::SwitchTo::Branch(branch),
            )),
            // The application's own.
            RepoCommand::Undo | RepoCommand::Redo | RepoCommand::Refresh => {}
        }
    }
}

/// The panel over the commit table: the top bar, and the banners of what stands until it is
/// dealt with (Git LFS or submodules out of step, an operation in progress).
fn bar_panel(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    work_tree: &mut WorkTree,
    view: &mut HistoryView,
    writing: &mut Bind<(), Problem>,
    pane: &mut TaskPane<'_>,
) {
    egui::Panel::top(Id::new(("history-bar", shared.id)))
        .frame(chrome(shared).inner_margin(Margin::symmetric(12, 8)))
        .show(ui, |ui| {
            top_bar(ui, shared, history, work_tree, view, writing, pane);
            sign_in_banner(ui, shared, pane.actions);
            if lfs_banner(ui, shared, pane) {
                view.show_lfs();
            }
            if submodules_banner(ui, shared, view, pane) {
                view.show_submodules();
            }
            let event = tasks::banner(ui, &mut view.tasks, pane);
            task_event(event, history, view);
        });
}

/// The strip under the commit table, while it has something to say: the last task's outcome
/// (one that went well goes by itself after a few seconds; a problem stays until dismissed),
/// and what background fetches brought. Under the table, so nothing above it moves.
fn notices_panel(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
    pane: &mut TaskPane<'_>,
) {
    let outcome = tasks::has_outcome(&mut view.tasks, pane);
    let brought = tasks::has_brought(pane);
    // Not for wanting a sign-in, which the top bar and the banner offer.
    let pipelines = shared
        .forge
        .pipelines_problem()
        .filter(|_| shared.forge.needs_sign_in().is_none())
        .map(ToString::to_string)
        .filter(|why| view.pipelines_dismissed.as_ref() != Some(why));
    if !outcome && !brought && pipelines.is_none() {
        return;
    }
    egui::Panel::bottom(Id::new(("history-notices", shared.id)))
        .resizable(false)
        .frame(chrome(shared).inner_margin(Margin::symmetric(12, 8)))
        .show(ui, |ui| {
            tasks::outcome(ui, &mut view.tasks, pane);
            if outcome && brought {
                ui.add_space(6.0);
            }
            let event = tasks::brought(ui, pane);
            task_event(event, history, view);
            if let Some(why) = pipelines {
                if outcome || brought {
                    ui.add_space(6.0);
                }
                if super::pipeline::unread(ui, shared.id, &why) {
                    view.pipelines_dismissed = Some(why);
                }
            }
        });
}

/// Carries out what a banner or notice asked.
fn task_event(event: Option<TaskEvent>, history: &HistoryState, view: &mut HistoryView) {
    match event {
        Some(TaskEvent::WorkTree) => view.select_work(),
        Some(TaskEvent::Patch) => view.show_patch(PatchSource::Stopped),
        Some(TaskEvent::Commit(commit)) => {
            view.file = None;
            view.tool = None;
            view.show_commit(&history.shown, commit);
        }
        None => {}
    }
}

/// Where submodules are out of step (not set up, at other commits than recorded, in conflict,
/// with stale addresses), a banner saying so and mending them in a click, until dismissed for
/// what it said; returns whether their page was asked.
fn submodules_banner(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    pane: &mut TaskPane<'_>,
) -> bool {
    let Some(standing) = shared
        .submodule_states
        .filter(|standing| !standing.states.is_empty())
    else {
        return false;
    };
    let modules = shared.submodules.map_or(&[][..], |modules| &**modules);
    let out = crate::submodules::OutOfStep::of(modules, standing, shared.config);
    if !out.any()
        || view.submodules_dismissed == Some(out)
        || matches!(view.tool, Some(Tool::Submodules(_)))
    {
        return false;
    }
    let palette = shared.palette;
    let mut open_page = false;
    ui.scope_builder(
        UiBuilder::new().id(Id::new(("submodules-banner", shared.id))),
        |ui| {
            ui.add_space(4.0);
            Frame::new()
                .fill(palette.warn.gamma_multiply(0.10))
                .stroke(Stroke::new(1.0, palette.warn.gamma_multiply(0.7)))
                .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    crate::help::note_topic(ui, ui.max_rect(), "submodule-mend");
                    ui.horizontal(|ui| {
                        let headline = format!("{}  Submodules: {}", icon::PACKAGE, out.words());
                        ui.add(
                            Label::new(RichText::new(headline).color(palette.warn).semibold())
                                .truncate(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_tool(ui, true, icon::X, "Dismiss").clicked() {
                                view.submodules_dismissed = Some(out);
                            }
                            let show = widgets::secondary(ui, true, "Show them");
                            crate::help::note_topic(ui, show.rect, "submodules-page");
                            open_page = show.clicked();
                            if widgets::primary(ui, !shared.busy, "Mend everything")
                            .on_hover_text(
                                "Sync their addresses, then set up every one and bring it to the \
                                 commit recorded, nested ones too",
                            )
                            .clicked()
                        {
                            pane.start(Task::Submodules(crate::submodules::SubmoduleTask::Mend));
                        }
                        });
                    });
                });
        },
    );
    open_page
}

/// Where the hosts are kept that the user said "Not now" to signing in to (their banner hidden,
/// the top bar's button left): in egui's memory, kept across runs.
fn sign_in_later_id() -> Id {
    Id::new("sign-in-later")
}

/// Where a remote is on GitHub or GitLab and nobody is signed in to its host (or it refused the
/// credentials git keeps), a banner saying what signing in shows, Sign in…, and Not now (the
/// host's banner hidden from then on; the top bar's button stays).
fn sign_in_banner(ui: &mut Ui, shared: &Shared<'_>, actions: &mut Vec<Action>) {
    let Some((host, refused)) = shared.forge.needs_sign_in() else {
        return;
    };
    let later = ui.ctx().data_mut(|data| {
        data.get_persisted_mut_or_default::<Vec<String>>(sign_in_later_id())
            .iter()
            .any(|name| name == host.name())
    });
    if later {
        return;
    }
    let palette = shared.palette;
    let kind = host.kind();
    ui.scope_builder(
        UiBuilder::new().id(Id::new(("sign-in-banner", shared.id))),
        |ui| {
            ui.add_space(4.0);
            Frame::new()
                .fill(palette.accent.gamma_multiply(0.10))
                .stroke(Stroke::new(1.0, palette.accent.gamma_multiply(0.7)))
                .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    crate::help::note_topic(ui, ui.max_rect(), "forge-sign-in");
                    let headline = if refused {
                        format!("{} refused the credentials git keeps for it.", host.name())
                    } else {
                        format!(
                            "Sign in to {} to see this repository's {}.",
                            kind.name(),
                            super::sign_in::what_is_read(kind)
                        )
                    };
                    ui.add(
                        Label::new(
                            RichText::new(format!("{}  {headline}", icon::SIGN_IN))
                                .color(palette.text)
                                .semibold(),
                        )
                        .wrap(),
                    );
                    ui.add(
                        Label::new(muted(
                            ui,
                            format!(
                                "egit asks for a personal access token for {}, which your \
                             credential helper keeps for git too.",
                                host.name()
                            ),
                        ))
                        .wrap(),
                    );
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, true, "Sign in…").clicked() {
                            actions.push(Action::SignIn {
                                repo: Some(shared.id),
                                host: host.clone(),
                            });
                        }
                        if widgets::secondary(ui, true, "Not now")
                            .on_hover_text(format!(
                                "Hide this for {}: the top bar's Sign in to {} stays",
                                host.name(),
                                kind.name()
                            ))
                            .clicked()
                        {
                            ui.ctx().data_mut(|data| {
                                data.get_persisted_mut_or_default::<Vec<String>>(sign_in_later_id())
                                .push(host.name().to_owned());
                            });
                        }
                    });
                });
        },
    );
}

/// Where the repository keeps files in Git LFS but they can't be whole here (git-lfs missing,
/// or not set up), a banner saying so and how to mend it; returns whether LFS's page was asked.
fn lfs_banner(ui: &mut Ui, shared: &Shared<'_>, pane: &mut TaskPane<'_>) -> bool {
    let Some(trouble) = shared.lfs.and_then(|lfs| lfs.trouble()) else {
        return false;
    };
    let palette = shared.palette;
    let mut open_page = false;
    ui.scope_builder(UiBuilder::new().id(Id::new(("lfs-banner", shared.id))), |ui| {
        ui.add_space(4.0);
        Frame::new()
            .fill(palette.warn.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0, palette.warn))
            .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::help::note_topic(ui, ui.max_rect(), "lfs");
                let (headline, how) = match trouble {
                    crate::lfs::LfsTrouble::NotInstalled => (
                        "This repository keeps large files in Git LFS, but git-lfs isn't installed.",
                        "Those files are only pointers here. Install git-lfs (its package; Git \
                         for Windows has it), then read the repository again.",
                    ),
                    crate::lfs::LfsTrouble::NotSetUp => (
                        "This repository keeps large files in Git LFS, but it isn't set up here.",
                        "Those files are only pointers, and pushes would leave them behind. \
                         Setting it up adds LFS's filters and hooks to this repository.",
                    ),
                };
                ui.add(
                    Label::new(
                        RichText::new(format!("{}  {headline}", icon::WARNING))
                            .color(palette.warn)
                            .semibold(),
                    )
                    .wrap(),
                );
                ui.add(Label::new(how).wrap());
                ui.horizontal(|ui| {
                    if trouble == crate::lfs::LfsTrouble::NotSetUp
                        && widgets::primary(ui, !shared.busy, "Set up Git LFS here").clicked()
                    {
                        pane.start(Task::Lfs(crate::lfs::LfsTask::Install));
                    }
                    open_page = widgets::secondary(ui, true, "Open Git LFS").clicked();
                });
            });
    });
    open_page
}

/// Patch files dropped on the window, shown to apply (others are left alone: images go to the
/// agents' prompt, while it shows); a hint while files are dragged over it.
fn dropped_patches(ui: &Ui, (id, agents_shown): (RepoId, bool), view: &mut HistoryView) {
    let (hovering, dropped) = ui.ctx().input(|input| {
        let dropped: Vec<PathBuf> = input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .filter(|path| looks_like_patch(BStr::new(path.as_os_str().as_encoded_bytes())))
            .collect();
        (!input.raw.hovered_files.is_empty(), dropped)
    });
    if !dropped.is_empty() {
        view.show_patches(dropped);
    }
    if hovering {
        let palette = Palette::of(ui.ctx());
        egui::Area::new(Id::new(("drop-patches", id)))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.accent))
                    .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        let said = if agents_shown {
                            "Drop patches to look at them, then apply them; or images to \
                             attach them to Claude's prompt"
                        } else {
                            "Drop patches to look at them, then apply them"
                        };
                        ui.label(RichText::new(format!("{}  {said}", icon::GIT_DIFF)).semibold());
                    });
            });
    }
}

/// The dialogs a task asks, and the one opening a pull request.
fn dialogs(
    ctx: &egui::Context,
    shared: &Shared<'_>,
    objects: &Objects,
    view: &mut HistoryView,
    pane: &mut TaskPane<'_>,
) {
    tasks::dialogs(ctx, &mut view.tasks, pane);
    if let Some(task) = pull_form::show(ctx, shared, objects, &mut view.pull_form) {
        pane.actions.push(Action::Task(shared.id, task));
    }
}

/// What the working tree's details need besides: staging, and committing.
pub(crate) struct WorkSide<'a> {
    /// The last staging, while it runs and after.
    pub(crate) staging: &'a mut Bind<(), Problem>,
    /// The last commit made, or being made.
    pub(crate) committing: &'a mut Bind<Committed, Problem>,
    /// How many commits egit has made here.
    pub(crate) commits_made: u64,
    /// Whether commits are signed unless asked otherwise (`commit.gpgSign`).
    pub(crate) sign_by_default: bool,
    /// The commit under way's run (its hooks, their output), while it runs.
    pub(crate) commit_run: Option<&'a crate::output::Run>,
    /// The hooks the repository has.
    pub(crate) hooks: Option<&'a egit_git::Hooks>,
}

/// The least the commit table (or a file looked at) keeps beside the details, where the
/// window has room for it: the details give way first.
const CENTRAL_LEAST: f32 = 360.0;

/// The details beside the history (a commit's, or the working tree's), and the commit table
/// (or a file looked at) in the middle.
fn panels(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &mut HistoryState,
    view: &mut HistoryView,
    sides: Sides<'_>,
    pane: &mut TaskPane<'_>,
    no_commits: bool,
) {
    let actions = &mut *pane.actions;
    let Sides { work, lists } = sides;
    let WorkSide {
        staging,
        committing,
        commits_made,
        sign_by_default,
        commit_run,
        hooks,
    } = work;
    side_panel(ui, shared, history, view, lists, actions);
    // A job's log takes the details' room too: its lines are wide, and nothing is selected.
    let details_shown = !matches!(view.tool, Some(Tool::Job(_)));
    // Never so wide the commit table (or a file) is left without room, where there is room.
    let widest = (ui.available_width() - CENTRAL_LEAST).clamp(260.0, 900.0);
    if details_shown {
        egui::Panel::right(Id::new(("history-details", shared.id)))
            .resizable(true)
            .default_size(380.0_f32.min(widest))
            .size_range(260.0..=widest)
            // Its first row the window's head band, from the top.
            .frame(chrome(shared).inner_margin(Margin {
                left: 12,
                right: 12,
                top: 0,
                bottom: 12,
            }))
            .show(ui, |ui| {
                if view.work_selected {
                    let commit = CommitSide {
                        committing,
                        commits_made,
                        sign_by_default,
                        run: commit_run,
                        hooks,
                        head: shared.head.commit,
                        git: shared.git,
                        repository: shared.repository,
                        objects: &history.objects,
                        me: shared
                            .config
                            .and_then(|config| config.get("user.email"))
                            .and_then(|entry| entry.value),
                    };
                    work_details(ui, shared, view, staging, commit, actions);
                } else {
                    details(ui, shared, history, view);
                }
            });
    }
    if let Some(asked) = view.asked.take() {
        carry_out(asked, shared, view, actions);
    }
    let picked = view.picked_asked.take();
    picked_asked(picked, (shared, &history.objects), view, actions);

    view.close_unselected_file();
    notices_panel(ui, shared, history, view, pane);
    let actions = &mut *pane.actions;
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| match &mut view.file {
            _ if view.tool.is_some() => tool(ui, shared, history, view, actions),
            Some(file) => {
                let open = file::show(ui, shared, &history.objects, file, actions);
                if let Some(dialog) = file.asked.take() {
                    view.tasks.dialog = Some(dialog);
                }
                let wanted = file.wanted.take();
                if !open {
                    view.file = None;
                }
                if let Some(wanted) = wanted {
                    view.open_tool(shared, &history.objects, wanted);
                }
            }
            None if no_commits => {
                Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    ui.label(muted(
                        ui,
                        "No commits yet. Stage the files for the first one, then commit them.",
                    ));
                });
            }
            None => commit_table(ui, shared, history, view, actions),
        });
}

/// The frame of the history's chrome (its bar, the sidebar, the details): a tone of its own
/// around the commit table, which shows on the window's background.
fn chrome(shared: &Shared<'_>) -> Frame {
    Frame::new().fill(shared.palette.panel)
}

/// The sidebar at the left (or the strip showing it again), and what it asks.
fn side_panel(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
    lists: &Lists<'_>,
    actions: &mut Vec<Action>,
) {
    let id = Id::new(("history-sidebar", shared.id, view.sidebar.open));
    let event = if view.sidebar.open {
        egui::Panel::left(id)
            .resizable(true)
            .default_size(260.0)
            .size_range(180.0..=520.0)
            .frame(chrome(shared).inner_margin(Margin::symmetric(6, 0)))
            .show(ui, |ui| {
                sidebar::show(ui, shared, history.shown.refs(), lists, &mut view.sidebar)
            })
            .inner
    } else {
        egui::Panel::left(id)
            .resizable(false)
            .exact_size(sidebar::HIDDEN_WIDTH)
            .frame(chrome(shared))
            .show(ui, |ui| sidebar::hidden(ui, &mut view.sidebar));
        None
    };
    match event {
        Some(SidebarEvent::Ask(asked)) => carry_out(asked, shared, view, actions),
        Some(SidebarEvent::Show(commit)) => {
            view.file = None;
            view.tool = None;
            view.show_commit(&history.shown, commit);
        }
        Some(SidebarEvent::Open(path)) => actions.push(Action::OpenBeside(shared.id, path)),
        Some(SidebarEvent::Reveal(path)) => {
            actions.push(Action::Reveal(path, crate::desktop::Reveal::Open));
        }
        Some(SidebarEvent::Submodules) => view.show_submodules(),
        Some(SidebarEvent::SignIn) => {
            if let Some((host, _)) = shared.forge.needs_sign_in() {
                actions.push(Action::SignIn {
                    repo: Some(shared.id),
                    host,
                });
            }
        }
        None => {}
    }
}

/// What the pane's parts share.
pub(super) struct Shared<'a> {
    pub(super) id: RepoId,
    pub(super) git: Option<&'a Git>,
    pub(super) repository: &'a Arc<Repository>,
    head: HeadAt<'a>,
    pub(super) palette: Palette,
    partial_clone: Option<bool>,
    /// What bringing commits in would do, foreseen for the menus.
    foresight: crate::foresight::Foresight,
    /// The working tree's latest status, once read.
    pub(super) work_status: Option<&'a Arc<Status>>,
    /// The operation git stopped in the middle of, if any (it names a conflict's sides).
    pub(super) operation: Option<Operation>,
    /// Where a bisection under way stands.
    pub(super) bisect: Option<&'a Bisection>,
    /// How the user likes egit.
    pub(super) preferences: Preferences,
    /// The repository's configuration, once read (its remotes, for the menus).
    pub(super) config: Option<&'a ConfigSnapshot>,
    /// Whether a task is running.
    pub(super) busy: bool,
    /// Whether a commit is being made.
    committing: bool,
    /// Its remotes on GitHub and GitLab, and their requests.
    pub(super) forge: &'a RepoForge,
    /// What of the history is shown, as asked.
    pub(super) scope: Scope,
    /// The user's home folder, shown as `~`.
    pub(super) home: Option<&'a Path>,
    /// What egit did here, to take back, and what it took back, to do again.
    journal: &'a Journal,
    /// Whether its agents' panel shows.
    agents_shown: bool,
    /// Its Git LFS, once read.
    pub(super) lfs: Option<&'a Arc<crate::lfs::LfsState>>,
    /// Its submodules, and where they stand (nested ones too), once read.
    pub(super) submodules: Option<&'a Arc<[egit_git::Submodule]>>,
    pub(super) submodule_states: Option<&'a Arc<crate::submodules::Standing>>,
}

impl Shared<'_> {
    /// Whether the repository is on this computer (its files open in its apps).
    pub(super) fn here(&self) -> bool {
        self.git.is_none_or(|git| git.host().is_local())
    }

    /// What the menus need to know.
    pub(super) fn menus<'b>(&'b self, refs: &'b Refs) -> Menus<'b> {
        Menus {
            branch: self.branch(),
            head: self.head.commit,
            config: self.config,
            refs,
            busy: self.busy,
            operation: self.operation.is_some(),
            bisect: self.bisect,
            forge: self.forge,
            scope: &self.scope,
            work_tree: self.repository.work_tree(),
            home: self.home,
            // Never in a partial clone (git would fetch what it merges), nor before that is
            // known.
            foreseeing: (self.partial_clone == Some(false))
                .then_some(self.git.zip(self.head.commit))
                .flatten()
                .map(|(git, head)| super::actions::Foreseeing {
                    foresight: &self.foresight,
                    git,
                    repository: self.repository,
                    head,
                }),
        }
    }

    /// The commit `HEAD` is at, once read.
    pub(super) const fn head_commit(&self) -> Option<ObjectId> {
        self.head.commit
    }

    /// The branch `HEAD` is on, by its short name.
    pub(super) fn branch(&self) -> Option<&BStr> {
        let reference = self.head.reference?;
        Some(
            reference
                .strip_prefix(b"refs/heads/")
                .unwrap_or(reference)
                .as_bstr(),
        )
    }
}

/// Where `HEAD` is: its commit, and the branch it is on.
struct HeadAt<'a> {
    commit: Option<ObjectId>,
    reference: Option<&'a BStr>,
}

impl<'a> HeadAt<'a> {
    fn of(head: Option<&'a Head>) -> Self {
        match head {
            Some(Head::Branch(branch)) => Self {
                commit: branch.commit.as_ref().map(|commit| commit.id),
                reference: Some(branch.reference.as_bstr()),
            },
            Some(Head::Detached(commit)) => Self {
                commit: Some(commit.id),
                reference: None,
            },
            None => Self {
                commit: None,
                reference: None,
            },
        }
    }
}

/// How many commits, whether more are coming, what went wrong, and the commit-graph offer.
fn top_bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    work_tree: &mut WorkTree,
    view: &mut HistoryView,
    writing: &mut Bind<(), Problem>,
    task_pane: &mut TaskPane<'_>,
) {
    let palette = shared.palette;
    let row = ui
        .allocate_space(egui::vec2(ui.available_width(), crate::theme::CONTROL))
        .1;
    // The task bar first, from the right: what is left of it is the views' and menus' room.
    let right = tasks::bar(ui, &mut view.tasks, task_pane, row);
    if !task_pane.tasks.is_busy() {
        view.bar.right = right.width();
    }
    // A running task's progress takes no more than the bar did while idle: the look stays.
    let taken = view.bar.right.max(right.width());
    let room = Rect::from_min_max(
        row.min,
        pos2(
            (row.right() - taken - BAR_GAP).max(row.left()),
            row.bottom(),
        ),
    );
    // Where the room is short for all the bar shows, the views' and menus' buttons keep only
    // their icons. Decided from what the words would add to the bar drawn compact, which
    // doesn't depend on the look drawn: it never flips back and forth.
    let stash = actions::stash_label(task_pane.stashes.len());
    let tools = [
        ((icon::MAGNIFYING_GLASS, "Search"), false),
        ((icon::CLOCK_COUNTER_CLOCKWISE, "Reflog"), false),
        ((icon::ROBOT, "Agents"), false),
        ((icon::ARCHIVE, stash.as_str()), true),
        ((icon::GIT_DIFF, "Patches"), true),
        ((icon::INFO, "Repository"), true),
    ];
    // Git LFS's menu, where the repository keeps files in it; the submodules', where it has
    // some.
    let lfs = shared
        .lfs
        .is_some_and(|lfs| lfs.used())
        .then_some(((icon::HARD_DRIVES, "LFS"), true));
    let modules = shared
        .submodule_states
        .is_some_and(|standing| !standing.states.is_empty())
        .then_some(((icon::PACKAGE, "Submodules"), true));
    let words: f32 = tools
        .iter()
        .chain(lfs.as_ref())
        .chain(modules.as_ref())
        .map(|&(tool, menu)| {
            widgets::tool_width(ui, tool, Look::Full, menu)
                - widgets::tool_width(ui, tool, Look::Compact, menu)
        })
        .sum();
    let measured = if view.bar.compact + words <= room.width() {
        Look::Full
    } else {
        Look::Compact
    };
    // A running task (its progress where the buttons were) changes nothing to the left of it.
    let look = match view.bar.look {
        Some(look) if task_pane.tasks.is_busy() => look,
        _ => measured,
    };
    view.bar.look = Some(look);
    let builder = UiBuilder::new()
        .id_salt("top-bar-left")
        .max_rect(room)
        .layout(Layout::left_to_right(Align::Center));
    let drawn = ui.scope_builder(builder, |ui| {
        // Never over the task bar, however wide.
        ui.set_clip_rect(room.intersect(ui.clip_rect()));
        left_bar(ui, shared, history, work_tree, view, task_pane, look)
    });
    let full = if look == Look::Full { words } else { 0.0 };
    view.bar.compact = drawn.response.rect.width() + drawn.inner - full;
    let actions = &mut *task_pane.actions;
    if !history.shown.scope().is_everything() {
        scope_line(ui, shared, history.shown.scope(), actions);
    }
    for problem in [history.shown.problem(), history.summaries.problem.as_ref()]
        .into_iter()
        .flatten()
    {
        ui.label(
            RichText::new(format!("{}  {problem}", icon::WARNING_CIRCLE)).color(palette.danger),
        );
    }
    if history.shown.commit_graph() == Some(false) && history.shown.rows() >= OFFER_COMMIT_GRAPH_AT
    {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "{}  Without a commit-graph file, git reads all of this history before \
                     showing any of it.",
                    icon::LIGHTNING
                ))
                .color(palette.warn),
            );
            let busy = writing.is_pending();
            if widgets::secondary(ui, !busy, "Write a commit-graph").clicked() {
                actions.push(Action::WriteCommitGraph(shared.id));
            }
            if busy {
                ui.spinner();
            }
        });
        if let Some(problem) = writing.err_ref() {
            ui.label(RichText::new(problem.to_string()).color(palette.danger));
        }
    }
}

/// Between the top bar's left part and its task bar.
const BAR_GAP: f32 = 16.0;

/// What the top bar measured last frame, to choose its look before drawing it.
#[derive(Clone, Copy, Debug, Default)]
struct BarRoom {
    /// Its left part's width with every view and menu compact, room kept for what comes and
    /// goes (the uncommitted files' button, the spinner while the history is read).
    compact: f32,
    /// The task bar's width while no task runs.
    right: f32,
    /// The look drawn last: kept while a task runs.
    look: Option<Look>,
}

/// The top bar's left part: undo and redo, the views and menus (drawn as `look` says), the
/// commit count, the uncommitted files, and the requests. Returns the room kept for what
/// isn't shown now but may come, so its coming never changes the look.
fn left_bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    work_tree: &mut WorkTree,
    view: &mut HistoryView,
    task_pane: &mut TaskPane<'_>,
    look: Look,
) -> f32 {
    // First: nothing before them changes width (a count would), so they stay put.
    undo_buttons(ui, shared, view, task_pane.actions);
    widgets::toolbar_separator(ui);
    search_button(ui, view, look);
    reflog_button(ui, view, look);
    agents_button(ui, shared, look, task_pane.actions);
    let mut asked = None;
    let has_changes = shared.work_status.is_some_and(|status| !status.is_empty());
    let menus = shared.menus(history.shown.refs());
    actions::stash_menu(ui, &menus, task_pane.stashes, has_changes, look, &mut asked);
    let changes = shared
        .work_status
        .map_or_else(actions::WorkChanges::default, |status| {
            actions::WorkChanges {
                staged: status
                    .entries()
                    .any(|entry| entry.staged() != egit_git::Change::Unmodified),
                tracked: status
                    .entries()
                    .any(|entry| entry.kind != egit_git::EntryKind::Untracked),
                has_commit: shared.head_commit().is_some(),
            }
        });
    actions::patches_menu(ui, &menus, changes, look, &mut asked);
    if let Some(asked) = asked {
        carry_out(asked, shared, view, task_pane.actions);
    }
    lfs_menu(ui, shared, view, look, task_pane.actions);
    submodules_menu(ui, shared, view, look, task_pane.actions);
    match repository_menu(ui, shared, look, task_pane.actions) {
        Some(Page::Lfs) => view.show_lfs(),
        Some(Page::Submodules) => view.show_submodules(),
        None => {}
    }
    // What comes and goes follows the buttons: ids are counted in order, and a click
    // pressed on one id and released on another is lost.
    let rows = history.shown.rows();
    let commits = if rows == 1 {
        "1 commit".to_owned()
    } else {
        format!("{} commits", count(rows))
    };
    widgets::toolbar_separator(ui);
    let stats_open = matches!(view.tool, Some(Tool::Stats(_)));
    if widgets::tool(
        ui,
        icon::CHART_BAR,
        &format!("Stats: {commits}"),
        stats_open,
        Look::Full,
    )
    .on_hover_text("The history in figures and charts")
    .clicked()
    {
        view.toggle_stats();
    }
    let spacing = ui.spacing().item_spacing.x;
    let mut kept = 0.0;
    if history.is_loading() {
        ui.add(egui::Spinner::new().size(SPINNER))
            .on_hover_text("Reading the history…");
    } else {
        kept += SPINNER + spacing;
    }
    work_tree_problem(ui, work_tree, shared.palette);
    pull_requests_button(ui, shared, view, task_pane.actions);
    kept
}

/// The top bar's spinner's size, while the history is read.
const SPINNER: f32 = 16.0;

/// A page the Repository menu shows in the commit table's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Lfs,
    Submodules,
}

/// The repository's menu: its overview and settings (each a page in place of the history), Git
/// LFS's and the submodules' pages, and reading it all again; returns the page asked for.
fn repository_menu(
    ui: &mut Ui,
    shared: &Shared<'_>,
    look: Look,
    actions: &mut Vec<Action>,
) -> Option<Page> {
    let mut page = None;
    widgets::tool_menu(ui, icon::INFO, "Repository", look, |ui| {
        ui.set_min_width(220.0);
        for (entry, panel) in [
            ((icon::INFO, "Overview"), RepoPanel::Overview),
            (
                (icon::SLIDERS_HORIZONTAL, "Settings"),
                RepoPanel::Configuration,
            ),
        ] {
            if actions::item(ui, entry, Ok(())) {
                actions.push(Action::ShowPanel(shared.id, panel));
            }
        }
        if actions::item(ui, (icon::PACKAGE, "Submodules…"), Ok(())) {
            page = Some(Page::Submodules);
        }
        if actions::item(ui, (icon::HARD_DRIVES, "Git LFS"), Ok(())) {
            page = Some(Page::Lfs);
        }
        let revealing = if shared.here() {
            Ok(())
        } else {
            Err(super::work::ELSEWHERE)
        };
        if actions::item(ui, (icon::FOLDER_OPEN, "Open the folder"), revealing) {
            let work_tree = shared.repository.work_tree().to_owned();
            actions.push(Action::Reveal(work_tree, crate::desktop::Reveal::Open));
        }
        ui.separator();
        if actions::shortcut_item(ui, (icon::ARROWS_CLOCKWISE, "Read it again"), "⌘R") {
            actions.push(Action::Refresh(shared.id));
        }
    });
    page
}

/// The submodules' menu, where the repository has some: how they stand, mending and updating
/// them all, and their page.
fn submodules_menu(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    look: Look,
    actions: &mut Vec<Action>,
) {
    let Some(standing) = shared
        .submodule_states
        .filter(|standing| !standing.states.is_empty())
    else {
        return;
    };
    let modules = shared.submodules.map_or(&[][..], |modules| &**modules);
    let out = crate::submodules::OutOfStep::of(modules, standing, shared.config);
    let mut page = false;
    widgets::tool_menu(ui, icon::PACKAGE, "Submodules", look, |ui| {
        ui.set_min_width(280.0);
        let header = if out.any() {
            out.words()
        } else {
            "All set up, at the commits recorded".to_owned()
        };
        actions::menu_header(ui, &header);
        let idle = if shared.busy {
            Err("Another task is running")
        } else {
            Ok(())
        };
        let mut task = |task| {
            actions.push(Action::Task(shared.id, Task::Submodules(task)));
        };
        if actions::item(ui, (icon::FIRST_AID_KIT, "Mend everything"), idle) {
            task(crate::submodules::SubmoduleTask::Mend);
        }
        for (glyph, label, to) in [
            (
                icon::ARROWS_CLOCKWISE,
                "Update all to the commits recorded",
                egit_git::UpdateTo::Recorded,
            ),
            (
                icon::ARROW_FAT_LINES_UP,
                "Update all to their branches' latest",
                egit_git::UpdateTo::Latest,
            ),
        ] {
            if actions::item(ui, (glyph, label), idle) {
                task(crate::submodules::SubmoduleTask::Update {
                    paths: Vec::new(),
                    to,
                });
            }
        }
        if actions::item(ui, (icon::LINK, "Sync addresses"), idle) {
            task(crate::submodules::SubmoduleTask::Sync(Vec::new()));
        }
        ui.separator();
        page = actions::item(ui, (icon::PACKAGE, "Each one, and adding one…"), Ok(()));
    });
    if page {
        view.show_submodules();
    }
}

/// Git LFS's menu, where the repository keeps files in it: its state, moving its files, and its
/// page.
fn lfs_menu(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    look: Look,
    actions: &mut Vec<Action>,
) {
    let Some(lfs) = shared.lfs.filter(|lfs| lfs.used()) else {
        return;
    };
    let mut page = false;
    widgets::tool_menu(ui, icon::HARD_DRIVES, "LFS", look, |ui| {
        ui.set_min_width(260.0);
        let state = match (lfs.version, lfs.trouble()) {
            (_, Some(crate::lfs::LfsTrouble::NotInstalled)) => "git-lfs isn't installed".to_owned(),
            (_, Some(crate::lfs::LfsTrouble::NotSetUp)) => "Not set up here".to_owned(),
            (Some(version), None) => format!("git-lfs {version}, set up here"),
            (None, None) => String::new(),
        };
        actions::menu_header(ui, &state);
        let allowed = if shared.busy {
            Err("Another task is running")
        } else if lfs.trouble().is_some() {
            Err("Set Git LFS up first: see its page")
        } else {
            Ok(())
        };
        let mut task = |task| actions.push(Action::Task(shared.id, Task::Lfs(task)));
        for (glyph, label, transfer) in [
            (icon::ARROW_DOWN, "Pull LFS files", LfsTransfer::Pull),
            (
                icon::DOWNLOAD_SIMPLE,
                "Fetch LFS files",
                LfsTransfer::Fetch { all: false },
            ),
            (
                icon::FILE_ARROW_DOWN,
                "Check out LFS files",
                LfsTransfer::Checkout,
            ),
        ] {
            if actions::item(ui, (glyph, label), allowed) {
                task(crate::lfs::LfsTask::Transfer(transfer));
            }
        }
        for remote in shared
            .config
            .map(crate::tasks::remote_names)
            .unwrap_or_default()
        {
            if actions::item(
                ui,
                (icon::ARROW_UP, &format!("Push LFS files to {remote}")),
                allowed,
            ) {
                task(crate::lfs::LfsTask::Transfer(LfsTransfer::Push {
                    remote: remote.as_str().into(),
                }));
            }
        }
        if actions::item(ui, (icon::BROOM, "Prune LFS files"), allowed) {
            task(crate::lfs::LfsTask::Prune);
        }
        ui.separator();
        page = actions::item(
            ui,
            (icon::HARD_DRIVES, "Patterns, files, and locks…"),
            Ok(()),
        );
    });
    if page {
        view.show_lfs();
    }
}

/// The button opening the search bar over the commit table (shown in place of a file or a
/// tool), or closing it.
fn search_button(ui: &mut Ui, view: &mut HistoryView, look: Look) {
    let open = view.search.is_open();
    if widgets::tool(ui, icon::MAGNIFYING_GLASS, "Search", open, look)
        .on_hover_text(format!(
            "Search the history ({})",
            crate::commands::keys_text("⌘F")
        ))
        .clicked()
    {
        if open {
            view.search.close();
        } else {
            view.open_search();
        }
    }
}

/// Shows or hides the agents' panel at the tab's right.
fn agents_button(ui: &mut Ui, shared: &Shared<'_>, look: Look, actions: &mut Vec<Action>) {
    if widgets::tool(ui, icon::ROBOT, "Agents", shared.agents_shown, look)
        .on_hover_text(format!(
            "Claude Code, working in this repository beside you ({})",
            crate::commands::keys_text("⌘L")
        ))
        .clicked()
    {
        actions.push(Action::ToggleAgents(shared.id));
    }
}

/// Undo and Redo, saying what each would take back or do again; ⌘Z and ⇧⌘Z while no text field
/// or dialog has the keys.
fn undo_buttons(ui: &mut Ui, shared: &Shared<'_>, view: &HistoryView, actions: &mut Vec<Action>) {
    let idle = !shared.busy && !shared.committing;
    let keys = idle && view.tasks.dialog.is_none() && !ui.ctx().text_edit_focused();
    let (undo, redo) = (shared.journal.next_undo(), shared.journal.next_redo());
    let pressed = |modifiers| {
        keys && ui
            .input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(modifiers, Key::Z)))
    };
    // Shift first: ⌘Z alone would match ⇧⌘Z too.
    let shifted = pressed(Modifiers::COMMAND | Modifiers::SHIFT);
    let plain = !shifted && pressed(Modifiers::COMMAND);
    let mut asked = if shifted && redo.is_some() {
        Some(true)
    } else if plain && undo.is_some() {
        Some(false)
    } else {
        None
    };
    for (glyph, name, next, redoing, keys) in [
        (icon::ARROW_COUNTER_CLOCKWISE, "Undo", undo, false, "⌘Z"),
        (icon::ARROW_CLOCKWISE, "Redo", redo, true, "⇧⌘Z"),
    ] {
        let enabled = idle && next.is_some();
        // Named in words, for those who don't see the arrow.
        let button = widgets::icon_tool(ui, enabled, glyph, name);
        let hover = next.map_or_else(
            || {
                if redoing {
                    "Nothing taken back to do again".to_owned()
                } else {
                    "Nothing egit did here to take back".to_owned()
                }
            },
            |next| {
                format!(
                    "{name} {} ({})",
                    next.what,
                    crate::commands::keys_text(keys)
                )
            },
        );
        let button = if enabled {
            button.on_hover_text(hover)
        } else if next.is_some() {
            button.on_disabled_hover_text(format!("{hover}: once the task running ends"))
        } else {
            button.on_disabled_hover_text(hover)
        };
        if button.clicked() {
            asked = Some(redoing);
        }
    }
    if let Some(redo) = asked {
        actions.push(Action::Undo(shared.id, redo));
    }
}

/// The button showing `HEAD`'s reflog in the commit table's place (or going back).
fn reflog_button(ui: &mut Ui, view: &mut HistoryView, look: Look) {
    let showing = matches!(view.tool, Some(Tool::Reflog(_)));
    if widgets::tool(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Reflog", showing, look)
        .on_hover_text("Where HEAD has been: every commit it was at, and what moved it")
        .clicked()
    {
        if showing {
            view.tool = None;
        } else {
            view.show_reflog("HEAD".into());
        }
    }
}

/// What of the history is shown, when not all of it as it comes: the branches it is narrowed
/// to (and showing every branch again), the branch pinned to the left (and unpinning it). In a
/// `Ui` of its own: it comes and goes.
fn scope_line(ui: &mut Ui, shared: &Shared<'_>, applied: &Scope, actions: &mut Vec<Action>) {
    let short = |name: &bstr::BString| {
        let name = name.as_slice();
        name.strip_prefix(b"refs/heads/")
            .or_else(|| name.strip_prefix(b"refs/remotes/"))
            .unwrap_or(name)
            .to_str_lossy()
            .into_owned()
    };
    let palette = shared.palette;
    let builder = egui::UiBuilder::new().id(Id::new(("history-scope", shared.id)));
    ui.scope_builder(builder, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if !applied.only.is_empty() {
                let names: Vec<String> = applied.only.iter().map(short).collect();
                ui.label(
                    RichText::new(format!(
                        "{}  Showing only {}",
                        icon::FUNNEL,
                        names.join(", ")
                    ))
                    .color(palette.muted),
                );
                if widgets::secondary(ui, true, "Show every branch").clicked() {
                    let scope = Scope {
                        only: Vec::new(),
                        ..shared.scope.clone()
                    };
                    actions.push(Action::Scope(shared.id, scope));
                }
            }
            if !applied.hidden.is_empty() {
                let hidden = applied.hidden.len();
                let text = if hidden == 1 {
                    format!("{}  1 branch hidden", icon::EYE_SLASH)
                } else {
                    format!("{}  {hidden} branches hidden", icon::EYE_SLASH)
                };
                ui.label(RichText::new(text).color(palette.muted));
                if widgets::secondary(ui, true, "Show them").clicked() {
                    let scope = Scope {
                        hidden: Vec::new(),
                        ..shared.scope.clone()
                    };
                    actions.push(Action::Scope(shared.id, scope));
                }
            }
            if let Some(pinned) = &applied.pinned {
                ui.label(
                    RichText::new(format!(
                        "{}  {} pinned to the left",
                        icon::PUSH_PIN,
                        short(pinned)
                    ))
                    .color(palette.muted),
                );
                if widgets::secondary(ui, true, "Unpin").clicked() {
                    let scope = Scope {
                        pinned: None,
                        ..shared.scope.clone()
                    };
                    actions.push(Action::Scope(shared.id, scope));
                }
            }
        });
    });
}

/// The pull (or merge) requests' button, when a remote is on GitHub or GitLab: how many are
/// open; or, where they can't be read for want of a sign-in, the way to sign in.
fn pull_requests_button(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    actions: &mut Vec<Action>,
) {
    if shared.forge.repositories.is_empty() {
        return;
    }
    if let Some((host, refused)) = shared.forge.needs_sign_in() {
        let text = format!("Sign in to {}", host.kind().name());
        let why = if refused {
            format!("{} refused the credentials git keeps", host.name())
        } else {
            format!(
                "Signed in to {}, egit shows its {}",
                host.name(),
                super::sign_in::what_is_read(host.kind())
            )
        };
        if widgets::tool(ui, icon::SIGN_IN, &text, false, Look::Full)
            .on_hover_text(why)
            .clicked()
        {
            actions.push(Action::SignIn {
                repo: Some(shared.id),
                host,
            });
        }
        return;
    }
    let open = shared.forge.found().pulls.len();
    let nouns = shared.forge.nouns();
    let text = match open {
        0 => {
            let mut letters = nouns.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        }
        1 => format!("1 {}", nouns.trim_end_matches('s')),
        open => format!("{} {nouns}", count(u64::try_from(open).unwrap_or(u64::MAX))),
    };
    let showing = matches!(view.tool, Some(Tool::PullRequests(_)));
    if widgets::tool(ui, icon::GIT_PULL_REQUEST, &text, showing, Look::Full)
        .on_hover_text(format!("The open {nouns}"))
        .clicked()
    {
        if showing {
            view.tool = None;
        } else {
            view.show_pull_request(None);
        }
    }
}

/// Why the work tree's changes aren't known, when they couldn't be read (the work in
/// progress's row shows them when they are).
fn work_tree_problem(ui: &mut Ui, work_tree: &mut WorkTree, palette: Palette) {
    if let Some(Err(problem)) = work_tree.status() {
        let problem = problem.to_string();
        ui.label(
            RichText::new(format!(
                "{}  The work tree couldn't be read",
                icon::WARNING_CIRCLE
            ))
            .color(palette.danger),
        )
        .on_hover_text(problem);
    }
}

/// The search bar (when open), the commit table, and the keys that move through it.
fn commit_table(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &mut HistoryState,
    view: &mut HistoryView,
    actions: &mut Vec<Action>,
) {
    if ui.input_mut(|input| {
        input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))
    }) {
        view.search.open();
    }
    let selected = view.selected.map(|selected| selected.row);
    if let Some(row) = search::bar(ui, shared, &history.shown, &mut view.search, selected) {
        view.select(&history.shown, row, Some(Align::Center));
    }
    let keys = ui.id().with("commit-keys");
    let area = ui.interact(
        ui.available_rect_before_wrap(),
        keys,
        Sense::focusable_noninteractive(),
    );
    area.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Commits"));
    if area.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                keys,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..egui::EventFilter::default()
                },
            );
        });
        move_by_keys(ui, &history.shown, view);
        if view.picked.is_several()
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            view.picked.clear();
        }
    }

    let salt = ("history", shared.id);
    let place = ui.available_rect_before_wrap();
    let room = place.width() - ui.spacing().scroll.allocated_width();
    let marks = shared.forge.has_pipelines();
    let widths = column_widths(ui, room, (history.shown.widest(), marks));
    let columns = columns(widths);
    let all = columns.len();
    // Under the titles, pinned as they are, the work in progress when there is some: the
    // graph's column, then the rest as one.
    let mut headers = vec![HeaderRow::new(HEADER_HEIGHT)];
    let mut height = HEADER_HEIGHT;
    if let Some(status) = shared.work_status.filter(|status| !status.is_empty()) {
        height += ROW_HEIGHT;
        view.work_counts.of(status);
        headers.push(HeaderRow {
            height: ROW_HEIGHT,
            groups: vec![0..1, 1..all],
        });
    }
    let mut table = Table::new()
        .id_salt(salt)
        .num_rows(history.shown.rows())
        .columns(columns)
        .headers(headers);
    if let Some((row, align)) = view.scroll_to.take() {
        table = table.scroll_to_row(row, align);
    }
    let HistoryState {
        shown,
        summaries,
        signatures,
        objects,
        ..
    } = history;
    let mut rows = CommitRows {
        shared,
        history: shown,
        summaries,
        signatures,
        objects,
        view,
        clicked: None,
        asked: None,
        picked: None,
        now: crate::format::now(),
    };
    table.show(ui, &mut rows);
    let shown_rows = f32::from(u16::try_from(shown.rows()).unwrap_or(u16::MAX));
    let work = height > HEADER_HEIGHT;
    column_lines(
        ui,
        place,
        widths,
        (shown_rows.mul_add(ROW_HEIGHT, height), work),
    );
    let asked = rows.asked.take();
    let picked_event = rows.picked.take();
    if let Some((row, click)) = rows.clicked {
        let before = view.selected.map(|selected| (selected.row, selected.id));
        view.picked.click(shown, row, before, click);
        view.select(shown, row, None);
        ui.memory_mut(|memory| memory.request_focus(keys));
    }
    picked_asked(picked_event, (shared, &history.objects), view, actions);
    if let Some(asked) = asked {
        carry_out(asked, shared, view, actions);
    }
}

/// Carries out what several commits picked were asked for (by their menu or their panel).
fn picked_asked(
    event: Option<PickedEvent>,
    (shared, objects): (&Shared<'_>, &Objects),
    view: &mut HistoryView,
    actions: &mut Vec<Action>,
) {
    match event {
        Some(PickedEvent::Sign(commits)) => {
            let dialog = super::dialogs::Dialog::SignCommits(commits);
            carry_out(Ask::Dialog(dialog), shared, view, actions);
        }
        Some(PickedEvent::Compare { from, to }) => {
            view.compare = Some(Comparison::new(shared, objects, from, to));
        }
        Some(PickedEvent::Export(commits, how)) => export(commits, how, shared, actions),
        Some(PickedEvent::Clear) => view.picked.clear(),
        None => {}
    }
}

/// Exports commits picked (the newest first) as asked: saved where the user says, or copied
/// by a task.
fn export(commits: Vec<ObjectId>, how: Exporting, shared: &Shared<'_>, actions: &mut Vec<Action>) {
    let copy = |how| {
        Action::Task(
            shared.id,
            crate::tasks::Task::Export {
                commits: commits.clone(),
                how,
            },
        )
    };
    match how {
        Exporting::SaveOne | Exporting::SaveEach => {
            let one = how == Exporting::SaveOne;
            let count = commits.len();
            actions.push(Action::AskFiles {
                question: FileQuestion::SavePicked {
                    repo: shared.id,
                    commits: Arc::from(commits),
                    one,
                },
                title: if one {
                    "Save their changes as one patch"
                } else {
                    "Save the patches in"
                },
                name: if one {
                    format!("changes-of-{count}-commits.patch")
                } else {
                    String::new()
                },
            });
        }
        Exporting::CopyOne => actions.push(copy(Export::CopyOnePatch)),
        Exporting::CopyEach => actions.push(copy(Export::CopyPatches)),
        Exporting::Changelog => {
            let forge = shared.forge.repositories.first().cloned();
            actions.push(copy(Export::CopyChangelog(forge)));
        }
    }
}

/// Where the details' line between a commit's message and facts and its changed files was
/// dragged to: the message's share of the panel's height (kept with the settings).
fn split_id() -> Id {
    Id::new("details-split")
}

/// The scroll area of a commit's message and facts: `room` tall exactly, as the line below it
/// was dragged, else as tall as they need up to half the panel (`height`).
fn message_area(room: Option<f32>, height: f32) -> egui::ScrollArea {
    let area = egui::ScrollArea::vertical().id_salt("commit-message");
    match room {
        Some(room) => area
            .min_scrolled_height(room)
            .max_height(room)
            .auto_shrink([false, false]),
        None => area.max_height(height * 0.5).auto_shrink([false, true]),
    }
}

/// The least room either side of the details' line keeps.
const SPLIT_LEAST: f32 = 96.0;

/// The message's room as the line was dragged (`None`: never, or put back), within a panel
/// `height` tall.
fn split_room(ui: &Ui, height: f32) -> Option<f32> {
    let share = ui
        .ctx()
        .data_mut(|data| data.get_persisted::<f32>(split_id()))?;
    let most = (height - SPLIT_LEAST).max(SPLIT_LEAST);
    Some((height * share).clamp(SPLIT_LEAST, most))
}

/// The line between a commit's message and facts (`top` tall now) and its changed files, in a
/// panel `height` tall: dragged, it gives either side more room, and holds; a double click puts
/// it back (the message as tall as it needs, up to half).
fn split_line(ui: &mut Ui, height: f32, top: f32) {
    let palette = Palette::of(ui.ctx());
    let (place, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), Sense::hover());
    // A thin handle (its line the place's middle), as a column's line is.
    let grab = Rect::from_center_size(place.center(), egui::vec2(place.width(), 6.0));
    let response = ui.interact(grab, Id::new("details-split-line"), Sense::click_and_drag());
    crate::help::note_topic(ui, place, "details-split");
    let active = response.hovered() || response.dragged();
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let stroke = if response.dragged() {
        Stroke::new(2.0, palette.accent)
    } else if active {
        Stroke::new(1.0, palette.border_strong)
    } else {
        Stroke::new(1.0, palette.border)
    };
    ui.painter()
        .hline(place.x_range(), place.center().y, stroke);
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(Id::new("details-split-y"), place.center().y));
    if response.double_clicked() {
        ui.ctx().data_mut(|data| data.remove::<f32>(split_id()));
    } else if response.dragged() && height > 0.0 {
        let share = (top + response.drag_delta().y) / height;
        ui.ctx()
            .data_mut(|data| data.insert_persisted(split_id(), share.clamp(0.0, 1.0)));
        ui.ctx().request_repaint();
    }
}

/// A job's log in the commit table's place; returns whether it went back to its pipeline.
fn job_log(
    ui: &mut Ui,
    shared: &Shared<'_>,
    log: &mut super::job::JobView,
    actions: &mut Vec<Action>,
) -> bool {
    match super::job::show(ui, shared, log) {
        Some(super::job::JobEvent::Back) => true,
        Some(super::job::JobEvent::Open(url)) => {
            actions.push(Action::OpenUrl(url));
            false
        }
        None => false,
    }
}

/// Opens a job's log in its pipeline's page's place (`opened`), or goes back to that page
/// (`left`).
fn switch_job_tool(
    ui: &Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    (opened, left): (Option<Box<egit_forge::Job>>, bool),
) {
    if let Some(job) = opened
        && let Some(Tool::Pipeline(page)) = view.tool.take()
    {
        view.tool = Some(Tool::Job(Box::new(super::job::JobView::new(
            page,
            *job,
            shared.forge,
            ui.ctx(),
        ))));
    } else if left && let Some(Tool::Job(log)) = view.tool.take() {
        view.tool = Some(Tool::Pipeline(log.pipeline));
    }
}

/// The tool in the commit table's place, and what it asks.
fn tool(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
    actions: &mut Vec<Action>,
) {
    let (mut menu, mut wanted_file) = (None, None);
    // A job's log opened from its pipeline's page, or that page gone back to.
    let (mut job_opened, mut job_left) = (None, false);
    let asked = match &mut view.tool {
        Some(Tool::Rebase(rebase)) => match rebase::show(ui, shared, rebase) {
            Some(RebaseEvent::Start(task)) => {
                actions.push(Action::Task(shared.id, task));
                Some(BlameEvent::Close)
            }
            Some(RebaseEvent::Close) => Some(BlameEvent::Close),
            None => None,
        },
        Some(Tool::Blame(blame)) => blame::show(ui, shared, blame),
        Some(Tool::Patch(patch)) => match patch::show(ui, shared, patch) {
            Some(PatchEvent::Apply(task)) => {
                actions.push(Action::Task(shared.id, task));
                Some(BlameEvent::Close)
            }
            Some(PatchEvent::Close) => Some(BlameEvent::Close),
            None => None,
        },
        Some(Tool::FileHistory(found)) => file_history::show(ui, shared, found),
        Some(Tool::PullRequests(pulls)) => pulls::show(ui, shared, history, pulls)
            .and_then(|asked| pull_request_event(asked, shared, actions)),
        Some(Tool::Pipeline(page)) => {
            pipeline::show(ui, shared, page).and_then(|asked| match asked {
                PipelineEvent::Close => Some(BlameEvent::Close),
                PipelineEvent::ShowCommit(id) => Some(BlameEvent::ShowCommit(id)),
                PipelineEvent::Open(url) => {
                    actions.push(Action::OpenUrl(url));
                    None
                }
                PipelineEvent::Job(job) => {
                    job_opened = Some(job);
                    None
                }
            })
        }
        Some(Tool::Job(log)) => {
            job_left = job_log(ui, shared, log, actions);
            None
        }
        Some(Tool::Submodules(page)) => super::submodules::show(ui, shared, page)
            .and_then(|event| submodules_event(event, shared, actions)),
        Some(Tool::Stats(page)) => super::stats::show(ui, shared, &history.shown, page)
            .map(|StatsEvent::Close| BlameEvent::Close),
        Some(Tool::Lfs(page)) => match super::lfs::show(ui, shared, shared.lfs, page) {
            Some(LfsEvent::Close) => Some(BlameEvent::Close),
            Some(LfsEvent::Task(task)) => {
                actions.push(Action::Task(shared.id, task));
                None
            }
            None => None,
        },
        Some(Tool::Reflog(found)) => {
            let read = (&history.shown, &history.objects);
            match reflog::show(ui, shared, read, found, actions) {
                Some(ReflogEvent::Close) => Some(BlameEvent::Close),
                Some(ReflogEvent::ShowCommit(id)) => Some(BlameEvent::ShowCommit(id)),
                Some(ReflogEvent::Ask(asked)) => {
                    menu = Some(asked);
                    None
                }
                Some(ReflogEvent::Wanted(wanted)) => {
                    wanted_file = Some(wanted);
                    None
                }
                None => None,
            }
        }
        None => None,
    };
    switch_job_tool(ui, shared, view, (job_opened, job_left));
    if let Some(asked) = menu {
        carry_out(asked, shared, view, actions);
    }
    if let Some(wanted) = wanted_file {
        view.open_tool(shared, &history.objects, wanted);
    }
    match asked {
        Some(BlameEvent::Close) => view.close_tool(),
        Some(BlameEvent::ShowCommit(id)) => {
            view.tool = None;
            let near = view.selected.map_or(0, |selected| selected.row);
            if let Some(row) = history.shown.row_of(&id, near) {
                view.select(&history.shown, row, Some(Align::Center));
            }
        }
        Some(BlameEvent::Blame { at, path }) => {
            view.open_tool(
                shared,
                &history.objects,
                FileWanted::Blame { at: Some(at), path },
            );
        }
        None => {}
    }
}

/// Does what the submodules' page asked: its tasks, a tab opened, a submodule's commit
/// recorded; returns whether it closes.
fn submodules_event(
    event: SubmodulesEvent,
    shared: &Shared<'_>,
    actions: &mut Vec<Action>,
) -> Option<BlameEvent> {
    match event {
        SubmodulesEvent::Close => return Some(BlameEvent::Close),
        SubmodulesEvent::Task(task) => actions.push(Action::Task(shared.id, task)),
        SubmodulesEvent::Open(folder) => actions.push(Action::OpenBeside(shared.id, folder)),
        SubmodulesEvent::Record(path) => actions.push(Action::Stage(
            shared.id,
            crate::staging::StageRequest::Files {
                paths: vec![path],
                direction: egit_git::Direction::Stage,
            },
        )),
    }
    None
}

/// What the pull requests' page asked: the history view's part of it (closing it, showing a
/// commit), or the application's (a task, a page, a sign-in, reading them again).
fn pull_request_event(
    asked: PullRequestEvent,
    shared: &Shared<'_>,
    actions: &mut Vec<Action>,
) -> Option<BlameEvent> {
    match asked {
        PullRequestEvent::Close => return Some(BlameEvent::Close),
        PullRequestEvent::ShowCommit(id) => return Some(BlameEvent::ShowCommit(id)),
        PullRequestEvent::Task(task) => actions.push(Action::Task(shared.id, task)),
        PullRequestEvent::Open(url) => actions.push(Action::OpenUrl(url)),
        PullRequestEvent::SignIn(host) => actions.push(Action::SignIn {
            repo: Some(shared.id),
            host,
        }),
        PullRequestEvent::Refresh => actions.push(Action::RefreshPullRequests(shared.id)),
    }
    None
}

/// Does what a menu asked: starts its task, asks its dialog's question first, or opens the
/// rebase editor.
fn carry_out(asked: Ask, shared: &Shared<'_>, view: &mut HistoryView, actions: &mut Vec<Action>) {
    match asked {
        Ask::Task(task) => actions.push(Action::Task(shared.id, task)),
        Ask::Dialog(dialog) => view.tasks.dialog = Some(dialog),
        Ask::Rebase { from, reword } => {
            view.file = None;
            view.tool = Some(Tool::Rebase(RebaseView::new(shared, from, reword)));
        }
        Ask::Open(url) => actions.push(Action::OpenUrl(url)),
        Ask::Dropped { dragged, onto, at } => view.dropped = Some(Dropped { dragged, onto, at }),
        Ask::Preferences(preferences) => actions.push(Action::SetPreferences(preferences)),
        Ask::Scope(scope) => actions.push(Action::Scope(shared.id, scope)),
        Ask::SavePatch { commit, name } => actions.push(Action::AskFiles {
            question: FileQuestion::SavePatch {
                repo: shared.id,
                commit,
            },
            title: "Save the patch",
            name,
        }),
        Ask::SavePatches { from } => actions.push(Action::AskFiles {
            question: FileQuestion::SavePatches {
                repo: shared.id,
                from,
            },
            title: "Save the patches in",
            name: String::new(),
        }),
        Ask::SaveChanges(changes) => actions.push(Action::AskFiles {
            question: FileQuestion::SaveChanges {
                repo: shared.id,
                changes,
            },
            title: "Save the patch",
            name: match changes {
                Changes::Staged => "staged.patch".to_owned(),
                Changes::All => "changes.patch".to_owned(),
            },
        }),
        Ask::ApplyFiles => actions.push(Action::AskFiles {
            question: FileQuestion::ApplyPatches { repo: shared.id },
            title: "Apply patches",
            name: String::new(),
        }),
        Ask::Paste => view.show_patch(PatchSource::Pasted),
        Ask::Reflog(reference) => view.show_reflog(reference),
        Ask::PullRequest(selected) => view.show_pull_request(selected),
        Ask::Pipeline(mark) => view.show_pipeline(*mark),
        Ask::CreatePullRequest {
            branch,
            remote,
            head_branch,
            head,
        } => {
            view.pull_form = Some(PullRequestForm::new(branch, remote, head_branch, head));
        }
    }
}

/// The commit table's columns' widths as the user set them, by dragging the lines between
/// them (egit's own: [`column_lines`]); kept in egui's memory, so they hold across runs and
/// repositories. The message takes what the others leave.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ColumnWidths {
    /// The graph's, once set: until then as wide as its first lanes need.
    graph: Option<f32>,
    author: f32,
    date: f32,
    commit: f32,
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            graph: None,
            author: AUTHOR_WIDTH,
            date: DATE_WIDTH,
            commit: COMMIT_WIDTH,
        }
    }
}

/// The columns' first widths, and how narrow and wide each may be.
const AUTHOR_WIDTH: f32 = 150.0;
const DATE_WIDTH: f32 = 140.0;
const COMMIT_WIDTH: f32 = 84.0;
const GRAPH_RANGE: (f32, f32) = (GRAPH_MARGIN * 2.0 + LANE_WIDTH, 1_200.0);
const AUTHOR_RANGE: (f32, f32) = (60.0, 480.0);
const DATE_RANGE: (f32, f32) = (90.0, 240.0);
const COMMIT_RANGE: (f32, f32) = (60.0, 240.0);

/// Where the widths are kept.
fn widths_id() -> Id {
    Id::new("commit-table-columns")
}

/// This frame's widths, graph, message, author, date, commit: the message's what the others
/// leave of `room` (never under its least); the commit's wider than kept where commits have
/// pipelines' `marks` (one beside each name), as drawn only.
fn column_widths(ui: &Ui, room: f32, (widest, marks): (usize, bool)) -> [f32; 5] {
    let marks = if marks {
        pipeline::MARK_WIDTH + ui.spacing().item_spacing.x
    } else {
        0.0
    };
    let kept: ColumnWidths = ui
        .ctx()
        .data_mut(|data| *data.get_persisted_mut_or_default::<ColumnWidths>(widths_id()));
    let lanes = widest.clamp(1, FIRST_LANES);
    let graph = kept.graph.unwrap_or_else(|| {
        f32::from(u16::try_from(lanes).unwrap_or(1))
            .mul_add(LANE_WIDTH, GRAPH_MARGIN * 2.0)
            .max(FIRST_GRAPH_WIDTH)
    });
    let mut widths = [
        graph,
        MIN_MESSAGE_WIDTH,
        kept.author,
        kept.date,
        kept.commit + marks,
    ];
    // Too narrow for each at its width (a panel open beside): the others give way, as drawn
    // only (what is kept stays), the graph first (it pans), then the author, the date, and
    // the commit, each down to its least, so the rows end where the table does.
    let mut over = widths.iter().sum::<f32>() - room;
    for (at, least) in [
        (0, GRAPH_RANGE.0),
        (2, AUTHOR_RANGE.0),
        (3, DATE_RANGE.0),
        (4, COMMIT_RANGE.0),
    ] {
        if let Some(width) = widths.get_mut(at)
            && over > 0.0
        {
            let given = (*width - least).clamp(0.0, over);
            *width -= given;
            over -= given;
        }
    }
    if let Some(message) = widths.get_mut(1) {
        *message = (MIN_MESSAGE_WIDTH - over).max(MIN_MESSAGE_WIDTH);
    }
    widths
}

/// The table's columns at `widths`: none resizable by table-kit, whose resizing moves a
/// column's right edge (the message, filling what is left, would then move its left one along,
/// each frame); egit moves the lines between them itself ([`column_lines`]). Ids of their own,
/// so widths table-kit kept for the old columns don't apply.
fn columns(widths: [f32; 5]) -> Vec<Column> {
    ["graph", "message", "author", "date", "commit"]
        .iter()
        .zip(widths)
        .map(|(name, width)| {
            Column::new(width)
                .range(width..=width)
                .resizable(false)
                .id(Id::new(("commit-column", *name)))
        })
        .collect()
}

/// The lines between the columns, drawn over the table (`place`: where it is, `widths` as
/// drawn, `height` its titles' and rows', `work`: whether the work in progress's row, one cell
/// right of the graph, is under the titles), each dragged to move, trading width between the
/// columns either side of it (the message's by filling what is left), so it follows the
/// pointer and nothing else moves. A double click puts both back as they were at first.
fn column_lines(ui: &Ui, place: Rect, widths: [f32; 5], (height, work): (f32, bool)) {
    let [graph, message, author, date, _] = widths;
    let left = place.left();
    let xs = [
        left + graph,
        left + graph + message,
        left + graph + message + author,
        left + graph + message + author + date,
    ];
    let bottom = (place.top() + height).min(place.bottom());
    let titles = place.top() + HEADER_HEIGHT;
    // Where the lines are, for tests to drag them.
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(Id::new("commit-column-xs"), xs));
    let palette = Palette::of(ui.ctx());
    let radius = ui.style().interaction.resize_grab_radius_side;
    for (at, x) in xs.into_iter().enumerate() {
        let line = Rect::from_x_y_ranges(x - radius..=x + radius, place.top()..=bottom);
        let response = ui.interact(
            line,
            Id::new(("commit-column-line", at)),
            Sense::click_and_drag(),
        );
        let active = response.hovered() || response.dragged();
        if active {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        let stroke = if response.dragged() {
            Stroke::new(2.0, palette.accent)
        } else if active {
            Stroke::new(1.0, palette.border_strong)
        } else {
            Stroke::new(1.0, palette.border)
        };
        // Through the titles and the rows; the work in progress's row is one cell past the
        // graph's.
        let painter = ui.painter();
        if work && at > 0 {
            painter.vline(x, egui::Rangef::new(place.top(), titles), stroke);
            painter.vline(x, egui::Rangef::new(titles + ROW_HEIGHT, bottom), stroke);
        } else {
            painter.vline(x, egui::Rangef::new(place.top(), bottom), stroke);
        }
        let mut kept: ColumnWidths = ui
            .ctx()
            .data_mut(|data| *data.get_persisted_mut_or_default::<ColumnWidths>(widths_id()));
        let before = kept;
        if response.double_clicked() {
            reset_columns(&mut kept, at);
        } else if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            move_line(&mut kept, at, (left, widths, pointer.x));
        }
        if kept != before {
            ui.ctx()
                .data_mut(|data| data.insert_persisted(widths_id(), kept));
            ui.ctx().request_repaint();
        }
    }
}

/// Puts the columns line `at` trades between back as they were at first.
fn reset_columns(kept: &mut ColumnWidths, at: usize) {
    let first = ColumnWidths::default();
    match at {
        0 => kept.graph = None,
        1 => kept.author = first.author,
        2 => {
            kept.author = first.author;
            kept.date = first.date;
        }
        _ => {
            kept.date = first.date;
            kept.commit = first.commit;
        }
    }
}

/// Moves line `at` to `x` (`left`: the table's left edge): the columns either side take the
/// widths that put it there, within their ranges; beside the message, the message fills what is
/// left, never under its least.
fn move_line(kept: &mut ColumnWidths, at: usize, (left, widths, x): (f32, [f32; 5], f32)) {
    let [graph, message, author, date, commit] = widths;
    // How much a column beside the message may grow before the message is at its least.
    let spare = (message - MIN_MESSAGE_WIDTH).max(0.0);
    let grow = |wanted: f32, current: f32, (least, most): (f32, f32)| {
        wanted.min(current + spare).clamp(least, most)
    };
    // Two columns trading width: the left one's within both their ranges.
    let trade = |wanted: f32, both: f32, (first, second): ((f32, f32), (f32, f32))| {
        let least = first.0.max(both - second.1);
        let most = first.1.min(both - second.0);
        let first = if least <= most {
            wanted.clamp(least, most)
        } else {
            wanted
        };
        (first, both - first)
    };
    let author_left = left + graph + message;
    match at {
        0 => kept.graph = Some(grow(x - left, graph, GRAPH_RANGE)),
        1 => kept.author = grow(author_left + author - x, author, AUTHOR_RANGE),
        2 => {
            let (wide, narrow) = trade(x - author_left, author + date, (AUTHOR_RANGE, DATE_RANGE));
            kept.author = wide;
            kept.date = narrow;
        }
        _ => {
            let date_left = author_left + author;
            let (wide, narrow) = trade(x - date_left, date + commit, (DATE_RANGE, COMMIT_RANGE));
            kept.date = wide;
            kept.commit = narrow;
        }
    }
}

/// Moves the selection with the arrow, page, Home, and End keys.
fn move_by_keys(ui: &Ui, history: &History, view: &mut HistoryView) {
    let last = history.rows().saturating_sub(1);
    let page = view
        .visible
        .end
        .saturating_sub(view.visible.start)
        .saturating_sub(1)
        .max(1);
    let current = view.selected.map(|selected| selected.row);
    let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
    // ⇧↑ and ⇧↓ stretch the commits picked from where picking started.
    let stretched = |key| ui.input_mut(|input| input.consume_key(Modifiers::SHIFT, key));
    let stretch = if stretched(Key::ArrowDown) {
        Some(current.map_or(0, |row| row.saturating_add(1).min(last)))
    } else if stretched(Key::ArrowUp) {
        Some(current.map_or(0, |row| row.saturating_sub(1)))
    } else {
        None
    };
    if let Some(row) = stretch {
        let before = view.selected.map(|selected| (selected.row, selected.id));
        view.picked.click(history, row, before, Click::Range);
        view.select(history, row, None);
        return;
    }
    let target = if pressed(Key::ArrowDown) {
        Some(current.map_or(0, |row| row.saturating_add(1).min(last)))
    } else if pressed(Key::ArrowUp) {
        Some(current.map_or(0, |row| row.saturating_sub(1)))
    } else if pressed(Key::PageDown) {
        Some(current.map_or(0, |row| row.saturating_add(page).min(last)))
    } else if pressed(Key::PageUp) {
        Some(current.map_or(0, |row| row.saturating_sub(page)))
    } else if pressed(Key::Home) {
        Some(0)
    } else if pressed(Key::End) {
        Some(last)
    } else {
        None
    };
    if let Some(row) = target {
        let before = view.selected.map(|selected| (selected.row, selected.id));
        view.picked.click(history, row, before, Click::Plain);
        view.select(history, row, None);
    }
}

/// Draws the table's rows.
struct CommitRows<'a> {
    shared: &'a Shared<'a>,
    history: &'a History,
    summaries: &'a mut Summaries,
    signatures: &'a mut Signatures,
    objects: &'a Objects,
    view: &'a mut HistoryView,
    /// The row clicked this frame, and how.
    clicked: Option<(u64, Click)>,
    /// What a menu (or a double-clicked branch) asked for this frame.
    asked: Option<Ask>,
    /// What the menu of several commits picked asked for this frame.
    picked: Option<PickedEvent>,
    /// Now (seconds since the Unix epoch), for dates as how long ago.
    now: u64,
}

impl TableDelegate for CommitRows<'_> {
    fn prepare(&mut self, info: &PrefetchInfo) {
        let rows = info.visible_rows.clone();
        self.view.visible = rows.clone();
        self.view.top = self.history.id(rows.start).map(|id| (rows.start, id));
        self.view
            .graph
            .fill(self.history.graph(), rows.clone(), self.view.lanes_shown);
        // The rows on screen first, then a screen's worth below and above.
        if let Some(git) = self.shared.git {
            let screen = rows.end.saturating_sub(rows.start);
            let below = rows.end..rows.end.saturating_add(screen).min(self.history.rows());
            let above = rows.start.saturating_sub(screen)..rows.start;
            let history = self.history;
            let wanted = rows
                .clone()
                .chain(below)
                .chain(above.rev())
                .filter_map(|row| history.id(row));
            self.summaries
                .want(git, self.shared.repository, self.objects, wanted);
            // Only the rows on screen: each check runs a program.
            let summaries = &*self.summaries;
            let signed = rows
                .filter_map(|row| history.id(row))
                .filter(|id| summaries.peek(id).is_some_and(|summary| summary.signed));
            self.signatures.want(git, self.shared.repository, signed);
        }
    }

    fn header_cell_ui(&mut self, ui: &mut Ui, cell: &HeaderCellInfo) {
        if cell.row_nr == 1 {
            self.work_cell(ui, cell.col_range.start);
            return;
        }
        let title = match cell.col_range.start {
            0 => self.view.graph.title(self.view.lanes_shown),
            1 => "Message".to_owned(),
            2 => "Author".to_owned(),
            // Dates as written are the viewer's own time zone's, named.
            3 if self.shared.preferences.dates == crate::preferences::Dates::Written => {
                let now = i64::try_from(self.now).unwrap_or(i64::MAX);
                let zone = crate::format::zone_name(now, &crate::format::local_zone());
                format!("Date ({zone})")
            }
            3 => "Date".to_owned(),
            _ => "Commit".to_owned(),
        };
        crate::help::note_topic(ui, ui.max_rect(), column_topic(cell.col_range.start));
        // A hairline under the titles, across every column.
        let rect = ui.max_rect();
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, self.shared.palette.border),
        );
        ui.add_space(GRAPH_MARGIN);
        ui.add(
            Label::new(
                RichText::new(title)
                    .medium()
                    .size(crate::theme::SMALL)
                    .color(self.shared.palette.muted),
            )
            .selectable(false)
            .truncate(),
        );
    }

    fn row_ui(&mut self, ui: &mut Ui, row: u64) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("commit"), Sense::click());
        let id = self.history.id(row);
        // While several are picked, they are what shows as selected (a row taken away by a
        // ⌘-click is no longer, though the last clicked).
        let picked = id.is_some_and(|id| self.view.picked.contains(&id));
        let selected = if self.view.picked.is_several() {
            picked
        } else {
            self.view
                .selected
                .is_some_and(|selected| selected.row == row)
        };
        let found = self
            .view
            .search
            .found()
            .is_some_and(|found| found.contains(row));
        let hovered = response.hovered();
        let fill = if selected {
            Some(self.shared.palette.accent.gamma_multiply(0.28))
        } else if found {
            Some(self.shared.palette.found)
        } else if hovered {
            Some(self.shared.palette.surface_alt)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
        }
        // A selected or found row answers the pointer too.
        if hovered && (selected || found) {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, self.shared.palette.selected_hover);
        }
        if response.clicked() {
            // The keys held as the button was let go (they may be let go too, that frame).
            let modifiers = ui.input(|input| {
                input
                    .events
                    .iter()
                    .rev()
                    .find_map(|event| match event {
                        egui::Event::PointerButton {
                            pressed: false,
                            modifiers,
                            ..
                        } => Some(*modifiers),
                        _ => None,
                    })
                    .unwrap_or(input.modifiers)
            });
            self.clicked = Some((row, Click::of(modifiers)));
        }
        let Some(id) = id else {
            return;
        };
        // Looked up only when asked for: these run rarely, rows every frame.
        let summaries = &*self.summaries;
        response.widget_info(|| {
            let name = summaries.peek(&id).map_or_else(
                || id.abbreviated(ID_DIGITS).to_string(),
                |summary| summary.subject.to_string(),
            );
            WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, name)
        });
        let menus = self.shared.menus(self.history.refs());
        let asked = &mut self.asked;
        // One of several picked: what can be done to them all.
        if picked && self.view.picked.is_several() {
            let ids = &self.view.picked;
            let event = &mut self.picked;
            response.context_menu(|ui| {
                *event = picked::menu(ui, &menus, &ids.ids());
            });
            return;
        }
        response.context_menu(|ui| {
            actions::commit_menu(ui, &menus, id, asked);
            ui.separator();
            actions::menu_header(ui, "Copy");
            if actions::item(ui, (icon::COPY, "Copy commit name"), Ok(())) {
                ui.ctx().copy_text(id.to_string());
            }
            if let Some(summary) = summaries.peek(&id)
                && actions::item(ui, (icon::COPY, "Copy subject"), Ok(()))
            {
                ui.ctx().copy_text(summary.subject.to_string());
            }
            ui.separator();
            let subject = summaries
                .peek(&id)
                .map(|summary| summary.subject.to_string());
            actions::patch_items(ui, &menus, id, subject.as_deref(), asked);
        });
    }

    fn cell_ui(&mut self, ui: &mut Ui, cell: &CellInfo) {
        let Some(id) = self.history.id(cell.row_nr) else {
            return;
        };
        crate::help::note_topic(ui, ui.max_rect(), column_topic(cell.col_nr));
        if cell.col_nr == 0 {
            self.graph_cell(ui, cell.row_nr, id);
            return;
        }
        ui.add_space(GRAPH_MARGIN);
        if cell.col_nr == 1 {
            let menus = self.shared.menus(self.history.refs());
            let refs = self.history.refs().at(&id);
            // A branch hidden from the graph loses its badge too.
            let scope = Some(&self.shared.scope);
            if badges(
                ui,
                (refs, scope),
                &menus,
                self.shared.palette,
                &mut self.asked,
            ) {
                self.clicked = Some((cell.row_nr, Click::Plain));
            }
            pull_badges(
                ui,
                self.shared.forge.badges_at(&id),
                self.shared,
                &mut self.asked,
            );
        }
        if cell.col_nr == 4 {
            // Room for a pipeline's mark on every row while any commit has one.
            let forge = self.shared.forge;
            if forge.has_pipelines() {
                let mark = forge.pipeline_of(&id);
                match pipeline::mark(ui, mark, id, self.shared.palette) {
                    Some(MarkEvent::Show(mark)) => self.asked = Some(Ask::Pipeline(Box::new(mark))),
                    Some(MarkEvent::Open(url)) => self.asked = Some(Ask::Open(url)),
                    None => {}
                }
            }
            let name = id.abbreviated(ID_DIGITS).to_string();
            ui.add(
                Label::new(
                    RichText::new(name)
                        .monospace()
                        .color(self.shared.palette.muted),
                )
                .selectable(false),
            );
            return;
        }
        let Some(summary) = self.summaries.get(&id) else {
            return;
        };
        if cell.col_nr == 1 && summary.signed {
            seal(ui, self.signatures.get(&id), self.shared.palette);
        }
        if cell.col_nr == 2 {
            authors_cell(ui, summary, self.shared.palette);
            return;
        }
        let ago;
        let (text, color): (&str, Color32) = match cell.col_nr {
            1 => (&summary.subject, self.shared.palette.text),
            _ if self.shared.preferences.dates == crate::preferences::Dates::Ago => {
                ago = u64::try_from(summary.time)
                    .map_or_else(|_| String::new(), |time| crate::format::ago(time, self.now));
                (&ago, self.shared.palette.muted)
            }
            _ => (&summary.date, self.shared.palette.muted),
        };
        // Dates in columns of their own: each digit under the one above.
        let style = if cell.col_nr == 3 {
            egui::TextStyle::Monospace
        } else {
            egui::TextStyle::Body
        };
        fonts::note_text(ui, &style.resolve(ui.style()), text);
        ui.add(
            Label::new(RichText::new(text).text_style(style).color(color))
                .selectable(false)
                .truncate(),
        );
    }

    fn default_row_height(&self) -> f32 {
        ROW_HEIGHT
    }
}

/// The help topic of the commit table's column `at`.
pub(crate) const fn column_topic(at: usize) -> &'static str {
    match at {
        0 => "history",
        1 => "message-column",
        2 => "author-column",
        3 => "date-column",
        _ => "commit-column",
    }
}

/// A commit's author and co-authors: their avatars, overlapping, and their names.
fn authors_cell(ui: &mut Ui, summary: &crate::history::Summary, palette: Palette) {
    let mut people = Vec::with_capacity(summary.co_authors.len().saturating_add(1));
    people.push((&*summary.author, &*summary.email));
    people.extend(
        summary
            .co_authors
            .iter()
            .map(|(name, email)| (&**name, &**email)),
    );
    widgets::avatars(ui, AVATAR, &people, palette.background);
    let names = match &*summary.co_authors {
        [] => std::borrow::Cow::Borrowed(&*summary.author),
        [other] => format!("{} and {}", summary.author, other.0).into(),
        others => format!("{} and {} others", summary.author, others.len()).into(),
    };
    fonts::note_text(ui, &egui::TextStyle::Body.resolve(ui.style()), &names);
    ui.add(
        Label::new(RichText::new(&*names).color(palette.muted))
            .selectable(false)
            .truncate(),
    );
}

/// An avatar's size in the commit table.
const AVATAR: f32 = 18.0;

impl CommitRows<'_> {
    /// The work in progress's row, pinned under the titles: in the graph's column a hollow node
    /// on `HEAD`'s lane, a dashed line down towards it; in the rest, how many files are
    /// uncommitted (and how many of them staged). A click shows them, as a commit's row shows
    /// its details. Named as the top bar's button was ("3 uncommitted files").
    fn work_cell(&mut self, ui: &mut Ui, column: usize) {
        let palette = self.shared.palette;
        let rect = ui.max_rect();
        crate::help::note_topic(ui, rect, "work");
        let selected = self.view.work_selected;
        let hovered = ui.rect_contains_pointer(rect);
        crate::widgets::row_fill(ui, rect, 0.0, selected, hovered);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, palette.border),
        );
        if column == 0 {
            self.work_node(ui, rect);
            return;
        }
        let counts = self.view.work_counts.counts;
        let files = if counts.files == 1 {
            "1 uncommitted file".to_owned()
        } else {
            format!(
                "{} uncommitted files",
                count(u64::try_from(counts.files).unwrap_or(u64::MAX))
            )
        };
        let response = ui.interact(rect, ui.id().with("work-in-progress"), Sense::click());
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, &files));
        if response.clicked() {
            self.view.select_work();
        }
        let detail = match (counts.staged, counts.files) {
            (0, _) => "none staged yet".to_owned(),
            (staged, files) if staged == files => "all staged".to_owned(),
            (staged, _) => format!(
                "{} staged",
                count(u64::try_from(staged).unwrap_or(u64::MAX))
            ),
        };
        ui.add_space(GRAPH_MARGIN);
        ui.add(
            Label::new(
                RichText::new(format!("{}  {files}", icon::PENCIL_SIMPLE))
                    .medium()
                    .color(palette.warn),
            )
            .selectable(false),
        );
        ui.add(Label::new(muted(ui, format!("· {detail}"))).selectable(false));
        response.on_hover_text("Changes not yet committed: stage them, and commit");
    }

    /// The work in progress's node: hollow and dashed-edged on `HEAD`'s lane (the first, when
    /// `HEAD`'s row isn't on screen), a dashed line down towards `HEAD`.
    fn work_node(&self, ui: &Ui, rect: Rect) {
        let lane = self
            .shared
            .head_commit()
            .and_then(|head| {
                self.view
                    .visible
                    .clone()
                    .find(|row| self.history.id(*row) == Some(head))
                    .and_then(|row| self.view.graph.row(row))
                    .map(|(graph, _)| graph.lane)
            })
            .unwrap_or(0);
        let start = f32::from(self.view.graph.start);
        let x = (f32::from(lane) - start + 0.5).mul_add(LANE_WIDTH, rect.left() + GRAPH_MARGIN);
        let center = pos2(x, rect.center().y);
        let color = self.shared.palette.muted;
        let painter = ui.painter();
        painter.add(egui::Shape::dashed_line(
            &[pos2(x, center.y + NODE_RADIUS), pos2(x, rect.bottom())],
            Stroke::new(LINE_WIDTH, color),
            3.0,
            3.0,
        ));
        painter.circle(
            center,
            NODE_RADIUS,
            self.shared.palette.background,
            Stroke::new(LINE_WIDTH, color),
        );
    }

    /// Paints a row's lanes and node.
    fn graph_cell(&mut self, ui: &Ui, row: u64, id: ObjectId) {
        let rect = ui.max_rect();
        // Next frame works out as many lanes as the column shows.
        self.view.lanes_shown = whole_lanes(rect.width());
        let start = f32::from(self.view.graph.start);
        let Some((graph, segments)) = self.view.graph.row(row) else {
            return;
        };
        let colors = &self.shared.palette.lanes;
        let color_of = |index: u16| {
            colors
                .get(usize::from(index).checked_rem(colors.len()).unwrap_or(0))
                .copied()
                .unwrap_or(self.shared.palette.accent)
        };
        let x =
            |lane: u16| rect.left() + GRAPH_MARGIN + (f32::from(lane) - start + 0.5) * LANE_WIDTH;
        let y = |level: Level| match level {
            Level::Top => rect.top(),
            Level::Middle => rect.center().y,
            Level::Bottom => rect.bottom(),
        };
        let painter = ui.painter();
        for segment in segments {
            let stroke = Stroke::new(LINE_WIDTH, color_of(segment.color));
            let from = pos2(x(segment.from.lane), y(segment.from.level));
            let to = pos2(x(segment.to.lane), y(segment.to.level));
            if segment.from.lane == segment.to.lane {
                painter.line_segment([from, to], stroke);
            } else {
                painter.add(curve(from, to, stroke));
            }
        }
        let center = pos2(x(graph.lane), rect.center().y);
        let color = color_of(graph.color);
        let background = self.shared.palette.background;
        if graph.merge {
            painter.circle(
                center,
                NODE_RADIUS - 1.0,
                background,
                Stroke::new(LINE_WIDTH, color),
            );
        } else {
            painter.circle(center, NODE_RADIUS, color, Stroke::new(1.5, background));
        }
        if self.shared.head.commit == Some(id) {
            painter.circle_stroke(center, NODE_RADIUS + 3.0, Stroke::new(1.5, color));
        }
    }
}

/// A signed commit's seal, by what checking it found (still checking: a plain one), what it
/// means on hover.
fn seal(ui: &mut Ui, check: Option<&SignatureCheck>, palette: Palette) {
    let (glyph, color, meaning) = check.map_or(
        (icon::SEAL, palette.muted, "Signed: checking it…"),
        |check| verdict_look(check.verdict, palette),
    );
    ui.add(
        Label::new(RichText::new(glyph).color(color))
            .selectable(false)
            .sense(Sense::hover()),
    )
    .on_hover_text(meaning);
}

/// How a verdict looks: its seal, colour, and words.
const fn verdict_look(verdict: Verdict, palette: Palette) -> (&'static str, Color32, &'static str) {
    match verdict {
        Verdict::Good => (icon::SEAL_CHECK, palette.ok, "Good signature"),
        Verdict::Untrusted => (
            icon::SEAL_QUESTION,
            palette.warn,
            "Good signature, by a key not known to be its owner's",
        ),
        Verdict::ExpiredSignature => (
            icon::SEAL_QUESTION,
            palette.warn,
            "Good signature, but expired",
        ),
        Verdict::ExpiredKey => (
            icon::SEAL_QUESTION,
            palette.warn,
            "Good signature, by a key since expired",
        ),
        Verdict::RevokedKey => (
            icon::SEAL_WARNING,
            palette.danger,
            "Signed by a key since revoked",
        ),
        Verdict::Bad => (
            icon::SEAL_WARNING,
            palette.danger,
            "Bad signature: the commit doesn't match it",
        ),
        Verdict::Unchecked => (
            icon::SEAL,
            palette.muted,
            "Signed, but it couldn't be checked (its key or program is missing)",
        ),
        Verdict::Unsigned => (icon::SEAL, palette.muted, "Not signed"),
    }
}

/// How many whole lanes a graph column `width` wide shows (at least one).
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to u16's range first; float-to-int `as` saturates anyway"
)]
fn whole_lanes(width: f32) -> u16 {
    ((width - GRAPH_MARGIN * 2.0) / LANE_WIDTH)
        .floor()
        // `max` before `min`: NaN becomes 1.
        .max(1.0)
        .min(f32::from(u16::MAX)) as u16
}

/// A line between lanes: leaving `from` and reaching `to` vertically, curving between.
fn curve(from: Pos2, to: Pos2, stroke: Stroke) -> CubicBezierShape {
    let middle = f32::midpoint(from.y, to.y);
    CubicBezierShape::from_points_stroke(
        [from, pos2(from.x, middle), pos2(to.x, middle), to],
        false,
        Color32::TRANSPARENT,
        stroke,
    )
}

/// The refs at a commit, as badges: branches, then remote branches, then tags. Each has its
/// menu (on a right-click), and a branch switches to it on a double-click; returns whether a
/// badge was clicked (which selects its row).
fn badges(
    ui: &mut Ui,
    (refs, scope): (&[Reference], Option<&Scope>),
    menus: &Menus<'_>,
    palette: Palette,
    asked: &mut Option<Ask>,
) -> bool {
    let mut clicked = false;
    let shown = || {
        refs.iter()
            .filter(|reference| scope.is_none_or(|scope| !scope.hides(reference.name.as_bstr())))
    };
    let total = shown().count();
    let mut drawn = 0;
    for reference in shown().take(MAX_BADGES) {
        let checked_out = menus.branch.is_some()
            && reference.kind == RefKind::Branch
            && menus.branch == Some(reference.short_name());
        let bisected = bisect_badge(reference, menus.bisect, palette);
        let (glyph, color) = match reference.kind {
            _ if let Some((glyph, color, _)) = bisected => (glyph, color),
            RefKind::Branch => (icon::GIT_BRANCH, palette.accent),
            RefKind::RemoteBranch => (icon::CLOUD, palette.muted),
            RefKind::Tag => (icon::TAG, palette.warn),
            RefKind::Other => (icon::BOOKMARK_SIMPLE, palette.muted),
        };
        let (fill, text) = if checked_out {
            (color, palette.on_accent)
        } else {
            (color.gamma_multiply(0.16), palette.text)
        };
        let name = bisected.map_or_else(
            || reference.short_name().to_str_lossy().into_owned(),
            |(_, _, word)| word,
        );
        let label = format!("{glyph} {name}");
        let look = BadgeLook {
            fill,
            edge: color.gamma_multiply(0.7),
            text,
            strong: checked_out,
        };
        let galley = badge_galley(ui, &label, &look);
        if !fits(ui, &galley, total.saturating_sub(drawn)) {
            break;
        }
        let badge = paint_badge(ui, galley, &look, &label);
        drawn = drawn.saturating_add(1);
        dragging(ui, &badge, reference, &label, asked);
        if badge.double_clicked() {
            if let Some(switch) = actions::ref_double_clicked(menus, reference) {
                *asked = Some(switch);
            }
        } else if badge.clicked() {
            clicked = true;
        }
        badge.context_menu(|ui| actions::ref_menu(ui, menus, reference, asked));
    }
    more_badges(ui, total.saturating_sub(drawn));
    clicked
}

/// A badge's height: a commit row's less a gap above and below, so badges in rows next to each
/// other never touch.
const BADGE_HEIGHT: f32 = 18.0;

/// Between a badge's edge and its words.
const BADGE_PAD: f32 = 5.0;

/// Room kept after a badge for the count of those that didn't fit ("+3").
const MORE_ROOM: f32 = 28.0;

/// How a badge looks: its fill, edge, and words' colour, and whether its words lead.
struct BadgeLook {
    fill: Color32,
    edge: Color32,
    text: Color32,
    strong: bool,
}

/// A badge's words, laid out to be measured, then drawn.
fn badge_galley(ui: &Ui, label: &str, look: &BadgeLook) -> Arc<egui::Galley> {
    let family = if look.strong {
        crate::theme::Weight::SemiBold.family()
    } else {
        egui::FontFamily::Proportional
    };
    fonts::note_text(ui, &egui::FontId::new(12.0, family.clone()), label);
    ui.painter()
        .layout_no_wrap(label.to_owned(), egui::FontId::new(12.0, family), look.text)
}

/// Whether a badge of `galley` fits in what is left of its row, with room for the count of the
/// `left` (it among them) should more follow. In a layout that wraps (the details), every
/// badge fits: it moves to the next line.
fn fits(ui: &Ui, galley: &egui::Galley, left: usize) -> bool {
    if ui.layout().main_wrap {
        return true;
    }
    let more = if left > 1 { MORE_ROOM } else { 0.0 };
    galley.size().x + 2.0 * BADGE_PAD + more <= ui.available_width()
}

/// Draws a badge: its words in a rounded tag [`BADGE_HEIGHT`] tall, centred on its row; named
/// `label`, it clicks.
fn paint_badge(
    ui: &mut Ui,
    galley: Arc<egui::Galley>,
    look: &BadgeLook,
    label: &str,
) -> egui::Response {
    let size = egui::vec2(2.0f32.mul_add(BADGE_PAD, galley.size().x), BADGE_HEIGHT);
    // Clicked to select its commit, dragged onto another ref to merge or rebase.
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(
            rect,
            crate::theme::RADIUS_INNER,
            look.fill,
            Stroke::new(1.0, look.edge),
            egui::StrokeKind::Inside,
        );
        let at = egui::Align2::CENTER_CENTER
            .align_size_within_rect(galley.size(), rect)
            .min;
        painter.galley(at, galley, look.text);
    }
    response
}

/// A ref's badge (or sidebar row) dragged and dropped: it carries its ref while dragged (its
/// words follow the pointer), and another ref dropped on it is ringed while held over it, then
/// asks what to do.
pub(super) fn dragging(
    ui: &Ui,
    badge: &egui::Response,
    reference: &Reference,
    label: &str,
    asked: &mut Option<Ask>,
) {
    badge.dnd_set_drag_payload(DraggedRef(reference.clone()));
    let topic = match reference.kind {
        RefKind::Branch => "branch",
        RefKind::RemoteBranch => "remote-branch",
        RefKind::Tag => "tag",
        RefKind::Other => "history",
    };
    crate::help::note_topic(ui, badge.rect, topic);
    if badge.dragged()
        && let Some(pointer) = ui.ctx().pointer_interact_pos()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        egui::Area::new(Id::new("dragged-ref"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos2(pointer.x + 14.0, pointer.y + 10.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(label).medium());
                    ui.label(muted(ui, "Drop it on a branch to merge or rebase"));
                });
            });
    }
    let other = |dragged: &DraggedRef| dragged.0.name != reference.name;
    if badge
        .dnd_hover_payload::<DraggedRef>()
        .is_some_and(|dragged| other(&dragged))
    {
        let accent = Palette::of(ui.ctx()).accent;
        ui.painter().rect_stroke(
            badge.rect.expand(2.0),
            crate::theme::RADIUS_INNER,
            Stroke::new(2.0, accent),
            egui::StrokeKind::Outside,
        );
    }
    if let Some(dragged) = badge.dnd_release_payload::<DraggedRef>()
        && other(&dragged)
        && let Some(at) = ui.ctx().pointer_interact_pos()
    {
        *asked = Some(Ask::Dropped {
            dragged: dragged.0.clone(),
            onto: reference.clone(),
            at,
        });
    }
}

/// How many badges didn't fit, or were more than are shown: "+3".
fn more_badges(ui: &mut Ui, more: usize) {
    if more > 0 {
        ui.add(Label::new(muted(ui, format!("+{more}"))).selectable(false));
    }
}

/// A bisection's ref (`refs/bisect/bad`, `good-…`, `skip-…`) as its badge shows it: its icon,
/// colour, and word (the bisection's own terms); `None` for any other ref.
fn bisect_badge(
    reference: &Reference,
    bisection: Option<&Bisection>,
    palette: Palette,
) -> Option<(&'static str, Color32, String)> {
    let mark = reference.name.strip_prefix(b"refs/bisect/")?;
    let terms = bisection
        .map(|bisection| bisection.terms.clone())
        .unwrap_or_default();
    if mark == terms.bad.as_bytes() {
        Some((icon::X_CIRCLE, palette.danger, terms.bad))
    } else if mark.starts_with(format!("{}-", terms.good).as_bytes()) {
        Some((icon::CHECK_CIRCLE, palette.ok, terms.good))
    } else if mark.starts_with(b"skip-") {
        Some((icon::MINUS_CIRCLE, palette.muted, "skipped".to_owned()))
    } else {
        None
    }
}

/// The pull requests whose head is a commit, as badges (their number, coloured by their
/// checks): clicking one shows it, and its menu opens its page.
fn pull_badges(
    ui: &mut Ui,
    pulls: &[(ObjectId, Badge)],
    shared: &Shared<'_>,
    asked: &mut Option<Ask>,
) {
    let palette = shared.palette;
    let mut drawn = 0;
    for (_, badge) in pulls.iter().take(MAX_BADGES) {
        let (_, color, meaning) = pulls::check_look(badge.checks, palette);
        let kind = shared.forge.kind_of(badge.repository);
        let label = format!(
            "{} {}",
            icon::GIT_PULL_REQUEST,
            request_number(kind, badge.number)
        );
        let look = BadgeLook {
            fill: color.gamma_multiply(0.16),
            edge: color.gamma_multiply(0.7),
            text: palette.text,
            strong: false,
        };
        let galley = badge_galley(ui, &label, &look);
        if !fits(ui, &galley, pulls.len().saturating_sub(drawn)) {
            break;
        }
        let response = paint_badge(ui, galley, &look, &label);
        crate::help::note_topic(ui, response.rect, "pull-request-badge");
        drawn = drawn.saturating_add(1);
        let selected = (badge.repository, badge.number);
        let response = response.on_hover_ui(|ui| {
            if let Some(title) = shared.forge.title_of(selected) {
                ui.label(title);
            }
            ui.label(muted(ui, meaning));
        });
        if response.clicked() {
            *asked = Some(Ask::PullRequest(Some(selected)));
        }
        response.context_menu(|ui| {
            let show = format!("Show the {}", request_noun(kind));
            if actions::item(ui, (icon::GIT_PULL_REQUEST, &show), Ok(())) {
                *asked = Some(Ask::PullRequest(Some(selected)));
            }
            let open = format!("Open on {}", kind.name());
            if let Some(repository) = shared.forge.repositories.get(badge.repository)
                && actions::item(ui, (icon::ARROW_SQUARE_OUT, &open), Ok(()))
            {
                *asked = Some(Ask::Open(repository.pull_request_url(badge.number)));
            }
        });
    }
    more_badges(ui, pulls.len().saturating_sub(drawn));
}

/// The working tree's changes: what isn't staged and what is.
fn work_details(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    staging: &mut Bind<(), Problem>,
    commit: CommitSide<'_>,
    actions: &mut Vec<Action>,
) {
    let Some(status) = shared.work_status else {
        widgets::working(ui, "Reading the work tree…");
        return;
    };
    widgets::panel_head(ui, widgets::Head::Title, |ui| {
        widgets::panel_title(ui, "Working tree")
    });
    ui.add_space(4.0);
    let shown = egui::ScrollArea::vertical()
        .id_salt("work-details")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            view.work
                .show(ui, status, staging, commit, (shared.busy, shared.here()))
        })
        .inner;
    match shown {
        Some(WorkEvent::Open(work)) => view.open_work(shared, work),
        Some(WorkEvent::Stage(request)) => actions.push(Action::Stage(shared.id, request)),
        Some(WorkEvent::Commit(made)) => actions.push(Action::Commit(shared.id, made)),
        Some(WorkEvent::CancelCommit) => {
            actions.push(Action::Output(shared.id, OutputAsk::CancelCommit));
        }
        Some(WorkEvent::ShowOutput) => actions.push(Action::Output(shared.id, OutputAsk::Show)),
        Some(WorkEvent::Preferences(preferences)) => {
            actions.push(Action::SetPreferences(preferences));
        }
        Some(WorkEvent::AskDiscard(discard)) => {
            view.tasks.dialog = Some(super::dialogs::Dialog::Discard(discard));
        }
        Some(WorkEvent::Ignore(ignore)) => {
            actions.push(Action::Task(shared.id, crate::tasks::Task::Ignore(ignore)));
        }
        Some(WorkEvent::Reveal(path, how)) => {
            let work_tree = shared.repository.work_tree();
            actions.push(Action::Reveal(
                crate::desktop::in_work_tree(work_tree, path.as_ref()),
                how,
            ));
        }
        None => {}
    }
}

/// The details' place while several commits are picked: them, and what can be done to them
/// all; returns whether it was drawn.
fn picked_details(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
) -> bool {
    if !view.picked.is_several() || view.peeked().is_some() {
        return false;
    }
    let menus = shared.menus(history.shown.refs());
    // Carried out once the details are drawn, where actions are taken.
    view.picked_asked = picked::panel(ui, shared, &history.summaries, &view.picked, &menus);
    true
}

/// Two commits picked, compared, in the details' place: which they are (Swap, Close), and the
/// files changed from one to the other, each opening its diff; returns whether it was drawn.
fn compare_details(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
) -> bool {
    let Some(compare) = &view.compare else {
        return false;
    };
    // Only while those two are what is picked.
    let (from, to) = (compare.from, compare.to);
    let still = view.picked.len() == 2 && view.picked.contains(&from) && view.picked.contains(&to);
    if !still || view.peeked().is_some() {
        view.compare = None;
        return false;
    }
    let (mut swap, mut close) = (false, false);
    widgets::panel_head(ui, widgets::Head::Title, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            close = widgets::icon_tool(ui, true, icon::X, "Close the comparison").clicked();
            swap = widgets::icon_tool(ui, true, icon::ARROWS_LEFT_RIGHT, "Swap them")
                .on_hover_text("Compare the other way round")
                .clicked();
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                widgets::panel_title(ui, "Comparing two commits");
            });
        });
    });
    for (label, id) in [("From", from), ("To", to)] {
        widgets::fact(ui, DETAIL_LABEL, label, |ui| {
            ui.label(
                RichText::new(id.abbreviated(ID_DIGITS).to_string())
                    .monospace()
                    .color(shared.palette.muted),
            );
            let subject = history
                .summaries
                .peek(&id)
                .map_or_else(String::new, |summary| summary.subject.to_string());
            ui.add(Label::new(subject).truncate());
        });
    }
    ui.add(
        Label::new(muted(
            ui,
            "What changed from the first to the second, as git diff shows it between them.",
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    if close {
        view.compare = None;
        return false;
    }
    if swap {
        view.compare = Some(Comparison::new(shared, &history.objects, to, from));
        return true;
    }
    compared_files(ui, shared, view);
    true
}

/// A comparison's changed files, once read, each opening its diff from one commit to the
/// other.
fn compared_files(ui: &mut Ui, shared: &Shared<'_>, view: &mut HistoryView) {
    let Some(compare) = view.compare.as_mut() else {
        return;
    };
    let shown = match compare.details.read_as_ref() {
        None => {
            widgets::working(ui, "Comparing…");
            return;
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
            return;
        }
        Some(Ok(details)) => Arc::clone(details),
    };
    let counts = compare.counting.of(shared, &shown);
    files_heading(
        ui,
        shown.changes.len(),
        counts.as_ref(),
        shared.palette,
        &mut view.asked,
    );
    ui.add_space(4.0);
    let counts = counts.and_then(Result::ok);
    let picked = widgets::list_box(ui, |ui| {
        let height = ui.available_height();
        compare.changes.show(ui, &shown, counts.as_deref(), height)
    });
    if let Some(index) = picked
        && let Some(change) = shown.changes.get(index)
    {
        view.file = Some(FileView::open(
            shared,
            &shown,
            change.clone(),
            view.file.as_ref(),
        ));
    }
}

/// The selected commit's details and changed files.
fn details(ui: &mut Ui, shared: &Shared<'_>, history: &mut HistoryState, view: &mut HistoryView) {
    if compare_details(ui, shared, history, view) || picked_details(ui, shared, history, view) {
        return;
    }
    let peeked = view.peeked();
    let Some(id) = peeked.or_else(|| view.selected.map(|selected| selected.id)) else {
        widgets::panel_head(ui, widgets::Head::Text, |ui| {
            ui.label(muted(ui, "Select a commit to see what it changed."));
        });
        return;
    };
    let pending = view.details.is_pending();
    let Some(shown) = read_shown(ui, shared, history, view, id) else {
        return;
    };
    // An edit of another commit's message is left behind with it.
    if view
        .message_edit
        .as_ref()
        .is_some_and(|edit| edit.commit != shown.id)
    {
        view.message_edit = None;
    }
    let rewordable = rewordable(shared, &shown);
    if view.message_edit.is_some() {
        message_editor(ui, shared, view, &shown, rewordable);
    } else if message_head(ui, &shown, pending, rewordable) {
        view.message_edit = Some(MessageEdit::of(&shown));
    }
    let editing = view.message_edit.is_some();
    let mut go_to = None;
    let height = ui.available_height();
    // As much room as the line below was dragged to give it, else what it needs up to half
    // the panel; its bar shows while it holds more than that shows.
    let room = split_room(ui, height);
    let saved_scroll = ui.spacing().scroll;
    widgets::show_overflow(ui);
    let message = message_area(room, height).show(ui, |ui| {
        ui.spacing_mut().scroll = saved_scroll;
        if shown.commit.signed
            && let Some(git) = shared.git
        {
            history.signatures.want(git, shared.repository, [shown.id]);
        }
        let check = history.signatures.get(&shown.id);
        let (parent, description) = commit_info(
            ui,
            (&shown, !editing),
            history.shown.refs(),
            check,
            shared,
            &mut view.asked,
        );
        go_to = parent;
        // A double click on the description edits the message too.
        if let Some(rect) = description
            && rewordable == Some(Ok(()))
            && double_clicked(ui, rect)
        {
            view.message_edit = Some(MessageEdit::of(&shown));
        }
    });
    ui.spacing_mut().scroll = saved_scroll;
    // As tall as it is drawn: its room, or its content up to half (its rect is the most it may
    // take).
    let drawn = if room.is_some() {
        message.inner_rect.height()
    } else {
        message.content_size.y.min(message.inner_rect.height())
    };
    split_line(ui, height, drawn);
    let counts = view.counting.of(shared, &shown);
    files_heading(
        ui,
        shown.changes.len(),
        counts.as_ref(),
        shared.palette,
        &mut view.asked,
    );
    ui.add_space(4.0);
    let counts = counts.and_then(Result::ok);
    let picked = widgets::list_box(ui, |ui| {
        let height = ui.available_height();
        view.changes.show(ui, &shown, counts.as_deref(), height)
    });
    if let Some(index) = picked
        && let Some(change) = shown.changes.get(index)
    {
        match &mut view.tool {
            Some(Tool::Reflog(reflog)) if peeked.is_some() => {
                let file = FileView::open(shared, &shown, change.clone(), reflog.file());
                reflog.open_file(file);
            }
            _ => view.open_file(shared, &shown, change.clone()),
        }
    }

    let near = view.selected.map_or(0, |selected| selected.row);
    if let Some(parent) = go_to
        && let Some(row) = history.shown.row_of(&parent, near)
    {
        // A parent of the reflog's commit is shown in the history.
        if peeked.is_some() {
            view.tool = None;
        }
        view.select(&history.shown, row, Some(Align::Center));
    }
}

/// The details of the commit `id`, asked for when they aren't yet; until they are read (or
/// when they couldn't be), what is going on instead, and `None`.
fn read_shown(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut HistoryView,
    id: ObjectId,
) -> Option<Arc<CommitDetails>> {
    if view.details_for != Some(id)
        && let Some(git) = shared.git
    {
        view.details.request(read_details(
            git.clone(),
            Arc::clone(shared.repository),
            history.objects.clone(),
            id,
        ));
        view.details_for = Some(id);
    }
    match view.details.read_as_ref() {
        None => {
            widgets::panel_head(ui, widgets::Head::Text, |ui| {
                widgets::working(ui, "Reading the commit…");
            });
            None
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            widgets::panel_head(ui, widgets::Head::Text, |_| {});
            widgets::problem_box(ui, &problem, |_| {});
            None
        }
        Some(Ok(details)) => Some(Arc::clone(details)),
    }
}

/// The changed files' section title (named "3 files changed"), and the lines they gained and
/// lost once counted, at its right.
fn files_heading(
    ui: &mut Ui,
    files: usize,
    counts: Option<&Result<Arc<ChangeCounts>, Problem>>,
    palette: Palette,
    asked: &mut Option<Ask>,
) {
    let name = if files == 1 {
        "1 file changed".to_owned()
    } else {
        format!(
            "{} files changed",
            count(u64::try_from(files).unwrap_or(u64::MAX))
        )
    };
    widgets::section_header(ui, ("Changed files", Some(files), &name), |ui| {
        if let Some(preferences) = widgets::tree_toggle(ui) {
            *asked = Some(Ask::Preferences(preferences));
        }
        match counts {
            Some(Ok(counts)) => {
                // From the right: what was lost, then what was gained before it.
                let total = counts.total;
                ui.label(
                    RichText::new(format!("−{}", count(u64::from(total.removed))))
                        .color(palette.danger),
                );
                ui.label(
                    RichText::new(format!("+{}", count(u64::from(total.added)))).color(palette.ok),
                );
            }
            Some(Err(problem)) => {
                ui.label(muted(ui, "Lines not counted"))
                    .on_hover_text(problem.to_string());
            }
            None => {}
        }
    });
}

impl MessageEdit {
    /// An edit of `details`'s message, as it stands.
    fn of(details: &CommitDetails) -> Self {
        Self {
            commit: details.id,
            summary: details.commit.subject().to_str_lossy().into_owned(),
            description: details
                .commit
                .body()
                .trim_ascii()
                .to_str_lossy()
                .into_owned(),
            focus: true,
        }
    }

    /// The message it makes: the summary, then the description after a blank line.
    fn message(&self) -> String {
        let summary = self.summary.trim();
        let description = self.description.trim();
        if description.is_empty() {
            summary.to_owned()
        } else {
            format!("{summary}\n\n{description}")
        }
    }
}

/// Whether the shown commit's message can be reworded here: `None` when it isn't on the
/// branch checked out (nothing offered), else allowed or why not.
fn rewordable(shared: &Shared<'_>, details: &CommitDetails) -> Option<Result<(), &'static str>> {
    if !details.in_head {
        return None;
    }
    Some(if shared.branch().is_none() {
        Err("Switch to a branch first: rewording rewrites one")
    } else if shared.busy {
        Err("Wait for the running task")
    } else if shared.operation.is_some() {
        Err("Finish or abort what is in progress first")
    } else {
        Ok(())
    })
}

/// Whether the pointer double-clicked in `rect` this frame; under the pointer, `rect` shows it
/// can be: a faint fill and a text cursor.
fn double_clicked(ui: &Ui, rect: Rect) -> bool {
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        ui.painter().rect_filled(
            rect,
            crate::theme::RADIUS_BOX,
            Palette::of(ui.ctx()).selected_hover,
        );
    }
    hovered
        && ui.input(|input| {
            input
                .pointer
                .button_double_clicked(egui::PointerButton::Primary)
        })
}

/// The panel's title, the commit's subject (a double click edits it, when it can be), with a
/// button editing the message at its right, or a spinner while another commit is read.
/// Returns whether editing was asked for.
fn message_head(
    ui: &mut Ui,
    details: &CommitDetails,
    pending: bool,
    rewordable: Option<Result<(), &'static str>>,
) -> bool {
    let mut edit = false;
    let title = widgets::panel_head(ui, widgets::Head::Title, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if pending {
                ui.spinner().on_hover_text("Reading the commit…");
            } else if let Some(allowed) = rewordable {
                let button = widgets::icon_tool(
                    ui,
                    allowed.is_ok(),
                    icon::PENCIL_SIMPLE,
                    "Edit the message",
                );
                let button = match allowed {
                    Ok(()) => button.on_hover_text("Edit the message (or double-click it)"),
                    Err(why) => button.on_disabled_hover_text(why),
                };
                edit |= button.clicked();
            }
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                widgets::panel_title(ui, details.commit.subject().to_str_lossy())
                    .rect
                    .expand2(egui::vec2(6.0, 4.0))
            })
            .inner
        })
        .inner
    });
    if rewordable == Some(Ok(())) && double_clicked(ui, title) {
        edit = true;
    }
    edit
}

/// The message being edited: its summary and description in fields, what rewording does, and
/// Save message (⌘Enter) or Cancel (Escape).
fn message_editor(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut HistoryView,
    details: &CommitDetails,
    rewordable: Option<Result<(), &'static str>>,
) {
    let Some(edit) = view.message_edit.as_mut() else {
        return;
    };
    widgets::panel_head(ui, widgets::Head::Title, |ui| {
        widgets::panel_title(ui, "Edit the message")
    });
    let label = ui.label(muted(ui, "Summary"));
    let summary = ui
        .add(
            widgets::line_field(&mut edit.summary)
                .hint_text("What it does, in a line")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    if std::mem::take(&mut edit.focus) {
        summary.request_focus();
    }
    ui.add_space(4.0);
    let label = ui.label(muted(ui, "Description"));
    // Bounded: a long message scrolls inside its field, leaving room for the buttons below.
    let most = (ui.available_height() * 0.45).max(120.0);
    let description = widgets::bounded_text_area(
        ui,
        widgets::text_area(&mut edit.description).desired_rows(5),
        most,
        "message-edit-description",
    )
    .labelled_by(label.id);
    let branch = shared.branch().map_or_else(
        || "the branch".to_owned(),
        |name| name.to_str_lossy().into_owned(),
    );
    let what = if shared.head_commit() == Some(details.id) {
        "Only its message changes; what is staged stays staged.".to_owned()
    } else {
        format!("Rewrites {branch} from this commit: it and the commits after it get new names.")
    };
    ui.add_space(4.0);
    ui.add(Label::new(muted(ui, what).small()).wrap());
    ui.add_space(8.0);
    let focused = [&summary, &description]
        .iter()
        .any(|field| field.has_focus() || field.lost_focus());
    let ready = rewordable == Some(Ok(())) && !edit.summary.trim().is_empty();
    let (mut save, mut cancel) = (false, false);
    // A row of its own, its buttons at its right.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), crate::theme::CONTROL),
        Layout::right_to_left(Align::Center),
        |ui| {
            save = widgets::primary(ui, ready, "Save message").clicked();
            cancel = widgets::secondary(ui, true, "Cancel").clicked();
        },
    );
    save |=
        ready && focused && ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Enter));
    cancel |= focused && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    if save {
        let message = edit.message();
        view.asked = Some(Ask::Task(crate::tasks::Task::Reword {
            commit: details.id,
            message,
        }));
        view.message_edit = None;
        view.keep_row = true;
    } else if cancel {
        view.message_edit = None;
    }
    ui.add_space(8.0);
}

/// The commit's description (unless `described` is false: it is being edited), names, people,
/// and refs (`all` holds them; their menus' answer goes to `asked`); returns a parent the user
/// chose to go to, and where the description is.
fn commit_info(
    ui: &mut Ui,
    (details, described): (&CommitDetails, bool),
    all: &Refs,
    check: Option<&SignatureCheck>,
    shared: &Shared<'_>,
    asked: &mut Option<Ask>,
) -> (Option<ObjectId>, Option<Rect>) {
    let commit = &details.commit;
    let refs = all.at(&details.id);
    let mut go_to = None;
    // The description apart from the subject (the panel's title): a box of its own.
    let body = commit.body().trim_ascii();
    let mut description = None;
    if described && !body.is_empty() {
        let shown = widgets::text_box(ui, |ui| ui.add(Label::new(body.to_str_lossy()).wrap()));
        description = Some(shown.rect.expand(12.0));
        ui.add_space(8.0);
    }
    ui.spacing_mut().item_spacing.y = 4.0;
    widgets::fact_of(ui, DETAIL_LABEL, ("Commit", "commit-name"), |ui| {
        // Within the room the copy button leaves: the details never push the panel wider.
        let room =
            (ui.available_width() - crate::theme::CONTROL - ui.spacing().item_spacing.x).max(0.0);
        ui.allocate_ui(egui::vec2(room, crate::theme::CONTROL), |ui| {
            let name = details.id.abbreviated(ID_DIGITS * 2).to_string();
            ui.add(Label::new(RichText::new(name).monospace()).truncate());
        });
        if widgets::icon_tool(ui, true, icon::COPY, "Copy the commit's name")
            .on_hover_text("Copy its whole name")
            .clicked()
        {
            ui.ctx().copy_text(details.id.to_string());
        }
    });
    let parents = if commit.parents.len() == 1 {
        "Parent"
    } else {
        "Parents"
    };
    widgets::fact(ui, DETAIL_LABEL, parents, |ui| {
        if commit.parents.is_empty() {
            ui.label(muted(ui, "None: the first commit"));
        }
        // Buttons rather than links: egui announces a link as plain text.
        for parent in &commit.parents {
            let name = parent.abbreviated(ID_DIGITS).to_string();
            let link = RichText::new(name)
                .monospace()
                .color(ui.visuals().hyperlink_color);
            if widgets::link(ui, link)
                .on_hover_text("Go to this parent")
                .clicked()
            {
                go_to = Some(*parent);
            }
        }
    });
    person(ui, ("Author", "Date"), &commit.author);
    co_authors(ui, commit);
    let committed_apart = commit.committer.name != commit.author.name
        || commit.committer.email != commit.author.email
        || commit.committer.time != commit.author.time;
    if committed_apart {
        person(ui, ("Committer", "Committed"), &commit.committer);
    }
    if commit.signed {
        widgets::fact_top(ui, DETAIL_LABEL, "Signature", |ui| {
            crate::help::note_topic(ui, ui.max_rect(), "signing");
            signature(ui, check, shared.palette);
        });
    }
    if !refs.is_empty() {
        let menus = shared.menus(all);
        widgets::fact_top(ui, DETAIL_LABEL, "Refs", |ui| {
            ui.horizontal_wrapped(|ui| badges(ui, (refs, None), &menus, shared.palette, asked));
        });
    }
    (go_to, description)
}

/// What checking a commit's signature found: the verdict, who signed it, and with which key.
fn signature(ui: &mut Ui, check: Option<&SignatureCheck>, palette: Palette) {
    ui.vertical(|ui| {
        let Some(check) = check else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(muted(ui, "Checking the signature…"));
            });
            return;
        };
        let (glyph, color, meaning) = verdict_look(check.verdict, palette);
        ui.add(Label::new(RichText::new(format!("{glyph}  {meaning}")).color(color)).wrap());
        if !check.signer.is_empty() {
            ui.add(Label::new(format!("by {}", check.signer.to_str_lossy())).wrap());
        }
        if !check.key.is_empty() {
            ui.add(
                Label::new(
                    RichText::new(format!("key {}", check.key.to_str_lossy()))
                        .monospace()
                        .small()
                        .color(palette.muted),
                )
                .wrap(),
            );
        }
    });
}

/// An author or committer: their avatar, then the name, leading, and the email beneath,
/// muted; then when, as a fact of its own (`when` names it).
fn person(ui: &mut Ui, (role, when): (&str, &str), signature: &egit_git::Signature) {
    widgets::fact_top(ui, DETAIL_LABEL, role, |ui| {
        someone(ui, (&signature.name, &signature.email));
    });
    // In the viewer's time zone, named, as the table shows it; its own clock on hover.
    let zone = crate::format::local_zone();
    let local = crate::format::local_date_time(signature.time, &zone);
    let name = crate::format::zone_name(signature.time, &zone);
    let own = format!(
        "{} {}, by its own clock",
        date_time(signature.time, signature.offset_minutes),
        crate::format::offset(signature.offset_minutes)
    );
    widgets::fact(ui, DETAIL_LABEL, when, |ui| {
        ui.add(Label::new(format!("{local} {name}")).truncate())
            .on_hover_text(own);
    });
}

/// Who a message's `Co-authored-by` trailers name, each as an author is shown.
fn co_authors(ui: &mut Ui, commit: &egit_git::Commit) {
    let people = commit.co_authors();
    if people.is_empty() {
        return;
    }
    let label = if people.len() == 1 {
        "Co-author"
    } else {
        "Co-authors"
    };
    let edge = Palette::of(ui.ctx()).panel;
    widgets::fact_top(ui, DETAIL_LABEL, label, |ui| {
        crate::help::note_topic(ui, ui.max_rect(), "co-authors");
        ui.spacing_mut().item_spacing.y = 4.0;
        // A line each, the email on hover: there may be many.
        for person in &people {
            let (name, email) = (person.name.to_str_lossy(), person.email.to_str_lossy());
            // As tall as the label's line, so the first sits level with it.
            let height = ui
                .text_style_height(&egui::TextStyle::Body)
                .max(CO_AUTHOR_AVATAR);
            let line = egui::vec2(ui.available_width(), height);
            ui.allocate_ui_with_layout(line, Layout::left_to_right(Align::Center), |ui| {
                widgets::avatar(ui, CO_AUTHOR_AVATAR, (&name, &email), edge);
                ui.add(Label::new(RichText::new(&*name).medium()).truncate())
                    .on_hover_text(&*email);
            });
        }
    });
}

/// A co-author's avatar in the details.
const CO_AUTHOR_AVATAR: f32 = 18.0;

/// Someone named in a commit: their avatar, their name leading, their email beneath.
fn someone(ui: &mut Ui, (name, email): (&[u8], &[u8])) {
    let (name, email) = (name.to_str_lossy(), email.to_str_lossy());
    let edge = Palette::of(ui.ctx()).panel;
    ui.horizontal_top(|ui| {
        widgets::avatar(ui, DETAIL_AVATAR, (&name, &email), edge);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(Label::new(RichText::new(&*name).medium()).wrap());
            ui.add(Label::new(muted(ui, &*email).small()).wrap());
        });
    });
}

/// An avatar's size in the details.
const DETAIL_AVATAR: f32 = 28.0;

/// The width of the details' labels (Commit, Author, …): the panel is narrow.
const DETAIL_LABEL: f32 = 72.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_shows_its_whole_lanes() {
        let width = |lanes: f32| GRAPH_MARGIN * 2.0 + lanes * LANE_WIDTH;
        assert_eq!(whole_lanes(0.0), 1);
        assert_eq!(whole_lanes(width(1.0)), 1);
        assert_eq!(whole_lanes(width(3.9)), 3);
        assert_eq!(whole_lanes(width(10.0)), 10);
        assert_eq!(whole_lanes(f32::INFINITY), u16::MAX);
        assert_eq!(whole_lanes(f32::NAN), 1);
    }

    /// Chunks of a history where every commit on rows 3 and 4 sits beyond lane 5: tips 0..=5
    /// all lead down to one root.
    fn wide() -> Vec<Chunk> {
        let mut builder = egit_graph::GraphBuilder::new();
        let mut chunks = Vec::new();
        for tip in 0..6_u32 {
            chunks.extend(builder.push(tip, &[tip.saturating_add(100)]));
        }
        for (commit, parent) in [(100, 105), (101, 105), (102, 105), (103, 105), (104, 105)] {
            chunks.extend(builder.push(commit, &[parent]));
        }
        chunks.extend(builder.push(105, &[]));
        chunks.extend(builder.finish());
        chunks
    }

    #[test]
    fn a_narrow_column_shows_the_nodes_on_screen() {
        let chunks = wide();
        let mut graph = GraphRows::default();
        graph.fill(&chunks, 0..6, 10);
        assert_eq!(graph.start, 0, "everything fits");
        assert_eq!(graph.title(10), "Graph");

        graph.fill(&chunks, 0..6, 3);
        assert_eq!(
            graph.start, 0,
            "from the leftmost node, which is in the first lane"
        );
        assert_eq!(graph.title(3), "Graph ›");

        graph.fill(&chunks, 4..6, 3);
        assert_eq!(graph.start, 3, "nodes in lanes 4 and 5, shown from 3");
        assert_eq!(graph.title(3), "‹ Graph");
        let (row, segments) = graph.row(5).unwrap();
        assert_eq!(row.lane, 5);
        assert!(
            segments
                .iter()
                .all(|segment| segment.from.lane.max(segment.to.lane) >= 3),
            "nothing left of the window is drawn"
        );
    }
}
