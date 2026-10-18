//! The sidebar beside the commit table: the repository's branches (folded into folders by
//! their `/`s), its remotes (each with its branches), its pull or merge requests, tags,
//! stashes, worktrees, and submodules, each section counted and folding away; a filter
//! narrowing them all; and an eye at each branch's right saying whether the graph shows it.
//!
//! A tree's levels step in by a whole icon: a folder's (or remote's) children have their
//! icons under its name, a guide running down from its icon beside them; its caret hangs left
//! of its icon, so folders and leaves at one depth line up.
//!
//! A ref's row selects its commit in the history (or shows it in the details when the history
//! doesn't have it), its menu is its badge's; a stash's shows its commit in the details; a
//! worktree or submodule opens in a tab on a double click. Remotes, worktrees, and submodules
//! are set up from their menus.
//!
//! The rows are made again only when what they show changed (another read of the refs, a list
//! read again, the filter, a fold), and drawn only as far as they show: a repository with
//! 100,000 refs costs what the screen holds.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_forge::CheckState;
use egit_git::{ConfigSnapshot, ObjectId, RefKind, Reference, Stash, Submodule, Worktree};
use egit_text::{Needle, matches_case};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Color32, FontId, Id, Key, KeyboardShortcut, Modifiers, Rect, Response, RichText, Sense,
    TextFormat, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use egui_async::Bind;

use super::actions::{self, Ask, Menus};
use super::dialogs::{Dialog, WorktreeAsked};
use super::history::{ID_DIGITS, Shared};
use super::pulls::check_look;
use crate::forge::{RepoForge, request_number};
use crate::format::display_path;
use crate::history::{Refs, Scope};
use crate::jobs;
use crate::problem::Problem;
use crate::tasks::{Task, remote_names};
use crate::theme::{Palette, RADIUS_INNER, SMALL, Weight, icon};
use crate::widgets::{TARGET, muted};

/// A row's height: a whole target to hit, and rows touch (no gap where the pointer finds
/// nothing).
const ROW_HEIGHT: f32 = 28.0;

/// Left of every row, before the column a section's (or a folder's) caret hangs in.
const PAD: f32 = 6.0;

/// The column a caret hangs in, left of its row's icon.
const CARET: f32 = 16.0;

/// A row's icon's width.
const ICON: f32 = 14.0;

/// How far each level of a tree steps in: from a row's icon to its words, under which its
/// children's icons go.
const INDENT: f32 = 20.0;

/// Room at a branch's right for its eye (shown in the graph or hidden), a whole target.
const EYE: f32 = TARGET;

/// The lists the sidebar shows besides the refs, as the tab last read them.
pub(crate) struct Lists<'a> {
    pub(crate) stashes: Option<Result<&'a Arc<[Stash]>, &'a Problem>>,
    pub(crate) worktrees: Option<Result<&'a Arc<[Worktree]>, &'a Problem>>,
    pub(crate) submodules: Option<Result<&'a Arc<[Submodule]>, &'a Problem>>,
    /// Its Git LFS, once read.
    pub(crate) lfs: Option<Result<&'a Arc<crate::lfs::LfsState>, &'a Problem>>,
    /// Where its submodules stand, nested ones too, once read.
    pub(crate) submodule_states: Option<Result<&'a Arc<crate::submodules::Standing>, &'a Problem>>,
    pub(crate) config: Option<&'a Arc<ConfigSnapshot>>,
}

/// The sidebar's sections, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Branches,
    Remotes,
    Requests,
    Tags,
    Stashes,
    Worktrees,
    Submodules,
}

impl Section {
    /// Its title, a forge's requests named its way.
    fn title(self, forge: &RepoForge) -> String {
        match self {
            Self::Branches => "Branches".to_owned(),
            Self::Remotes => "Remotes".to_owned(),
            Self::Requests => {
                let nouns = forge.nouns();
                let mut letters = nouns.chars();
                letters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(letters).collect()
                })
            }
            Self::Tags => "Tags".to_owned(),
            Self::Stashes => "Stashes".to_owned(),
            Self::Worktrees => "Worktrees".to_owned(),
            Self::Submodules => "Submodules".to_owned(),
        }
    }

    const fn icon(self) -> &'static str {
        match self {
            Self::Branches => icon::GIT_BRANCH,
            Self::Remotes => icon::CLOUD,
            Self::Requests => icon::GIT_PULL_REQUEST,
            Self::Tags => icon::TAG,
            Self::Stashes => icon::ARCHIVE,
            Self::Worktrees => icon::TREE_STRUCTURE,
            Self::Submodules => icon::PACKAGE,
        }
    }
}

/// A row of the sidebar.
#[derive(Clone, Debug)]
enum Row {
    /// A section's header, and how many it holds (that match the filter).
    Section {
        section: Section,
        count: usize,
    },
    /// A folder of refs (`feature/` of `feature/ui`): its key (a remote's branches' starts with
    /// the remote), how deep, and where its own name starts in the key.
    Folder {
        section: Section,
        key: BString,
        depth: u8,
        name: usize,
    },
    /// A remote, its branches below.
    Remote {
        remote: usize,
    },
    /// A ref (its place among the refs), how deep, and where its last part starts.
    Ref {
        index: usize,
        depth: u8,
        leaf: usize,
    },
    Stash(usize),
    Worktree(usize),
    Submodule(usize),
    Request(usize),
    /// A section with nothing to show, or a list that couldn't be read.
    Note {
        text: String,
        warn: bool,
    },
}

/// A pull (or merge) request as its row shows it.
#[derive(Clone, Debug)]
struct Request {
    repository: usize,
    number: u32,
    label: String,
    checks: CheckState,
}

/// What a row stands for, to keep it selected as the rows are made again.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Picked {
    Ref(BString),
    Stash(ObjectId),
    Worktree(PathBuf),
    Submodule(BString),
    Request(usize, u32),
}

/// What the rows were made from: when any of it changes, they are made again.
#[derive(Debug, Default)]
struct Made {
    sorted: Arc<Sorted>,
    stashes: Option<Arc<[Stash]>>,
    worktrees: Option<Arc<[Worktree]>>,
    submodules: Option<Arc<[Submodule]>>,
    config: Option<Arc<ConfigSnapshot>>,
    requests: (u64, usize),
    problems: usize,
}

/// What the sidebar asks of the history view.
#[derive(Debug)]
pub(super) enum SidebarEvent {
    /// What a menu (or a click) asked.
    Ask(Ask),
    /// Show this commit: selected in the history when it is there, in the details otherwise.
    Show(ObjectId),
    /// Open this folder's repository in a tab.
    Open(PathBuf),
    /// Show the submodules' page.
    Submodules,
}

/// The filter: what is typed, and what it asks next frame.
#[derive(Debug, Default)]
struct Filter {
    text: String,
    /// Whether to scroll back to the top (what is typed changed).
    to_top: bool,
    /// Whether the field should take the focus (asked from the palette).
    focus: bool,
}

/// The sidebar's state.
#[derive(Debug)]
pub(crate) struct Sidebar {
    /// Whether it is shown.
    pub(super) open: bool,
    filter: Filter,
    /// The folded sections (an empty key), folders, and remotes.
    folded: HashSet<(Section, BString)>,
    rows: Vec<Row>,
    made: Option<Made>,
    /// Whether a fold or the filter changed since the rows were made.
    stale: bool,
    /// The refs as listed, once sorted (the rows show these refs, until another sort lands).
    sorted: Arc<Sorted>,
    /// The sort under way, or done.
    sorting: Bind<Arc<Sorted>, Problem>,
    /// What the last sort was asked for: a read of the refs, and the configuration (its
    /// remotes).
    sorting_for: Option<(u64, Option<Arc<ConfigSnapshot>>)>,
    requests: Vec<Request>,
    selected: Option<Picked>,
    /// Whether the filter changed since the rows were made (so they may be made in a job).
    filtered: bool,
    /// Rows a job is making, and which making they were (only the latest's are taken).
    making: Bind<(u64, Vec<Row>), Problem>,
    generation: u64,
}

/// How many refs make filtering worth a job: fewer are narrowed within a frame.
const ROWS_IN_A_JOB: usize = 5_000;

/// A list as read (or why it couldn't be), owned.
fn owned<T: ?Sized>(read: Result<&Arc<T>, &Problem>) -> Result<Arc<T>, String> {
    read.map(Arc::clone).map_err(ToString::to_string)
}

