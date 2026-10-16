//! A repository's history: the commit table (a painted graph column, ref badges, subjects,
//! authors, dates, names) and the selected commit's details and changed files.
//!
//! The table is egui-table-kit's layout `Table`, driven directly: only rows on screen are drawn,
//! their lanes replayed from the nearest chunk and their summaries read as they come into view,
//! so a frame costs the same at row 10 as at row 1,000,000. (Its `TableKit` keeps state for
//! every row, which a graph, never sorted or filtered, doesn't need.)

use std::ops::Range;
use std::sync::Arc;

use bstr::{BStr, ByteSlice as _};
use egit_git::{CancelToken, FileChange, Git, Head, ObjectId, RefKind, Reference, Repository};
use egit_graph::{Chunk, Level, Segment, for_each_row};
use egui::epaint::CubicBezierShape;
use egui::{
    Align, Color32, CornerRadius, Frame, Id, Key, Label, Margin, Modifiers, Pos2, RichText, Sense,
    Stroke, Ui, WidgetInfo, WidgetType, pos2,
};
use egui_async::Bind;
use egui_table_kit::layout::{
    CellInfo, Column, HeaderCellInfo, HeaderRow, PrefetchInfo, Table, TableDelegate, TableState,
};

use super::file::{self, FileView};
use crate::app::Action;
use crate::fonts;
use crate::format::{count, date_time};
use crate::history::{
    ChangeCounts, CommitDetails, History, HistoryState, Objects, Summaries, read_change_counts,
    read_details,
};
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::tables::ChangesTable;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};
use crate::worktree::WorkTree;

/// A row's height.
const ROW_HEIGHT: f32 = 24.0;

/// The header's height.
const HEADER_HEIGHT: f32 = 26.0;

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
        }
    }

    /// Opens `change` of commit `details` in a file view, shown as the one open was.
    fn open_file(&mut self, shared: &Shared<'_>, details: &CommitDetails, change: FileChange) {
        self.file = Some(FileView::open(shared, details, change, self.file.as_ref()));
    }

    /// Where on screen the place before cell `cell` of line `line` was drawn, in the view shown.
    #[cfg(test)]
    pub(crate) fn file_spot(&self, line: u32, cell: u32) -> Option<egui::Pos2> {
        self.file.as_ref()?.spot(line, cell)
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
        let selected = self.selected.map(|selected| selected.id);
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
        let selected = self.selected.map(|selected| selected.id);
        !self.details.is_pending()
            && self
                .details
                .ok_ref()
                .is_some_and(|details| Some(details.id) == selected)
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
        }
    }

    /// Finds its place again in a history that replaced the one shown: the selected commit,
    /// and the commit that was at the top of the screen.
    fn follow(&mut self, history: &History) {
        if let Some(selected) = self.selected {
            self.selected = history
                .row_of(&selected.id, selected.row)
                .map(|row| Selected { row, ..selected });
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
        actions,
    } = pane;
    if std::mem::take(&mut history.replaced) {
        view.follow(&history.shown);
    }
    let shared = Shared {
        id,
        git,
        repository,
        head: HeadAt::of(head),
        palette: Palette::of(ui.ctx()),
        partial_clone,
    };

    egui::Panel::top(Id::new(("history-bar", id)))
        .frame(Frame::new().inner_margin(Margin::symmetric(12, 8)))
        .show(ui, |ui| {
            top_bar(
                ui,
                &shared,
                history,
                work_tree,
                writing_commit_graph,
                actions,
            );
        });

    if history.shown.rows() == 0 {
        egui::CentralPanel::default()
            .frame(Frame::new().inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                if !history.shown.is_loading() && history.shown.problem().is_none() {
                    ui.label(muted(ui, "No commits yet."));
                }
            });
        return;
    }

    egui::Panel::right(Id::new(("history-details", id)))
        .resizable(true)
        .default_size(380.0)
        .size_range(260.0..=900.0)
        .frame(Frame::new().inner_margin(Margin::same(12)))
        .show(ui, |ui| details(ui, &shared, history, view));

    // A file looked at belongs to the selected commit, until another is selected.
    if view
        .file
        .as_ref()
        .is_some_and(|file| view.selected.map(|selected| selected.id) != Some(file.commit()))
    {
        view.file = None;
    }
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| match &mut view.file {
            Some(file) => {
                if !file::show(ui, &shared, &history.objects, file) {
                    view.file = None;
                }
            }
            None => commit_table(ui, &shared, history, view),
        });
}

