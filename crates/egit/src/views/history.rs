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

use bstr::{BStr, ByteSlice as _};
use egit_git::{
    Bisection, CancelToken, Changes, Committed, ConfigSnapshot, FileChange, Git, Head,
    MailboxProgress, ObjectId, Operation, RefKind, Reference, Repository, SignatureCheck, Stash,
    Status, Verdict, looks_like_patch,
};
use egit_graph::{Chunk, Level, Segment, for_each_row};
use egui::epaint::CubicBezierShape;
use egui::{
    Align, Color32, CornerRadius, Frame, Id, Key, KeyboardShortcut, Label, Layout, Margin,
    Modifiers, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, WidgetInfo, WidgetType, pos2,
};
use egui_async::Bind;
use egui_table_kit::layout::{
    CellInfo, Column, HeaderCellInfo, HeaderRow, PrefetchInfo, Table, TableDelegate, TableState,
};

use super::actions::{self, Ask, Menus};
use super::blame::{self, BlameEvent, BlameView};
use super::file::{self, FileView, FileWanted, WorkFile};
use super::file_history::{self, FileHistoryView};
use super::patch::{self, PatchEvent, PatchSource, PatchView};
use super::pull_form::{self, PullRequestForm};
use super::pulls::{self, PullRequestEvent, PullRequestView};
use super::rebase::{self, RebaseEvent, RebaseView};
use super::reflog::{self, ReflogEvent, ReflogView};
use super::search::{self, HistorySearch};
use super::sidebar::{self, Lists, Sidebar, SidebarEvent};
use super::tasks::{self, TaskEvent, TaskPane, TasksView};
use super::work::{CommitSide, WorkEvent, WorkPanel};
use crate::app::Action;
use crate::commands::RepoCommand;
use crate::desktop::FileQuestion;
use crate::fonts;
use crate::forge::{Badge, RepoForge, request_noun, request_number};
use crate::format::{count, date_time};
use crate::history::{
    ChangeCounts, CommitDetails, History, HistoryState, Objects, Refs, Scope, Signatures,
    Summaries, read_change_counts, read_details,
};
use crate::preferences::Preferences;
use crate::problem::Problem;
use crate::repo::{RepoId, RepoPanel};
use crate::tables::ChangesTable;
use crate::tasks::Tasks;
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
    /// The last staging or unstaging.
    pub(crate) staging: &'a mut Bind<(), Problem>,
    /// The last commit made, or being made.
    pub(crate) committing: &'a mut Bind<Committed, Problem>,
    /// How many commits egit has made here.
    pub(crate) commits_made: u64,
    /// Whether commits are signed unless asked otherwise (`commit.gpgSign`).
    pub(crate) sign_by_default: bool,
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
    /// The stashes, worktrees, submodules, and configuration, for the sidebar.
    pub(crate) lists: Lists<'a>,
    pub(crate) actions: &'a mut Vec<Action>,
}

/// What the history pane remembers between frames.
#[derive(Debug)]
pub(crate) struct HistoryView {
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
    /// What the top bar measured last frame.
    bar: BarRoom,
    /// The selected commit's message being edited in the details, to reword it.
    message_edit: Option<MessageEdit>,
    /// A rewrite of the selected commit was asked for: when it is gone from the next history
    /// read, the selection stays on its row (where its rewritten self is).
    keep_row: bool,
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
    Patch(PatchView),
    Reflog(Box<ReflogView>),
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
            bar: BarRoom::default(),
            message_edit: None,
            keep_row: false,
        }
    }

    /// Asks it to carry `command` out (next frame, with everything its own buttons have).
    pub(crate) fn queue(&mut self, command: RepoCommand) {
        self.queued.push(command);
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

    /// How many lines the new side of the open file's split diff has, once split.
    #[cfg(test)]
    pub(crate) fn split_lines(&mut self) -> Option<u32> {
        self.file.as_mut()?.split_lines()
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
        let belongs = self.file.as_ref().is_none_or(|file| match file.commit() {
            Some(commit) => {
                !self.work_selected
                    && (self.selected.map(|selected| selected.id) == Some(commit)
                        || self.peek == Some(commit))
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
        staging,
        committing,
        commits_made,
        sign_by_default,
        operations,
        applying,
        bisect,
        config,
        tasks,
        stashes,
        forge,
        home,
        journal,
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
        work_status: work_status.as_ref(),
        operation: operations.first().copied(),
        bisect,
        preferences: Preferences::of(ui.ctx()),
        config,
        busy: tasks.is_busy(),
        committing: committing.is_pending(),
        forge,
        scope: history.scope().clone(),
        home,
        journal,
    };
    // Before the first commit, the work tree is all there is to look at.
    if history.shown.rows() == 0 && !history.shown.is_loading() {
        view.work_selected = true;
    }
    dropped_patches(ui, id, view);

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
        egui::CentralPanel::default()
            .frame(Frame::new().inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                if !history.shown.is_loading() && history.shown.problem().is_none() {
                    ui.label(muted(ui, "No commits yet."));
                }
            });
        return;
    }

    let work = WorkSide {
        staging,
        committing,
        commits_made,
        sign_by_default,
    };
    let sides = Sides {
        work,
        lists: &lists,
    };
    panels(ui, &shared, history, view, sides, actions, no_commits);
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

/// The panel over the commit table: the top bar, the last task's outcome, and an operation's
/// banner.
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
            tasks::outcome(ui, &mut view.tasks, pane);
            match tasks::banner(ui, &mut view.tasks, pane) {
                Some(TaskEvent::WorkTree) => view.select_work(),
                Some(TaskEvent::Patch) => view.show_patch(PatchSource::Stopped),
                Some(TaskEvent::Commit(commit)) => {
                    view.file = None;
                    view.tool = None;
                    view.show_commit(&history.shown, commit);
                }
                None => {}
            }
        });
}