/// What the rows are made from, owned, so a job can make them.
struct RowsInput {
    sorted: Arc<Sorted>,
    query: String,
    folded: HashSet<(Section, BString)>,
    stashes: Option<Result<Arc<[Stash]>, String>>,
    worktrees: Option<Result<Arc<[Worktree]>, String>>,
    submodules: Option<Result<Arc<[Submodule]>, String>>,
    /// The forge's requests' labels, when a remote is on a forge.
    requests: Option<Vec<String>>,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            open: true,
            filter: Filter::default(),
            // Tags are often many: folded at first.
            folded: HashSet::from([(Section::Tags, BString::default())]),
            rows: Vec::new(),
            made: None,
            stale: true,
            sorted: Arc::default(),
            sorting: Bind::new(true),
            sorting_for: None,
            requests: Vec::new(),
            selected: None,
            filtered: false,
            making: Bind::new(true),
            generation: 0,
        }
    }
}

/// Compares two names as people read them: runs of digits by their value (`v2.9` before
/// `v2.10`), letters in either case alike, then byte by byte, so only equal names are equal.
fn natural(a: &[u8], b: &[u8]) -> Ordering {
    let (mut left, mut right) = (a, b);
    loop {
        match (left.first(), right.first()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let digits =
                    |text: &[u8]| text.iter().take_while(|byte| byte.is_ascii_digit()).count();
                let (one, two) = (digits(left), digits(right));
                let (first, second) = (left.split_at(one), right.split_at(two));
                let significant = |run: &[u8]| {
                    let zeros = run.iter().take_while(|byte| **byte == b'0').count();
                    run.get(zeros..).unwrap_or_default().to_vec()
                };
                let (x, y) = (significant(first.0), significant(second.0));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
                (left, right) = (first.1, second.1);
            }
            (Some(x), Some(y)) => {
                let order = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                (left, right) = (
                    left.get(1..).unwrap_or_default(),
                    right.get(1..).unwrap_or_default(),
                );
            }
        }
    }
}

/// Compares two names part by part (split at `/`), each naturally: a folder's names stay
/// together.
fn by_parts(a: &[u8], b: &[u8]) -> Ordering {
    let (mut left, mut right) = (a.split_str("/"), b.split_str("/"));
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = natural(x, y);
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Where a kind of ref is listed.
const fn kind_rank(kind: RefKind) -> u8 {
    match kind {
        RefKind::Branch => 0,
        RefKind::RemoteBranch => 1,
        RefKind::Tag => 2,
        RefKind::Other => 3,
    }
}

/// Rows for refs in a tree, folders made of the `/`s in their names (`items`: each ref's place,
/// its name in the tree, and where that name starts in its short name), sorted by parts.
struct Tree<'a> {
    section: Section,
    /// How deep its first level is.
    base: u8,
    /// What comes before a folder's path in its key (a remote's name and `/`).
    prefix: &'a [u8],
    /// Whether a folder (by key) is folded.
    folded: &'a dyn Fn(&[u8]) -> bool,
}

impl Tree<'_> {
    fn rows<'n>(&self, rows: &mut Vec<Row>, items: impl Iterator<Item = (usize, &'n [u8], usize)>) {
        // The ends (in the current name) of the folders open, and the first one folded.
        let mut open: Vec<usize> = Vec::new();
        let mut folded_at: Option<usize> = None;
        let mut previous: &[u8] = b"";
        let level = |open: &[usize]| {
            self.base
                .saturating_add(u8::try_from(open.len()).unwrap_or(u8::MAX))
        };
        for (index, name, offset) in items {
            while let Some(&end) = open.last() {
                if name.get(..end) == previous.get(..end) && name.get(end) == Some(&b'/') {
                    break;
                }
                open.pop();
            }
            if folded_at.is_some_and(|depth| open.len() <= depth) {
                folded_at = None;
            }
            let mut from = open.last().map_or(0, |end| end.saturating_add(1));
            while let Some(slash) = name.get(from..).and_then(|rest| rest.find_byte(b'/')) {
                let end = from.saturating_add(slash);
                if folded_at.is_none() {
                    let key = [self.prefix, name.get(..end).unwrap_or_default()].concat();
                    let folded = (self.folded)(&key);
                    rows.push(Row::Folder {
                        section: self.section,
                        key: BString::from(key),
                        depth: level(&open),
                        name: self.prefix.len().saturating_add(from),
                    });
                    if folded {
                        folded_at = Some(open.len());
                    }
                }
                open.push(end);
                from = end.saturating_add(1);
            }
            if folded_at.is_none() {
                rows.push(Row::Ref {
                    index,
                    depth: level(&open),
                    leaf: offset.saturating_add(from),
                });
            }
            previous = name;
        }
    }
}

/// The refs as the sidebar lists them: sorted by kind, then by their names' parts, each remote
/// branch with its remote. Worked out in a job (100,000 refs take 90 ms) once a read of the refs,
/// or when the remotes change.
#[derive(Debug, Default)]
struct Sorted {
    /// The refs, as read.
    refs: Arc<[Reference]>,
    /// Their places, by kind and then name.
    order: Vec<usize>,
    /// Each remote branch's remote, by its place in `order` (`usize::MAX` for the others).
    branch_remote: Vec<usize>,
    /// The remotes: those configured, and any the refs name besides.
    remotes: Vec<BString>,
}

/// Sorts `refs` for the sidebar, finding each remote branch's remote among `configured`.
fn sort(refs: Arc<[Reference]>, configured: Vec<BString>) -> Sorted {
    let mut order: Vec<usize> = (0..refs.len()).collect();
    order.sort_unstable_by(|a, b| match (refs.get(*a), refs.get(*b)) {
        (Some(a), Some(b)) => kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| by_parts(a.short_name(), b.short_name())),
        _ => Ordering::Equal,
    });
    let mut remotes = configured;
    remotes.sort_unstable_by(|a, b| natural(a, b));
    // Each remote branch's remote: the longest configured name it starts with, else its first
    // part (a remote no longer configured).
    let mut branch_remote = Vec::with_capacity(order.len());
    for index in &order {
        let Some(reference) = refs
            .get(*index)
            .filter(|reference| reference.kind == RefKind::RemoteBranch)
        else {
            branch_remote.push(usize::MAX);
            continue;
        };
        let short = reference.short_name();
        let known = remotes
            .iter()
            .enumerate()
            .filter(|(_, remote)| {
                short.len() > remote.len()
                    && short.starts_with(remote.as_slice())
                    && short.get(remote.len()) == Some(&b'/')
            })
            .max_by_key(|(_, remote)| remote.len())
            .map(|(at, _)| at);
        let at = known.unwrap_or_else(|| {
            let first = short.split_str("/").next().unwrap_or_default();
            remotes
                .iter()
                .position(|remote| remote.as_slice() == first)
                .unwrap_or_else(|| {
                    remotes.push(BString::from(first));
                    remotes.len().saturating_sub(1)
                })
        });
        branch_remote.push(at);
    }
    Sorted {
        refs,
        order,
        branch_remote,
        remotes,
    }
}

/// Whether `kept` is `now` (the same list, not one read again).
fn same<T: ?Sized>(kept: Option<&Arc<T>>, now: Option<&Arc<T>>) -> bool {
    match (kept, now) {
        (Some(kept), Some(now)) => Arc::ptr_eq(kept, now),
        (None, None) => true,
        _ => false,
    }
}

impl Sidebar {
    /// Gives the filter the focus (next frame).
    pub(super) const fn focus_filter(&mut self) {
        self.filter.focus = true;
    }

    /// Sorts the refs again (in a job) when they were read again or the remotes changed; takes
    /// in a sort that landed.
    fn sort_refs(&mut self, refs: &Refs, lists: &Lists<'_>) {
        let asked = self.sorting_for.as_ref().is_some_and(|(read, config)| {
            *read == refs.read() && same(config.as_ref(), lists.config)
        });
        if !asked {
            let configured = lists.config.map_or_else(Vec::new, |config| {
                remote_names(config)
                    .into_iter()
                    .map(BString::from)
                    .collect()
            });
            let shared = refs.shared();
            self.sorting.request(jobs::blocking(move || {
                Ok(Arc::new(sort(shared, configured)))
            }));
            self.sorting_for = Some((refs.read(), lists.config.cloned()));
        }
        let landed = match self.sorting.read_as_ref() {
            Some(Ok(sorted)) if !Arc::ptr_eq(sorted, &self.sorted) => Some(Arc::clone(sorted)),
            _ => None,
        };
        if let Some(sorted) = landed {
            self.sorted = sorted;
        }
    }