/// What the pane's parts share.
pub(super) struct Shared<'a> {
    pub(super) id: RepoId,
    pub(super) git: Option<&'a Git>,
    pub(super) repository: &'a Arc<Repository>,
    head: HeadAt<'a>,
    pub(super) palette: Palette,
    partial_clone: Option<bool>,
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
    writing: &mut Bind<(), Problem>,
    actions: &mut Vec<Action>,
) {
    let palette = shared.palette;
    ui.horizontal(|ui| {
        let rows = history.shown.rows();
        let commits = if rows == 1 {
            "1 commit".to_owned()
        } else {
            format!("{} commits", count(rows))
        };
        ui.label(RichText::new(commits).strong());
        if history.is_loading() {
            ui.spinner();
            ui.label(muted(ui, "Reading the history…"));
        }
        work_tree_summary(ui, work_tree, palette);
    });
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

/// How many files have changes not yet committed, if any; or why that isn't known.
fn work_tree_summary(ui: &mut Ui, work_tree: &mut WorkTree, palette: Palette) {
    match work_tree.status() {
        Some(Ok(status)) if !status.is_empty() => {
            let files = status.len();
            let text = if files == 1 {
                "1 file with uncommitted changes".to_owned()
            } else {
                format!(
                    "{} files with uncommitted changes",
                    count(u64::try_from(files).unwrap_or(u64::MAX))
                )
            };
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}  {text}", icon::PENCIL_SIMPLE)).color(palette.warn));
        }
        Some(Err(problem)) => {
            let problem = problem.to_string();
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{}  The work tree couldn't be read",
                    icon::WARNING_CIRCLE
                ))
                .color(palette.danger),
            )
            .on_hover_text(problem);
        }
        _ => {}
    }
}