/// Patch files dropped on the window, shown to apply (others are left alone); a hint while
/// files are dragged over it.
fn dropped_patches(ui: &Ui, id: RepoId, view: &mut HistoryView) {
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
                        ui.label(
                            RichText::new(format!(
                                "{}  Drop patches to look at them, then apply them",
                                icon::GIT_DIFF
                            ))
                            .semibold(),
                        );
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
struct WorkSide<'a> {
    staging: &'a mut Bind<(), Problem>,
    committing: &'a mut Bind<Committed, Problem>,
    commits_made: u64,
    sign_by_default: bool,
}

/// The details beside the history (a commit's, or the working tree's), and the commit table
/// (or a file looked at) in the middle.
fn panels(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &mut HistoryState,
    view: &mut HistoryView,
    sides: Sides<'_>,
    actions: &mut Vec<Action>,
    no_commits: bool,
) {
    let Sides { work, lists } = sides;
    let WorkSide {
        staging,
        committing,
        commits_made,
        sign_by_default,
    } = work;
    side_panel(ui, shared, history, view, lists, actions);
    egui::Panel::right(Id::new(("history-details", shared.id)))
        .resizable(true)
        .default_size(380.0)
        .size_range(260.0..=900.0)
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
                    head: shared.head.commit,
                    git: shared.git,
                    repository: shared.repository,
                    objects: &history.objects,
                };
                work_details(ui, shared, view, staging, commit, actions);
            } else {
                details(ui, shared, history, view);
            }
        });
    if let Some(asked) = view.asked.take() {
        carry_out(asked, shared, view, actions);
    }

    view.close_unselected_file();
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| match &mut view.file {
            _ if view.tool.is_some() => tool(ui, shared, history, view, actions),
            Some(file) => {
                let open = file::show(ui, shared, &history.objects, file, actions);
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
            .exact_size(28.0)
            .frame(chrome(shared).inner_margin(Margin::symmetric(4, 0)))
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
        Some(SidebarEvent::Open(path)) => actions.push(Action::Open(path)),
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
}

impl Shared<'_> {
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
        ((icon::ARCHIVE, stash.as_str()), true),
        ((icon::GIT_DIFF, "Patches"), true),
        ((icon::INFO, "Repository"), true),
    ];
    let words: f32 = tools
        .iter()
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
    repository_menu(ui, shared, look, task_pane.actions);
    // What comes and goes follows the buttons: ids are counted in order, and a click
    // pressed on one id and released on another is lost.
    let rows = history.shown.rows();
    let commits = if rows == 1 {
        "1 commit".to_owned()
    } else {
        format!("{} commits", count(rows))
    };
    widgets::toolbar_separator(ui);
    ui.label(muted(ui, commits));
    let spacing = ui.spacing().item_spacing.x;
    let mut kept = 0.0;
    if history.is_loading() {
        ui.add(egui::Spinner::new().size(SPINNER))
            .on_hover_text("Reading the history…");
    } else {
        kept += SPINNER + spacing;
    }
    if !work_tree_summary(ui, work_tree, view, shared.palette) {
        let button = (icon::PENCIL_SIMPLE, "1 uncommitted file");
        kept += widgets::tool_width(ui, button, Look::Full, false) + spacing;
    }
    pull_requests_button(ui, shared, view);
    kept
}

/// The top bar's spinner's size, while the history is read.
const SPINNER: f32 = 16.0;