    /// Makes the rows again when what they show changed (taking those a job made, unless
    /// newer ones were made since).
    fn refresh(&mut self, refs: &Refs, lists: &Lists<'_>, forge: &RepoForge) {
        if let Some((generation, rows)) = self.making.take_ok()
            && generation == self.generation
        {
            self.rows = rows;
        }
        let stashes = lists.stashes.and_then(Result::ok);
        let worktrees = lists.worktrees.and_then(Result::ok);
        let submodules = lists.submodules.and_then(Result::ok);
        let problems = [
            lists.stashes.is_some_and(|read| read.is_err()),
            lists.worktrees.is_some_and(|read| read.is_err()),
            lists.submodules.is_some_and(|read| read.is_err()),
        ]
        .iter()
        .filter(|failed| **failed)
        .count();
        self.sort_refs(refs, lists);
        let listed = forge.listed();
        let current = self.made.as_ref().is_some_and(|made| {
            Arc::ptr_eq(&made.sorted, &self.sorted)
                && same(made.stashes.as_ref(), stashes)
                && same(made.worktrees.as_ref(), worktrees)
                && same(made.submodules.as_ref(), submodules)
                && same(made.config.as_ref(), lists.config)
                && made.requests == listed
                && made.problems == problems
        });
        if current && !self.stale {
            return;
        }
        // One job at a time: what is typed meanwhile is made once it lands (the latest only).
        let in_a_job = self.filtered && self.sorted.refs.len() > ROWS_IN_A_JOB;
        if in_a_job && self.making.is_pending() {
            return;
        }
        if self
            .made
            .as_ref()
            .is_none_or(|made| made.requests != listed)
        {
            let found = forge.found();
            self.requests = found
                .pulls
                .iter()
                .map(|listed| Request {
                    repository: listed.repository,
                    number: listed.pull.number,
                    label: format!(
                        "{} {}",
                        request_number(forge.kind_of(listed.repository), listed.pull.number),
                        listed.pull.title
                    ),
                    checks: listed.pull.checks,
                })
                .collect();
        }
        let input = self.rows_input(lists, forge);
        self.generation = self.generation.wrapping_add(1);
        // Typing in the filter over many refs: the rows are made in a job, the last ones shown
        // until they land (a frame or two), so no letter waits for them.
        self.filtered = false;
        if in_a_job {
            let generation = self.generation;
            self.making.request(jobs::blocking(move || {
                Ok((generation, Self::make_rows(&input)))
            }));
        } else {
            self.rows = Self::make_rows(&input);
        }
        self.made = Some(Made {
            sorted: Arc::clone(&self.sorted),
            stashes: stashes.cloned(),
            worktrees: worktrees.cloned(),
            submodules: submodules.cloned(),
            config: lists.config.cloned(),
            requests: listed,
            problems,
        });
        self.stale = false;
    }

    /// What the rows are made from now, owned (a job may make them).
    fn rows_input(&self, lists: &Lists<'_>, forge: &RepoForge) -> RowsInput {
        RowsInput {
            sorted: Arc::clone(&self.sorted),
            query: self.filter.text.trim().to_owned(),
            folded: self.folded.clone(),
            stashes: lists.stashes.map(owned),
            worktrees: lists.worktrees.map(owned),
            submodules: lists.submodules.map(owned),
            requests: (!forge.repositories.is_empty()).then(|| {
                self.requests
                    .iter()
                    .map(|request| request.label.clone())
                    .collect()
            }),
        }
    }

    /// The rows, each section's after its header, narrowed by the filter (which unfolds
    /// everything it matches in).
    fn make_rows(input: &RowsInput) -> Vec<Row> {
        let sorted = &input.sorted;
        let refs = &sorted.refs;
        let query = input.query.as_str();
        let needle =
            (!query.is_empty()).then(|| Needle::new(query.as_bytes(), !matches_case(query)));
        let matches = |text: &[u8]| {
            needle
                .as_ref()
                .is_none_or(|needle| needle.find(text).is_some())
        };
        let filtering = needle.is_some();
        let folded = |section: Section, key: &[u8]| {
            !filtering && input.folded.contains(&(section, BString::from(key)))
        };
        let mut rows = Vec::new();
        let of_kind = |kind: RefKind| {
            sorted
                .order
                .iter()
                .enumerate()
                .filter_map(move |(at, index)| {
                    let reference = refs.get(*index)?;
                    (reference.kind == kind && matches(reference.short_name()))
                        .then_some((at, *index, reference))
                })
        };
        // Branches and tags: trees from their short names.
        for (section, kind, none) in [
            (Section::Branches, RefKind::Branch, "No branches yet"),
            (Section::Tags, RefKind::Tag, "No tags"),
        ] {
            if section == Section::Tags {
                Self::remote_rows(&mut rows, sorted, &matches, &folded);
                Self::request_rows(&mut rows, input.requests.as_deref(), &matches, &folded);
            }
            let count = of_kind(kind).count();
            rows.push(Row::Section { section, count });
            if folded(section, b"") {
                continue;
            }
            if count == 0 {
                rows.push(note(if filtering { "None match" } else { none }));
            }
            let fold = |key: &[u8]| folded(section, key);
            let tree = Tree {
                section,
                base: 0,
                prefix: b"",
                folded: &fold,
            };
            tree.rows(
                &mut rows,
                of_kind(kind)
                    .map(|(_, index, reference)| (index, reference.short_name().as_bytes(), 0)),
            );
        }
        Self::list_rows(&mut rows, input, &matches, &folded, filtering);
        rows
    }

    /// The remotes, each with its branches below.
    fn remote_rows(
        rows: &mut Vec<Row>,
        sorted: &Sorted,
        matches: &dyn Fn(&[u8]) -> bool,
        folded: &dyn Fn(Section, &[u8]) -> bool,
    ) {
        let refs = &sorted.refs;
        let branches_of = |remote: usize| {
            sorted
                .order
                .iter()
                .zip(&sorted.branch_remote)
                .filter_map(move |(index, of)| {
                    let reference = refs.get(*index)?;
                    (*of == remote && matches(reference.short_name()))
                        .then_some((*index, reference))
                })
        };
        let shown: Vec<usize> = (0..sorted.remotes.len())
            .filter(|remote| {
                sorted
                    .remotes
                    .get(*remote)
                    .is_some_and(|name| matches(name))
                    || branches_of(*remote).next().is_some()
            })
            .collect();
        rows.push(Row::Section {
            section: Section::Remotes,
            count: shown.len(),
        });
        if folded(Section::Remotes, b"") {
            return;
        }
        if shown.is_empty() {
            rows.push(note("No remotes"));
        }
        for remote in shown {
            let Some(name) = sorted.remotes.get(remote) else {
                continue;
            };
            rows.push(Row::Remote { remote });
            if folded(Section::Remotes, name) {
                continue;
            }
            let prefix = [name.as_slice(), b"/"].concat();
            let fold = |key: &[u8]| folded(Section::Remotes, key);
            let tree = Tree {
                section: Section::Remotes,
                base: 1,
                prefix: &prefix,
                folded: &fold,
            };
            let skip = prefix.len();
            tree.rows(
                rows,
                branches_of(remote).map(|(index, reference)| {
                    let short = reference.short_name().as_bytes();
                    (index, short.get(skip..).unwrap_or_default(), skip)
                }),
            );
        }
    }

    /// The pull (or merge) requests, when a remote is on a forge.
    fn request_rows(
        rows: &mut Vec<Row>,
        requests: Option<&[String]>,
        matches: &dyn Fn(&[u8]) -> bool,
        folded: &dyn Fn(Section, &[u8]) -> bool,
    ) {
        let Some(requests) = requests else {
            return;
        };
        let shown: Vec<usize> = requests
            .iter()
            .enumerate()
            .filter(|(_, label)| matches(label.as_bytes()))
            .map(|(at, _)| at)
            .collect();
        rows.push(Row::Section {
            section: Section::Requests,
            count: shown.len(),
        });
        if folded(Section::Requests, b"") {
            return;
        }
        if shown.is_empty() {
            rows.push(note("None open"));
        }
        rows.extend(shown.into_iter().map(Row::Request));
    }