/// The commit table, and the keys that move through it.
fn commit_table(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &mut HistoryState,
    view: &mut HistoryView,
) {
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
        objects,
        ..
    } = history;
    let mut rows = CommitRows {
        shared,
        history: shown,
        summaries,
        objects,
        view,
        clicked: None,
    };
    table.show(ui, &mut rows);
    if let Some(row) = rows.clicked {
        view.select(shown, row, None);
        ui.memory_mut(|memory| memory.request_focus(keys));
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
    objects: &'a Objects,
    view: &'a mut HistoryView,
    /// The row clicked this frame.
    clicked: Option<u64>,
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
                .chain(below)
                .chain(above.rev())
                .filter_map(|row| history.id(row));
            self.summaries
                .want(git, self.shared.repository, self.objects, wanted);
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
        ui.add_space(GRAPH_MARGIN);
        ui.add(
            Label::new(RichText::new(title).strong())
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
        let fill = if selected {
            Some(self.shared.palette.accent.gamma_multiply(0.28))
        } else if response.hovered() {
            Some(self.shared.palette.surface_alt)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
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
        response.context_menu(|ui| {
            if ui.button("Copy commit name").clicked() {
                ui.ctx().copy_text(id.to_string());
                ui.close();
            }
            if let Some(summary) = summaries.peek(&id)
                && ui.button("Copy subject").clicked()
            {
                ui.ctx().copy_text(summary.subject.to_string());
                ui.close();
            }
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
            badges(
                ui,
                self.history.refs().at(&id),
                &self.shared.head,
                self.shared.palette,
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
        let (text, color) = match cell.col_nr {
            1 => (&summary.subject, self.shared.palette.text),
            2 => (&summary.author, self.shared.palette.text),
            _ => (&summary.date, self.shared.palette.muted),
        };
        fonts::note_text(ui, &egui::TextStyle::Body.resolve(ui.style()), text);
        ui.add(
            Label::new(RichText::new(&**text).color(color))
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

/// The refs at a commit, as badges: branches, then remote branches, then tags.
fn badges(ui: &mut Ui, refs: &[Reference], head: &HeadAt<'_>, palette: Palette) {
    for reference in refs.iter().take(MAX_BADGES) {
        let checked_out = head.reference == Some(reference.name.as_bstr());
        let (glyph, color) = match reference.kind {
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
        let label = format!("{glyph} {}", reference.short_name().to_str_lossy());
        Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.7)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(5, 0))
            .show(ui, |ui| {
                let text = RichText::new(label).size(12.0).color(text);
                ui.add(
                    Label::new(if checked_out { text.strong() } else { text }).selectable(false),
                );
            });
    }
    if let Some(more) = refs.len().checked_sub(MAX_BADGES).filter(|more| *more > 0) {
        ui.add(Label::new(muted(ui, format!("+{more}"))).selectable(false));
    }
}

/// The selected commit's details and changed files.
fn details(ui: &mut Ui, shared: &Shared<'_>, history: &HistoryState, view: &mut HistoryView) {
    let Some(selected) = view.selected else {
        ui.label(muted(ui, "Select a commit to see what it changed."));
        return;
    };
    if view.details_for != Some(selected.id)
        && let Some(git) = shared.git
    {
        view.details.request(read_details(
            git.clone(),
            Arc::clone(shared.repository),
            history.objects.clone(),
            selected.id,
        ));
        view.details_for = Some(selected.id);
    }
    let pending = view.details.is_pending();
    let shown = match view.details.read_as_ref() {
        None => {
            widgets::working(ui, "Reading the commit…");
            return;
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
            return;
        }
        Some(Ok(details)) => Arc::clone(details),
    };
    if pending {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(muted(ui, "Reading the commit…"));
        });
    }
    let mut go_to = None;
    let height = ui.available_height();
    egui::ScrollArea::vertical()
        .id_salt("commit-message")
        .max_height(height * 0.5)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            go_to = commit_info(ui, &shown, history.shown.refs().at(&shown.id), shared);
        });
    ui.add_space(8.0);
    let counts = view.counting.of(shared, &shown);
    files_heading(ui, shown.changes.len(), counts.as_ref(), shared.palette);
    let height = ui.available_height();
    let counts = counts.and_then(Result::ok);
    if let Some(index) = view.changes.show(ui, &shown, counts.as_deref(), height)
        && let Some(change) = shown.changes.get(index)
    {
        view.open_file(shared, &shown, change.clone());
    }

    if let Some(parent) = go_to
        && let Some(row) = history.shown.row_of(&parent, selected.row)
    {
        view.select(&history.shown, row, Some(Align::Center));
    }
}

/// "3 files changed", and the lines they gained and lost once counted.
fn files_heading(
    ui: &mut Ui,
    files: usize,
    counts: Option<&Result<Arc<ChangeCounts>, Problem>>,
    palette: Palette,
) {
    ui.horizontal(|ui| {
        widgets::heading(
            ui,
            &if files == 1 {
                "1 file changed".to_owned()
            } else {
                format!(
                    "{} files changed",
                    count(u64::try_from(files).unwrap_or(u64::MAX))
                )
            },
        );
        match counts {
            Some(Ok(counts)) => {
                let total = counts.total;
                ui.label(
                    RichText::new(format!("+{}", count(u64::from(total.added)))).color(palette.ok),
                );
                ui.label(
                    RichText::new(format!("−{}", count(u64::from(total.removed))))
                        .color(palette.danger),
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

/// The commit's message, names, people, and refs; returns a parent the user chose to go to.
fn commit_info(
    ui: &mut Ui,
    details: &CommitDetails,
    refs: &[Reference],
    shared: &Shared<'_>,
) -> Option<ObjectId> {
    let commit = &details.commit;
    let mut go_to = None;
    ui.add(
        Label::new(
            RichText::new(commit.subject().to_str_lossy())
                .size(16.0)
                .strong(),
        )
        .wrap(),
    );
    let body = commit.body().trim_ascii();
    if !body.is_empty() {
        ui.add_space(4.0);
        ui.add(Label::new(body.to_str_lossy()).wrap());
    }
    ui.add_space(8.0);
    egui::Grid::new("commit-info")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(muted(ui, "Commit"));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(details.id.abbreviated(ID_DIGITS * 2).to_string()).monospace(),
                );
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(details.id.to_string());
                }
            });
            ui.end_row();

            ui.label(muted(
                ui,
                if commit.parents.len() == 1 {
                    "Parent"
                } else {
                    "Parents"
                },
            ));
            ui.horizontal_wrapped(|ui| {
                if commit.parents.is_empty() {
                    ui.label(muted(ui, "None: the first commit"));
                }
                // Buttons rather than links: egui announces a link as plain text.
                for parent in &commit.parents {
                    let name = parent.abbreviated(ID_DIGITS).to_string();
                    let link = RichText::new(name)
                        .monospace()
                        .color(ui.visuals().hyperlink_color);
                    let response = ui.add(egui::Button::new(link).frame(false));
                    if response.on_hover_text("Go to this parent").clicked() {
                        go_to = Some(*parent);
                    }
                }
            });
            ui.end_row();

            person(ui, "Author", &commit.author);
            let committed_apart = commit.committer.name != commit.author.name
                || commit.committer.email != commit.author.email
                || commit.committer.time != commit.author.time;
            if committed_apart {
                person(ui, "Committer", &commit.committer);
            }
            if commit.signed {
                ui.label(muted(ui, "Signature"));
                ui.label(format!("{}  Signed (not verified yet)", icon::SEAL));
                ui.end_row();
            }
            if !refs.is_empty() {
                ui.label(muted(ui, "Refs"));
                ui.horizontal_wrapped(|ui| badges(ui, refs, &shared.head, shared.palette));
                ui.end_row();
            }
        });
    go_to
}

/// A grid row for an author or committer: name, email, and when.
fn person(ui: &mut Ui, role: &str, signature: &egit_git::Signature) {
    ui.label(muted(ui, role));
    ui.vertical(|ui| {
        ui.label(format!(
            "{} <{}>",
            signature.name.to_str_lossy(),
            signature.email.to_str_lossy()
        ));
        ui.label(muted(
            ui,
            date_time(signature.time, signature.offset_minutes),
        ));
    });
    ui.end_row();
}

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