/// The repository's menu: its overview and settings (each a page in place of the history), and
/// reading it all again.
fn repository_menu(ui: &mut Ui, shared: &Shared<'_>, look: Look, actions: &mut Vec<Action>) {
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
        ui.separator();
        if actions::shortcut_item(ui, (icon::ARROWS_CLOCKWISE, "Read it again"), "⌘R") {
            actions.push(Action::Refresh(shared.id));
        }
    });
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
/// open.
fn pull_requests_button(ui: &mut Ui, shared: &Shared<'_>, view: &mut HistoryView) {
    if shared.forge.repositories.is_empty() {
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

/// How many files have changes not yet committed, if any (a button showing them beside the
/// history); or why that isn't known. Returns whether it drew the button.
fn work_tree_summary(
    ui: &mut Ui,
    work_tree: &mut WorkTree,
    view: &mut HistoryView,
    palette: Palette,
) -> bool {
    match work_tree.status() {
        Some(Ok(status)) if !status.is_empty() => {
            let files = status.len();
            let text = if files == 1 {
                "1 uncommitted file".to_owned()
            } else {
                format!(
                    "{} uncommitted files",
                    count(u64::try_from(files).unwrap_or(u64::MAX))
                )
            };
            let label =
                RichText::new(format!("{}  {text}", icon::PENCIL_SIMPLE)).color(palette.warn);
            let button = crate::widgets::quiet(ui, |ui| {
                ui.add(
                    egui::Button::selectable(view.work_selected, label)
                        .min_size(egui::vec2(0.0, crate::theme::CONTROL)),
                )
            });
            // Named in words.
            button.widget_info(|| {
                WidgetInfo::selected(WidgetType::Button, true, view.work_selected, &text)
            });
            if button
                .on_hover_text("Changes not yet committed: stage them, and commit")
                .clicked()
            {
                view.select_work();
            }
            true
        }
        Some(Err(problem)) => {
            let problem = problem.to_string();
            ui.label(
                RichText::new(format!(
                    "{}  The work tree couldn't be read",
                    icon::WARNING_CIRCLE
                ))
                .color(palette.danger),
            )
            .on_hover_text(problem);
            false
        }
        _ => false,
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
    }

    let salt = ("history", shared.id);
    let columns = columns(ui, salt, history.shown.widest());
    let mut table = Table::new()
        .id_salt(salt)
        .num_rows(history.shown.rows())
        .columns(columns)
        .headers([HeaderRow::new(HEADER_HEIGHT)]);
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
        now: crate::format::now(),
    };
    table.show(ui, &mut rows);
    let asked = rows.asked.take();
    if let Some(row) = rows.clicked {
        view.select(shown, row, None);
        ui.memory_mut(|memory| memory.request_focus(keys));
    }
    if let Some(asked) = asked {
        carry_out(asked, shared, view, actions);
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
        Some(Tool::PullRequests(pulls)) => match pulls::show(ui, shared, history, pulls) {
            Some(PullRequestEvent::Close) => Some(BlameEvent::Close),
            Some(PullRequestEvent::ShowCommit(id)) => Some(BlameEvent::ShowCommit(id)),
            Some(PullRequestEvent::Task(task)) => {
                actions.push(Action::Task(shared.id, task));
                None
            }
            Some(PullRequestEvent::Open(url)) => {
                actions.push(Action::OpenUrl(url));
                None
            }
            Some(PullRequestEvent::SignIn(host)) => {
                actions.push(Action::SignIn(shared.id, host));
                None
            }
            Some(PullRequestEvent::Refresh) => {
                actions.push(Action::RefreshPullRequests(shared.id));
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
    if let Some(asked) = menu {
        carry_out(asked, shared, view, actions);
    }
    if let Some(wanted) = wanted_file {
        view.open_tool(shared, &history.objects, wanted);
    }
    match asked {
        Some(BlameEvent::Close) => view.tool = None,
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

/// The table's columns: the graph, sized for the history's lanes; the message, taking what
/// the others leave; author, date, and name.
fn columns(ui: &Ui, salt: (&str, RepoId), widest: usize) -> Vec<Column> {
    let lanes = widest.clamp(1, FIRST_LANES);
    let graph_width = f32::from(u16::try_from(lanes).unwrap_or(1))
        .mul_add(LANE_WIDTH, GRAPH_MARGIN * 2.0)
        .max(FIRST_GRAPH_WIDTH);
    let mut columns = vec![
        Column::new(graph_width)
            .range(GRAPH_MARGIN * 2.0 + LANE_WIDTH..=1_200.0)
            .id(Id::new("graph")),
        Column::new(MIN_MESSAGE_WIDTH)
            .range(MIN_MESSAGE_WIDTH..=f32::INFINITY)
            .resizable(false)
            .id(Id::new("message")),
        Column::new(150.0).range(60.0..=480.0).id(Id::new("author")),
        Column::new(132.0).range(90.0..=240.0).id(Id::new("date")),
        Column::new(84.0).range(60.0..=240.0).id(Id::new("commit")),
    ];
    // The message takes what the others (at their remembered widths) leave.
    let remembered = TableState::load(ui.ctx(), TableState::id(ui, egui::IdSalt::new(salt)))
        .map(|state| state.col_widths)
        .unwrap_or_default();
    let others: f32 = columns
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 1)
        .map(|(index, column)| {
            remembered
                .get(&column.id_for(index))
                .copied()
                .unwrap_or(column.current)
        })
        .sum();
    let room = ui.available_width() - others - ui.spacing().scroll.allocated_width();
    if let Some(message) = columns.get_mut(1) {
        message.current = room.max(MIN_MESSAGE_WIDTH);
    }
    columns
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
    /// The row clicked this frame.
    clicked: Option<u64>,
    /// What a menu (or a double-clicked branch) asked for this frame.
    asked: Option<Ask>,
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
        let title = match cell.col_range.start {
            0 => self.view.graph.title(self.view.lanes_shown),
            1 => "Message".to_owned(),
            2 => "Author".to_owned(),
            3 => "Date".to_owned(),
            _ => "Commit".to_owned(),
        };
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
        let selected = self
            .view
            .selected
            .is_some_and(|selected| selected.row == row);
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
            self.clicked = Some(row);
        }
        let Some(id) = self.history.id(row) else {
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
                self.clicked = Some(cell.row_nr);
            }
            pull_badges(
                ui,
                self.shared.forge.badges_at(&id),
                self.shared,
                &mut self.asked,
            );
        }
        if cell.col_nr == 4 {
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
        let ago;
        let (text, color): (&str, Color32) = match cell.col_nr {
            1 => (&summary.subject, self.shared.palette.text),
            2 => (&summary.author, self.shared.palette.muted),
            _ if self.shared.preferences.dates == crate::preferences::Dates::Ago => {
                ago = u64::try_from(summary.time)
                    .map_or_else(|_| String::new(), |time| crate::format::ago(time, self.now));
                (&ago, self.shared.palette.muted)
            }
            _ => (&summary.date, self.shared.palette.muted),
        };
        fonts::note_text(ui, &egui::TextStyle::Body.resolve(ui.style()), text);
        ui.add(
            Label::new(RichText::new(text).color(color))
                .selectable(false)
                .truncate(),
        );
    }

    fn default_row_height(&self) -> f32 {
        ROW_HEIGHT
    }
}

impl CommitRows<'_> {
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
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
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
        .show(ui, |ui| view.work.show(ui, status, staging, commit))
        .inner;
    match shown {
        Some(WorkEvent::Open(work)) => view.open_work(shared, work),
        Some(WorkEvent::Stage(request)) => actions.push(Action::Stage(shared.id, request)),
        Some(WorkEvent::Commit(made)) => actions.push(Action::Commit(shared.id, made)),
        None => {}
    }
}

/// The selected commit's details and changed files.
fn details(ui: &mut Ui, shared: &Shared<'_>, history: &mut HistoryState, view: &mut HistoryView) {
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
    egui::ScrollArea::vertical()
        .id_salt("commit-message")
        .max_height(height * 0.5)
        .auto_shrink([false, true])
        .show(ui, |ui| {
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
    ui.add_space(8.0);
    let counts = view.counting.of(shared, &shown);
    files_heading(ui, shown.changes.len(), counts.as_ref(), shared.palette);
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
    let description = ui
        .add(widgets::text_area(&mut edit.description).desired_rows(5))
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
    widgets::fact(ui, DETAIL_LABEL, "Commit", |ui| {
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
    let committed_apart = commit.committer.name != commit.author.name
        || commit.committer.email != commit.author.email
        || commit.committer.time != commit.author.time;
    if committed_apart {
        person(ui, ("Committer", "Committed"), &commit.committer);
    }
    if commit.signed {
        widgets::fact_top(ui, DETAIL_LABEL, "Signature", |ui| {
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

/// An author or committer: the name, leading, and the email beneath, muted; then when, as a
/// fact of its own (`when` names it).
fn person(ui: &mut Ui, (role, when): (&str, &str), signature: &egit_git::Signature) {
    widgets::fact_top(ui, DETAIL_LABEL, role, |ui| {
        ui.add(Label::new(RichText::new(signature.name.to_str_lossy()).medium()).wrap());
        ui.add(Label::new(muted(ui, signature.email.to_str_lossy()).small()).wrap());
    });
    widgets::fact(ui, DETAIL_LABEL, when, |ui| {
        ui.add(Label::new(date_time(signature.time, signature.offset_minutes)).truncate());
    });
}

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