    /// The stashes, worktrees, and submodules (shown when there are any).
    fn list_rows(
        rows: &mut Vec<Row>,
        lists: &RowsInput,
        matches: &dyn Fn(&[u8]) -> bool,
        folded: &dyn Fn(Section, &[u8]) -> bool,
        filtering: bool,
    ) {
        let stashes = lists.stashes.as_ref().map(|read| {
            read.as_ref().map(|stashes| {
                stashes
                    .iter()
                    .enumerate()
                    .filter(|(_, stash)| matches(stash.message.as_bytes()))
                    .map(|(at, _)| Row::Stash(at))
                    .collect::<Vec<_>>()
            })
        });
        let worktrees = lists.worktrees.as_ref().map(|read| {
            read.as_ref().map(|worktrees| {
                worktrees
                    .iter()
                    .enumerate()
                    .filter(|(_, worktree)| {
                        matches(worktree.path.as_os_str().as_encoded_bytes())
                            || worktree
                                .branch
                                .as_ref()
                                .is_some_and(|branch| matches(branch))
                    })
                    .map(|(at, _)| Row::Worktree(at))
                    .collect::<Vec<_>>()
            })
        });
        let submodules = lists.submodules.as_ref().map(|read| {
            read.as_ref().map(|submodules| {
                submodules
                    .iter()
                    .enumerate()
                    .filter(|(_, submodule)| matches(&submodule.path))
                    .map(|(at, _)| Row::Submodule(at))
                    .collect::<Vec<_>>()
            })
        });
        let has_submodules = lists.submodules.as_ref().is_some_and(|read| {
            read.is_err() || read.as_ref().is_ok_and(|submodules| !submodules.is_empty())
        });
        for (section, listed, none) in [
            (Section::Stashes, stashes, "No stashes"),
            (Section::Worktrees, worktrees, "Only this one"),
            (Section::Submodules, submodules, "No submodules"),
        ] {
            if section == Section::Submodules && !has_submodules {
                continue;
            }
            let count = match &listed {
                Some(Ok(found)) => found.len(),
                _ => 0,
            };
            rows.push(Row::Section { section, count });
            if folded(section, b"") {
                continue;
            }
            match listed {
                None => rows.push(note("Reading…")),
                Some(Err(problem)) => rows.push(Row::Note {
                    text: problem.clone(),
                    warn: true,
                }),
                Some(Ok(found)) if found.is_empty() => {
                    rows.push(note(if filtering { "None match" } else { none }));
                }
                Some(Ok(found)) => rows.extend(found),
            }
        }
    }
}

/// A muted row saying something.
fn note(text: &str) -> Row {
    Row::Note {
        text: text.to_owned(),
        warn: false,
    }
}

/// What drawing the rows needs.
struct Drawing<'a> {
    shared: &'a Shared<'a>,
    /// The refs the rows show, as sorted.
    refs: &'a [Reference],
    /// The remotes, as sorted.
    remotes: &'a [BString],
    lists: &'a Lists<'a>,
    menus: Menus<'a>,
    palette: Palette,
    font: FontId,
    /// The current branch's full name.
    current: Option<BString>,
}

/// Draws the sidebar; returns what it asks.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    refs: &Refs,
    lists: &Lists<'_>,
    view: &mut Sidebar,
) -> Option<SidebarEvent> {
    header(ui, view);
    let mut event = None;
    view.refresh(refs, lists, shared.forge);
    let sorted = Arc::clone(&view.sorted);
    let drawing = Drawing {
        shared,
        refs: &sorted.refs,
        remotes: &sorted.remotes,
        lists,
        menus: shared.menus(refs),
        palette: shared.palette,
        font: egui::TextStyle::Body.resolve(ui.style()),
        current: shared
            .branch()
            .map(|branch| BString::from([b"refs/heads/".as_slice(), branch].concat())),
    };
    let count = view.rows.len();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("sidebar-rows", shared.id))
        .auto_shrink([false, false]);
    if std::mem::take(&mut view.filter.to_top) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    // Rows touch: no spacing between them (`show_rows` counts it in their pitch).
    ui.spacing_mut().item_spacing.y = 0.0;
    scroll.show_rows(ui, ROW_HEIGHT, count, |ui, visible| {
        for at in visible {
            let Some(row) = view.rows.get(at).cloned() else {
                continue;
            };
            if let Some(asked) = draw_row(ui, &drawing, view, &row) {
                event = Some(asked);
            }
        }
    });
    event
}

/// The filter, and the button hiding the sidebar.
fn header(ui: &mut Ui, view: &mut Sidebar) {
    // In the window's head band, level with the commit table's column titles.
    crate::widgets::panel_head(ui, crate::widgets::Head::Control, |ui| {
        let (_, place) = ui.allocate_space(vec2(TARGET, TARGET));
        let hide = crate::widgets::icon_button(
            ui,
            place,
            Id::new("sidebar-hide"),
            icon::CARET_DOUBLE_LEFT,
            "Hide the sidebar",
            true,
        );
        if hide.on_hover_text("Hide the sidebar").clicked() {
            view.open = false;
        }
        let label = ui.label(muted(ui, "Filter"));
        let field = ui
            .add(
                crate::widgets::line_field(&mut view.filter.text)
                    .hint_text("branches, tags, …")
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(label.id)
            .on_hover_text(format!(
                "Narrow what the sidebar lists ({})",
                crate::commands::keys_text("⌥⌘F")
            ));
        let asked = std::mem::take(&mut view.filter.focus);
        if asked
            || ui.input_mut(|input| {
                input.consume_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::ALT,
                    Key::F,
                ))
            })
        {
            field.request_focus();
        }
        // The field lets the focus go on Escape, in the frame it is pressed.
        let escaped = (field.has_focus() || field.lost_focus())
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        if escaped && !view.filter.text.is_empty() {
            view.filter.text.clear();
            view.stale = true;
            view.filtered = true;
            view.filter.to_top = true;
        }
        if field.changed() {
            view.stale = true;
            view.filtered = true;
            view.filter.to_top = true;
        }
    });
    ui.add_space(4.0);
}

/// A row's place: its whole width, its clickable part (all but `right` pixels kept for a
/// button), named and marked as selected or hovered.
fn row_area(ui: &mut Ui, id: Id, name: &str, selected: bool, right: f32) -> (Rect, Response) {
    row_sensing(ui, (id, Sense::click()), name, selected, right)
}

/// A row's place, as [`row_area`], sensing as `sense` says (a ref's is dragged too).
fn row_sensing(
    ui: &mut Ui,
    (id, sense): (Id, Sense),
    name: &str,
    selected: bool,
    right: f32,
) -> (Rect, Response) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let body = Rect::from_min_max(
        rect.min,
        pos2((rect.right() - right).max(rect.left()), rect.bottom()),
    );
    let response = ui.interact(body, id, sense);
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, name));
    crate::widgets::row_fill(
        ui,
        rect.shrink2(vec2(0.0, 1.0)),
        f32::from(RADIUS_INNER),
        selected,
        response.hovered(),
    );
    (rect, response)
}

/// Paints one line of `parts` from `x`, cut short at `right`: the first in `lead`'s weight
/// (what the row is: a name), the rest regular.
fn paint_line(
    ui: &Ui,
    drawing: &Drawing<'_>,
    (x, right): (f32, f32),
    y: f32,
    parts: &[(&str, Color32)],
    lead: Weight,
) {
    let mut job = LayoutJob::default();
    for (at, (text, color)) in parts.iter().enumerate() {
        let font = if at == 0 {
            FontId::new(drawing.font.size, lead.family())
        } else {
            drawing.font.clone()
        };
        crate::fonts::note_text(ui, &font, text);
        job.append(text, 0.0, TextFormat::simple(font, *color));
    }
    job.wrap = TextWrapping::truncate_at_width((right - x).max(0.0));
    let galley = ui.painter().layout_job(job);
    let pos = pos2(x, y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, drawing.palette.text);
}

/// Where a row's icon starts, `depth` levels in (a section's own icon at depth 0's).
fn icon_x(rect: Rect, depth: u8) -> f32 {
    rect.left() + PAD + CARET + f32::from(depth) * INDENT
}

/// Paints a row `depth` levels in: the guides of the levels around it, its `glyph` in its
/// column, and `parts` from a level further in (the first in `lead`'s weight), cut short
/// `right` pixels from the row's end.
fn paint_row(
    ui: &Ui,
    drawing: &Drawing<'_>,
    (rect, right): (Rect, f32),
    depth: u8,
    (glyph, color): (&str, Color32),
    parts: &[(&str, Color32)],
    lead: Weight,
) {
    let painter = ui.painter();
    // A guide down from each level's icon: what the row is inside of.
    for level in 0..depth {
        let x = (icon_x(rect, level) + ICON / 2.0).round() + 0.5;
        painter.vline(
            x,
            rect.y_range(),
            egui::Stroke::new(1.0, drawing.palette.border),
        );
    }
    let x = icon_x(rect, depth);
    painter.text(
        pos2(x + ICON / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        glyph,
        drawing.font.clone(),
        color,
    );
    paint_line(
        ui,
        drawing,
        (x + INDENT, rect.right() - right - 6.0),
        rect.center().y,
        parts,
        lead,
    );
}

/// Paints a fold's caret, hanging left of the icon of a row `depth` levels in.
fn paint_caret(ui: &Ui, drawing: &Drawing<'_>, rect: Rect, depth: u8, folded: bool) {
    let caret = if folded {
        icon::CARET_RIGHT
    } else {
        icon::CARET_DOWN
    };
    ui.painter().text(
        pos2(icon_x(rect, depth) - CARET / 2.0 - 1.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        caret,
        FontId::proportional(SMALL),
        drawing.palette.muted,
    );
}

/// Draws one row; returns what it asks.
fn draw_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    row: &Row,
) -> Option<SidebarEvent> {
    match row {
        Row::Section { section, count } => section_row(ui, drawing, view, *section, *count),
        Row::Folder {
            section,
            key,
            depth,
            name,
        } => {
            fold_row(ui, drawing, view, (*section, key), *depth, *name);
            None
        }
        Row::Remote { remote } => remote_row(ui, drawing, view, *remote),
        Row::Ref { index, depth, leaf } => ref_row(ui, drawing, view, *index, *depth, *leaf),
        Row::Stash(at) => stash_row(ui, drawing, view, *at),
        Row::Worktree(at) => worktree_row(ui, drawing, view, *at),
        Row::Submodule(at) => submodule_row(ui, drawing, view, *at),
        Row::Request(at) => request_row(ui, drawing, view, *at),
        Row::Note { text, warn } => {
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            let color = if *warn {
                drawing.palette.warn
            } else {
                drawing.palette.muted
            };
            let label = egui::Label::new(RichText::new(text.as_str()).color(color))
                .truncate()
                .selectable(false);
            let inner = Rect::from_min_max(pos2(icon_x(rect, 0), rect.top()), rect.max);
            let builder = egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center));
            ui.scope_builder(builder, |ui| ui.add(label));
            None
        }
    }
}

/// A section's header: its title and count (a click folds it), its menu, and a button adding
/// to it.
fn section_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    section: Section,
    count: usize,
) -> Option<SidebarEvent> {
    let title = section.title(drawing.shared.forge);
    let adding = match section {
        Section::Remotes => Some("Add a remote"),
        Section::Stashes => Some("Stash the changes"),
        Section::Worktrees => Some("Create a worktree"),
        _ => None,
    };
    let right = if adding.is_some() { EYE } else { 0.0 };
    let key = (section, BString::default());
    let id = Id::new(("sidebar-section", drawing.shared.id, section));
    let named = format!("{title}: {count}");
    let (rect, response) = row_area(ui, id, &named, false, right);
    let folded = view.folded.contains(&key);
    paint_section(ui, drawing, rect, (section, &title, count), folded);
    if response.clicked() {
        toggle(view, key);
    }
    let mut event = None;
    response.context_menu(|ui| event = section_menu(ui, drawing, section));
    if let Some(name) = adding {
        // In the column of the branches' eyes below.
        let button = Rect::from_min_max(pos2(rect.right() - EYE, rect.top()), rect.max);
        let enabled = adding_allowed(drawing, section).is_ok();
        let plus =
            crate::widgets::icon_button(ui, button, id.with("add"), icon::PLUS, name, enabled);
        if plus.on_hover_text(name).clicked() && enabled {
            event = adding_ask(drawing, section).map(SidebarEvent::Ask);
        }
    }
    event
}

/// Paints a section's header: a band, its caret and icon, its title in small capitals, and how
/// many it holds in a pill.
fn paint_section(
    ui: &Ui,
    drawing: &Drawing<'_>,
    rect: Rect,
    (section, title, count): (Section, &str, usize),
    folded: bool,
) {
    let palette = drawing.palette;
    let painter = ui.painter();
    // A band of its own, the section's rows below it; a little lighter under the pointer.
    let (band, pill) = if ui.rect_contains_pointer(rect) {
        (palette.surface_alt, palette.surface)
    } else {
        (palette.surface, palette.surface_alt)
    };
    painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), RADIUS_INNER, band);
    paint_caret(ui, drawing, rect, 0, folded);
    let x = icon_x(rect, 0);
    painter.text(
        pos2(x + ICON / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        section.icon(),
        drawing.font.clone(),
        palette.muted,
    );
    let galleys = crate::widgets::section_galleys(painter, title, Some(count), &palette);
    crate::widgets::paint_section(
        painter,
        pos2(x + INDENT, rect.center().y),
        galleys,
        (&palette, pill),
    );
}

/// Folds or unfolds a section, folder, or remote.
fn toggle(view: &mut Sidebar, key: (Section, BString)) {
    if !view.folded.remove(&key) {
        view.folded.insert(key);
    }
    view.stale = true;
}

/// Whether something can be added to `section` now, and why not.
fn adding_allowed(drawing: &Drawing<'_>, section: Section) -> Result<(), &'static str> {
    drawing.menus.idle()?;
    match section {
        Section::Stashes
            if drawing
                .shared
                .work_status
                .is_none_or(|status| status.is_empty()) =>
        {
            Err("There are no changes to stash")
        }
        Section::Worktrees if drawing.shared.head_commit().is_none() => {
            Err("There is no commit to check out yet")
        }
        _ => Ok(()),
    }
}

/// What adding to `section` asks.
fn adding_ask(drawing: &Drawing<'_>, section: Section) -> Option<Ask> {
    match section {
        Section::Remotes => {
            let first = drawing
                .lists
                .config
                .is_none_or(|config| remote_names(config).is_empty());
            Some(Ask::Dialog(Dialog::AddRemote {
                name: if first {
                    "origin".to_owned()
                } else {
                    String::new()
                },
                url: String::new(),
                fetch: true,
            }))
        }
        Section::Stashes => Some(Ask::Dialog(Dialog::Stash {
            message: String::new(),
            untracked: true,
        })),
        Section::Worktrees => {
            let head = drawing.shared.head_commit()?;
            Some(Ask::Dialog(Dialog::AddWorktree {
                path: drawing
                    .menus
                    .worktree_path(&head.abbreviated(ID_DIGITS).to_string()),
                from: WorktreeAsked::Commit(head),
                name: String::new(),
                open: true,
            }))
        }
        _ => None,
    }
}

/// A section header's menu.
fn section_menu(ui: &mut Ui, drawing: &Drawing<'_>, section: Section) -> Option<SidebarEvent> {
    let mut event = None;
    let mut ask = |asked| event = Some(SidebarEvent::Ask(asked));
    let idle = drawing.menus.idle();
    match section {
        Section::Branches if !drawing.shared.scope.hidden.is_empty() => {
            if actions::item(ui, (icon::EYE, "Show every branch in the graph"), Ok(())) {
                ask(Ask::Scope(Scope {
                    hidden: Vec::new(),
                    ..drawing.shared.scope.clone()
                }));
            }
        }
        Section::Remotes => {
            if actions::item(ui, (icon::PLUS, "Add a remote…"), idle) {
                ask(adding_ask(drawing, section)?);
            }
            if actions::item(ui, (icon::ARROWS_CLOCKWISE, "Fetch every remote"), idle) {
                ask(Ask::Task(Task::Fetch));
            }
        }
        Section::Stashes => {
            if actions::item(
                ui,
                (icon::ARCHIVE, "Stash the changes…"),
                adding_allowed(drawing, section),
            ) {
                ask(adding_ask(drawing, section)?);
            }
        }
        Section::Worktrees => {
            if actions::item(
                ui,
                (icon::TREE_STRUCTURE, "Create a worktree…"),
                adding_allowed(drawing, section),
            ) {
                ask(adding_ask(drawing, section)?);
            }
            if actions::item(
                ui,
                (icon::BROOM, "Forget those whose folders are gone"),
                idle,
            ) {
                ask(Ask::Task(Task::PruneWorktrees));
            }
        }
        Section::Submodules => {
            if actions::item(ui, (icon::FIRST_AID_KIT, "Mend everything"), idle) {
                ask(Ask::Task(Task::Submodules(
                    crate::submodules::SubmoduleTask::Mend,
                )));
            }
            if actions::item(ui, (icon::ARROWS_CLOCKWISE, "Update every submodule"), idle) {
                ask(Ask::Task(Task::UpdateSubmodules(Vec::new())));
            }
            if actions::item(ui, (icon::PACKAGE, "Each one, and adding one…"), Ok(())) {
                event = Some(SidebarEvent::Submodules);
            }
        }
        _ => {
            ui.label(muted(ui, "Click the title to fold it"));
        }
    }
    event
}

/// A folder of refs: a click folds it.
fn fold_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    (section, key): (Section, &BString),
    depth: u8,
    name: usize,
) {
    let shown = key.get(name..).unwrap_or_default().to_str_lossy();
    let id = Id::new(("sidebar-folder", drawing.shared.id, section, key.as_slice()));
    let (rect, response) = row_area(ui, id, &format!("{shown}/"), false, 0.0);
    let folded = view.folded.contains(&(section, key.clone()));
    let palette = drawing.palette;
    paint_caret(ui, drawing, rect, depth, folded);
    let glyph = if folded {
        icon::FOLDER_SIMPLE
    } else {
        icon::FOLDER_OPEN
    };
    paint_row(
        ui,
        drawing,
        (rect, 0.0),
        depth,
        (glyph, palette.muted),
        &[(&shown, palette.text)],
        Weight::Medium,
    );
    if response.clicked() {
        toggle(view, (section, key.clone()));
    }
}

/// A remote: a click folds its branches; its menu fetches, re-points, renames, or removes it.
fn remote_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    remote: usize,
) -> Option<SidebarEvent> {
    let name = drawing.remotes.get(remote)?.clone();
    let shown = name.to_str_lossy().into_owned();
    let id = Id::new(("sidebar-remote", drawing.shared.id, name.as_slice()));
    let (rect, response) = row_area(ui, id, &shown, false, 0.0);
    let key = (Section::Remotes, name.clone());
    paint_caret(ui, drawing, rect, 0, view.folded.contains(&key));
    let url = drawing.lists.config.and_then(|config| {
        config
            .get(&format!("remote.{shown}.url"))
            .map(|entry| entry.text().to_string())
    });
    let palette = drawing.palette;
    paint_row(
        ui,
        drawing,
        (rect, 0.0),
        0,
        (icon::CLOUD, palette.muted),
        &[
            (&shown, palette.text),
            ("  ", palette.muted),
            (url.as_deref().unwrap_or(""), palette.muted),
        ],
        Weight::SemiBold,
    );
    let response = match &url {
        Some(url) => response.on_hover_text(url.as_str()),
        None => response,
    };
    if response.clicked() {
        toggle(view, key);
    }
    let mut event = None;
    response.context_menu(|ui| event = remote_menu(ui, drawing, &name, url.as_deref()));
    event
}

/// A remote's menu.
fn remote_menu(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    name: &BString,
    url: Option<&str>,
) -> Option<SidebarEvent> {
    let mut event = None;
    let mut ask = |asked| event = Some(SidebarEvent::Ask(asked));
    let shown = name.to_str_lossy();
    let idle = drawing.menus.idle();
    let configured = url.is_some();
    let unconfigured = if configured {
        idle
    } else {
        Err("It isn't configured any more: only its branches are left")
    };
    for (label, prune) in [
        (format!("Fetch {shown}"), false),
        (format!("Fetch {shown}, pruning branches gone there"), true),
    ] {
        if actions::item(ui, (icon::ARROWS_CLOCKWISE, &label), unconfigured) {
            ask(Ask::Task(Task::FetchRemote {
                remote: name.clone(),
                prune,
            }));
        }
    }
    ui.separator();
    if actions::item(
        ui,
        (icon::PENCIL_SIMPLE, "Change its address…"),
        unconfigured,
    ) {
        let push = drawing
            .lists
            .config
            .and_then(|config| config.get(&format!("remote.{shown}.pushurl")))
            .map(|entry| entry.text().to_string())
            .unwrap_or_default();
        ask(Ask::Dialog(Dialog::EditRemote {
            remote: name.clone(),
            url: url.unwrap_or_default().to_owned(),
            push,
        }));
    }
    if actions::item(ui, (icon::PENCIL_SIMPLE, "Rename…"), unconfigured) {
        ask(Ask::Dialog(Dialog::RenameRemote {
            old: name.clone(),
            name: shown.clone().into_owned(),
        }));
    }
    if actions::danger_item(ui, (icon::TRASH, "Remove…"), unconfigured) {
        ask(Ask::Dialog(Dialog::RemoveRemote(name.clone())));
    }
    ui.separator();
    if let Some(url) = url
        && actions::item(ui, (icon::COPY, "Copy its address"), Ok(()))
    {
        ui.ctx().copy_text(url.to_owned());
    }
    if let Some(repository) = drawing.shared.forge.repository_of(name.as_bstr())
        && actions::item(
            ui,
            (
                icon::ARROW_SQUARE_OUT,
                &format!("Open on {}", repository.host().kind().name()),
            ),
            Ok(()),
        )
    {
        ask(Ask::Open(repository.web_url()));
    }
    event
}

/// A branch's eye at its row's right: open while the graph shows the branch, struck through
/// while it is hidden; a click hides or shows it. Drawn here so it keeps to its place
/// (egui's own buttons grow to their interaction size, over the rows around).
fn eye(ui: &Ui, drawing: &Drawing<'_>, rect: Rect, id: Id, seen: bool) -> Response {
    let place = Rect::from_min_max(pos2(rect.right() - EYE, rect.top()), rect.max);
    let response = ui.interact(place, id, Sense::click());
    let palette = drawing.palette;
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(place.shrink2(vec2(0.0, 2.0)), RADIUS_INNER, palette.border);
    }
    let (glyph, color) = match (seen, hovered) {
        (true, false) => (icon::EYE, palette.muted),
        (true, true) => (icon::EYE, palette.text),
        (false, _) => (icon::EYE_SLASH, palette.text),
    };
    ui.painter().text(
        place.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        drawing.font.clone(),
        color,
    );
    response
}

/// Whether the graph shows `reference`, as asked (the current branch always shows).
fn visible(shared: &Shared<'_>, reference: &Reference) -> bool {
    let scope = &shared.scope;
    if scope.only.is_empty() {
        !scope.hides(reference.name.as_bstr())
    } else {
        scope.only.contains(&reference.name)
    }
}

/// The scope with `reference` shown or hidden as asked.
fn shown_as(scope: &Scope, reference: &Reference, show: bool) -> Scope {
    if scope.only.is_empty() {
        return scope.with_hidden(reference.name.as_bstr(), !show);
    }
    let mut scope = scope.clone();
    if show {
        if !scope.only.contains(&reference.name) {
            scope.only.push(reference.name.clone());
        }
    } else {
        scope.only.retain(|name| *name != reference.name);
        if scope.only.is_empty() {
            // Nothing alone: everything but it.
            scope.hidden = vec![reference.name.clone()];
        }
    }
    scope
}

/// A branch, remote branch, or tag: its name, and a branch's eye; a click shows its commit, a
/// double click switches to a branch, its menu is its badge's.
fn ref_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    index: usize,
    depth: u8,
    leaf: usize,
) -> Option<SidebarEvent> {
    let reference = drawing.refs.get(index)?;
    let short = reference.short_name();
    let shown = short.to_str_lossy();
    let key = Picked::Ref(reference.name.clone());
    let selected = view.selected.as_ref() == Some(&key);
    let id = Id::new(("sidebar-ref", drawing.shared.id, reference.name.as_slice()));
    let is_branch = matches!(reference.kind, RefKind::Branch | RefKind::RemoteBranch);
    let right = if is_branch { EYE } else { 0.0 };
    // Dragged onto another ref (here, or a badge in the graph) to merge or rebase.
    let sense = Sense::click_and_drag();
    let (rect, response) = row_sensing(ui, (id, sense), &shown, selected, right);
    let current = drawing.current.as_ref() == Some(&reference.name);
    let seen = !is_branch || current || visible(drawing.shared, reference);
    let mut event = None;
    // The branch checked out always shows: it has no eye.
    if is_branch && !current {
        let toggle = eye(ui, drawing, rect, id.with("shown"), seen);
        toggle.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Checkbox,
                true,
                seen,
                format!("Show {shown} in the graph"),
            )
        });
        let toggle = toggle.on_hover_text(if seen {
            "Shown in the graph: hide it"
        } else {
            "Hidden from the graph: show it"
        });
        if toggle.clicked() {
            let scope = shown_as(&drawing.shared.scope, reference, !seen);
            event = Some(SidebarEvent::Ask(Ask::Scope(scope)));
        }
    }
    let palette = drawing.palette;
    let (glyph, color) = match reference.kind {
        _ if current => (icon::CHECK_CIRCLE, palette.accent),
        RefKind::Branch => (icon::GIT_BRANCH, palette.accent),
        RefKind::RemoteBranch => (icon::GIT_BRANCH, palette.muted),
        RefKind::Tag => (icon::TAG, palette.warn),
        RefKind::Other => (icon::BOOKMARK_SIMPLE, palette.muted),
    };
    let name = short.get(leaf..).unwrap_or_default().to_str_lossy();
    let text = if !seen {
        palette.muted
    } else if current {
        palette.accent
    } else {
        palette.text
    };
    let (mark, weight) = if current {
        ("  checked out", Weight::SemiBold)
    } else {
        ("", Weight::Regular)
    };
    paint_row(
        ui,
        drawing,
        (rect, right),
        depth,
        (glyph, color),
        &[(&name, text), (mark, palette.muted)],
        weight,
    );
    if response.double_clicked() {
        if let Some(asked) = actions::ref_double_clicked(&drawing.menus, reference) {
            event = Some(SidebarEvent::Ask(asked));
        }
    } else if response.clicked() {
        view.selected = Some(key);
        event = Some(SidebarEvent::Show(reference.target));
    }
    let mut asked = None;
    let dragged = format!("{glyph} {shown}");
    super::history::dragging(ui, &response, reference, &dragged, &mut asked);
    response.context_menu(|ui| actions::ref_menu(ui, &drawing.menus, reference, &mut asked));
    asked.map(SidebarEvent::Ask).or(event)
}

/// A stash: a click shows its changes in the details; its menu applies, pops, or drops it.
fn stash_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    at: usize,
) -> Option<SidebarEvent> {
    let stash = drawing.lists.stashes?.ok()?.get(at)?;
    let place = format!("stash@{{{}}}", stash.index);
    let key = Picked::Stash(stash.id);
    let selected = view.selected.as_ref() == Some(&key);
    let id = Id::new(("sidebar-stash", drawing.shared.id, stash.id));
    let (rect, response) = row_area(
        ui,
        id,
        &format!("{place}: {}", stash.message),
        selected,
        0.0,
    );
    let palette = drawing.palette;
    paint_row(
        ui,
        drawing,
        (rect, 0.0),
        0,
        (icon::ARCHIVE, palette.muted),
        &[
            (&place, palette.muted),
            ("  ", palette.muted),
            (&stash.message, palette.text),
        ],
        Weight::Regular,
    );
    let clicked = response.clicked();
    if clicked {
        view.selected = Some(key);
    }
    let mut event = clicked.then_some(SidebarEvent::Show(stash.id));
    response.context_menu(|ui| {
        let idle = drawing.menus.idle();
        for (label, pop) in [("Apply", false), ("Pop", true)] {
            if actions::item(ui, (icon::TRAY_ARROW_UP, label), idle) {
                event = Some(SidebarEvent::Ask(Ask::Task(Task::ApplyStash {
                    stash: stash.clone(),
                    pop,
                })));
            }
        }
        if actions::danger_item(ui, (icon::TRASH, "Drop…"), idle) {
            event = Some(SidebarEvent::Ask(Ask::Dialog(Dialog::DropStash(
                stash.clone(),
            ))));
        }
    });
    event
}

/// A worktree: its branch (or commit) and folder; a click shows its commit, a double click
/// opens it in a tab; its menu locks, unlocks, or removes it.
fn worktree_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    at: usize,
) -> Option<SidebarEvent> {
    let worktree = drawing.lists.worktrees?.ok()?.get(at)?;
    let here = worktree.path == drawing.shared.repository.work_tree();
    let checked_out = match (&worktree.branch, worktree.head) {
        (Some(branch), _) => branch
            .strip_prefix(b"refs/heads/")
            .unwrap_or(branch)
            .to_str_lossy()
            .into_owned(),
        (None, Some(head)) => format!("{} (no branch)", head.abbreviated(ID_DIGITS)),
        (None, None) => "bare".to_owned(),
    };
    let folder = display_path(&worktree.path, drawing.shared.home);
    let key = Picked::Worktree(worktree.path.clone());
    let selected = view.selected.as_ref() == Some(&key);
    let id = Id::new(("sidebar-worktree", drawing.shared.id, &worktree.path));
    let named = format!("{checked_out} · {folder}");
    let (rect, response) = row_area(ui, id, &named, selected, 0.0);
    let palette = drawing.palette;
    let glyph = if worktree.main {
        icon::HOUSE
    } else {
        icon::TREE_STRUCTURE
    };
    let mut state = String::new();
    if here {
        state.push_str("  (here)");
    }
    if worktree.locked.is_some() {
        state.push_str("  ");
        state.push_str(icon::LOCK_SIMPLE);
    }
    let gone = if worktree.prunable.is_some() {
        "  folder missing"
    } else {
        ""
    };
    paint_row(
        ui,
        drawing,
        (rect, 0.0),
        0,
        (glyph, palette.muted),
        &[
            (
                &checked_out,
                if here { palette.accent } else { palette.text },
            ),
            (&state, palette.muted),
            (gone, palette.warn),
            ("  ", palette.muted),
            (&folder, palette.muted),
        ],
        Weight::Regular,
    );
    let mut hover = folder;
    if let Some(reason) = worktree.locked.as_ref().filter(|reason| !reason.is_empty()) {
        let _ = write!(hover, "\nLocked: {}", reason.to_str_lossy());
    }
    if let Some(reason) = &worktree.prunable {
        let _ = write!(hover, "\nPrunable: {}", reason.to_str_lossy());
    }
    let response = response.on_hover_text(hover);
    let mut event = None;
    if response.double_clicked() && !here {
        event = Some(SidebarEvent::Open(worktree.path.clone()));
    } else if response.clicked() {
        view.selected = Some(key);
        event = worktree.head.map(SidebarEvent::Show);
    }
    response.context_menu(|ui| {
        if let Some(asked) = worktree_menu(ui, drawing, worktree, here) {
            event = Some(asked);
        }
    });
    event
}

/// A worktree's menu.
fn worktree_menu(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    worktree: &Worktree,
    here: bool,
) -> Option<SidebarEvent> {
    let mut event = None;
    let idle = drawing.menus.idle();
    let opening = if here {
        Err("It is this tab's")
    } else if worktree.prunable.is_some() {
        Err("Its folder is missing")
    } else {
        Ok(())
    };
    if actions::item(ui, (icon::APP_WINDOW, "Open in a tab"), opening) {
        event = Some(SidebarEvent::Open(worktree.path.clone()));
    }
    if actions::item(ui, (icon::COPY, "Copy its path"), Ok(())) {
        ui.ctx()
            .copy_text(worktree.path.to_string_lossy().into_owned());
    }
    ui.separator();
    let other = if worktree.main {
        Err("It is the repository's main worktree")
    } else {
        idle
    };
    let path = worktree.path.clone();
    if worktree.locked.is_some() {
        if actions::item(ui, (icon::LOCK_SIMPLE_OPEN, "Unlock"), other) {
            event = Some(SidebarEvent::Ask(Ask::Task(Task::UnlockWorktree(path))));
        }
    } else if actions::item(ui, (icon::LOCK_SIMPLE, "Lock…"), other) {
        event = Some(SidebarEvent::Ask(Ask::Dialog(Dialog::LockWorktree {
            path,
            reason: String::new(),
        })));
    }
    let removing = if here {
        Err("It is this tab's")
    } else if worktree.locked.is_some() {
        Err("It is locked: unlock it first")
    } else {
        other
    };
    if actions::danger_item(ui, (icon::TRASH, "Remove…"), removing) {
        event = Some(SidebarEvent::Ask(Ask::Dialog(Dialog::RemoveWorktree {
            path: worktree.path.clone(),
            changes: false,
        })));
    }
    event
}

/// A submodule: its path, and whether it is set up here; a double click opens it in a tab;
/// its menu updates it.
fn submodule_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    at: usize,
) -> Option<SidebarEvent> {
    let submodule = drawing.lists.submodules?.ok()?.get(at)?;
    let path = submodule.path.to_str_lossy();
    let key = Picked::Submodule(submodule.path.clone());
    let selected = view.selected.as_ref() == Some(&key);
    let id = Id::new((
        "sidebar-submodule",
        drawing.shared.id,
        submodule.path.as_slice(),
    ));
    let (rect, response) = row_area(ui, id, &path, selected, 0.0);
    let palette = drawing.palette;
    let recorded = submodule.recorded.abbreviated(ID_DIGITS).to_string();
    let standing = drawing
        .lists
        .submodule_states
        .and_then(Result::ok)
        .and_then(|standing| {
            standing
                .states
                .iter()
                .find(|state| state.path == submodule.path)
        });
    let state = match standing.map(|state| state.checkout) {
        Some(egit_git::Checkout::Elsewhere) => "at another commit",
        Some(egit_git::Checkout::Conflict) => "in conflict",
        Some(egit_git::Checkout::NotInitialized) => "not set up",
        _ if !submodule.initialized => "not set up",
        _ => recorded.as_str(),
    };
    paint_row(
        ui,
        drawing,
        (rect, 0.0),
        0,
        (icon::PACKAGE, palette.muted),
        &[
            (&path, palette.text),
            ("  ", palette.muted),
            (state, palette.muted),
        ],
        Weight::Regular,
    );
    let folder = drawing
        .shared
        .repository
        .work_tree()
        .join(submodule.path.to_path_lossy());
    let response = response.on_hover_text(
        submodule
            .url
            .as_ref()
            .map_or_else(String::new, |url| format!("From {}", url.to_str_lossy())),
    );
    let mut event = None;
    if response.double_clicked() && submodule.initialized {
        event = Some(SidebarEvent::Open(folder.clone()));
    } else if response.clicked() {
        view.selected = Some(key);
    }
    response.context_menu(|ui| {
        let idle = drawing.menus.idle();
        let update = if submodule.initialized {
            format!("Update {path}")
        } else {
            format!("Set up {path}")
        };
        if actions::item(ui, (icon::ARROWS_CLOCKWISE, &update), idle) {
            event = Some(SidebarEvent::Ask(Ask::Task(Task::UpdateSubmodules(vec![
                submodule.path.clone(),
            ]))));
        }
        let opening = if submodule.initialized {
            Ok(())
        } else {
            Err("Set it up first")
        };
        if actions::item(ui, (icon::APP_WINDOW, "Open in a tab"), opening) {
            event = Some(SidebarEvent::Open(folder.clone()));
        }
        if actions::item(ui, (icon::COPY, "Copy its path"), Ok(())) {
            ui.ctx().copy_text(path.clone().into_owned());
        }
    });
    event
}

/// A pull (or merge) request: its checks, number, and title; a click shows it.
fn request_row(
    ui: &mut Ui,
    drawing: &Drawing<'_>,
    view: &mut Sidebar,
    at: usize,
) -> Option<SidebarEvent> {
    let request = view.requests.get(at)?.clone();
    let key = Picked::Request(request.repository, request.number);
    let selected = view.selected.as_ref() == Some(&key);
    let id = Id::new((
        "sidebar-request",
        drawing.shared.id,
        request.repository,
        request.number,
    ));
    let (rect, response) = row_area(ui, id, &request.label, selected, 0.0);
    let palette = drawing.palette;
    let (glyph, color, meaning) = check_look(request.checks, palette);
    paint_row(
        ui,
        drawing,
        (rect, 0.0),
        0,
        (glyph, color),
        &[(&request.label, palette.text)],
        Weight::Regular,
    );
    let response = response.on_hover_text(meaning);
    if response.clicked() {
        view.selected = Some(key);
        return Some(SidebarEvent::Ask(Ask::PullRequest(Some((
            request.repository,
            request.number,
        )))));
    }
    None
}

/// The narrow strip shown in the sidebar's place while it is hidden: a button showing it.
pub(super) fn hidden(ui: &mut Ui, view: &mut Sidebar) {
    ui.add_space(6.0);
    let (_, place) = ui.allocate_space(vec2(TARGET, TARGET));
    let show = crate::widgets::icon_button(
        ui,
        place,
        Id::new("sidebar-show"),
        icon::SIDEBAR_SIMPLE,
        "Show the sidebar",
        true,
    );
    if show.on_hover_text("Show the sidebar").clicked() {
        view.open = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sort_as_people_read_them() {
        let mut names = [
            "v2.10",
            "v2.9",
            "V2.1",
            "feature-x",
            "feature/b",
            "Feature/a",
            "v02.9",
            "main",
        ];
        names.sort_by(|a, b| by_parts(a.as_bytes(), b.as_bytes()));
        assert_eq!(
            names,
            [
                "Feature/a",
                "feature/b",
                "feature-x",
                "main",
                "V2.1",
                "v02.9",
                "v2.9",
                "v2.10"
            ]
        );
        assert_eq!(natural(b"a", b"a"), Ordering::Equal);
        assert_ne!(
            natural(b"A", b"a"),
            Ordering::Equal,
            "only equal names are equal"
        );
    }

    fn names(rows: &[Row], refs: &[&str]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Folder {
                    key, depth, name, ..
                } => {
                    format!(
                        "{depth} {}/",
                        key.get(*name..).unwrap_or_default().to_str_lossy()
                    )
                }
                Row::Ref { index, depth, leaf } => {
                    let name = refs.get(*index).copied().unwrap_or_default();
                    format!("{depth} {}", name.get(*leaf..).unwrap_or_default())
                }
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn refs_fold_into_folders_by_their_slashes() {
        let refs = [
            "a",
            "feature/ui/x",
            "feature/ui/y",
            "feature/z",
            "fix/1",
            "main",
        ];
        let items = || {
            refs.iter()
                .enumerate()
                .map(|(at, name)| (at, name.as_bytes(), 0))
        };
        let open = |_: &[u8]| false;
        let tree = Tree {
            section: Section::Branches,
            base: 0,
            prefix: b"",
            folded: &open,
        };
        let mut rows = Vec::new();
        tree.rows(&mut rows, items());
        assert_eq!(
            names(&rows, &refs),
            [
                "0 a",
                "0 feature/",
                "1 ui/",
                "2 x",
                "2 y",
                "1 z",
                "0 fix/",
                "1 1",
                "0 main"
            ]
        );

        // A folded folder hides what is in it, folders too, and nothing else.
        let shut = |key: &[u8]| key == b"feature";
        let tree = Tree {
            folded: &shut,
            ..tree
        };
        rows.clear();
        tree.rows(&mut rows, items());
        assert_eq!(
            names(&rows, &refs),
            ["0 a", "0 feature/", "0 fix/", "1 1", "0 main"]
        );
        let shut = |key: &[u8]| key == b"feature/ui";
        let tree = Tree {
            folded: &shut,
            ..tree
        };
        rows.clear();
        tree.rows(&mut rows, items());
        assert_eq!(
            names(&rows, &refs),
            [
                "0 a",
                "0 feature/",
                "1 ui/",
                "1 z",
                "0 fix/",
                "1 1",
                "0 main"
            ]
        );

        // A remote's branches: keyed with the remote, one level in.
        let refs = ["origin/main", "origin/topic/a"];
        let items = refs
            .iter()
            .enumerate()
            .map(|(at, name)| (at, name.as_bytes().get(7..).unwrap_or_default(), 7));
        let tree = Tree {
            section: Section::Remotes,
            base: 1,
            prefix: b"origin/",
            folded: &open,
        };
        rows.clear();
        tree.rows(&mut rows, items);
        assert_eq!(names(&rows, &refs), ["1 main", "1 topic/", "2 a"]);
        let Some(Row::Folder { key, .. }) = rows.get(1) else {
            unreachable!()
        };
        assert_eq!(key, "origin/topic");
    }
}
