//! A repository's history as the commit table shows it: every commit's row and lanes, streamed
//! in by a walk in the background, the refs pointing into it, and commits' summaries, read only
//! for the rows on screen.
//!
//! A history costs its commits' names (packed: 20 bytes each for SHA-1) and their rows of lanes
//! (`egit-graph`, about 14 bytes each): 51 MB for Linux's 1.5M commits. Everything else is
//! bounded by what is on screen.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, Commit, CountedFile, FileChange, Git, GitError, LineCounts, Object, ObjectFormat,
    ObjectId, ObjectKind, ObjectReader, RefKind, Reference, Repository, SignatureCheck, TreeEntry,
    Verdict, check_signatures, commit_changes, line_counts, list_references, walk_history,
    walk_history_from,
};
use egit_gix::GixRepository;
use egit_graph::{CHUNK_ROWS, Chunk, GraphBuilder};
use egit_text::Text;
use egui_async::Bind;
use parking_lot::Mutex;

use crate::format::date_time;
use crate::grid::Plain;
use crate::jobs;
use crate::problem::Problem;

/// The most summaries kept beyond those just read; those used longest ago are forgotten first.
const KEPT_SUMMARIES: usize = 4096;

/// The most commits one read of summaries asks for.
const READ_AT_ONCE: usize = 512;

/// The largest file shown, in bytes.
const MAX_SHOWN: usize = 256 << 20;

/// How much of a file git looks through for a NUL to call it binary.
const BINARY_PROBE: usize = 8_000;

/// How a Git LFS pointer file starts.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1\n";

/// A history: every commit reachable from a ref, children first, as rows.
#[derive(Debug)]
pub(crate) struct History {
    /// The rows' lanes, [`CHUNK_ROWS`] rows a chunk.
    graph: Vec<Chunk>,
    /// The rows' commits' names.
    names: Names,
    rows: u64,
    widest: usize,
    refs: Refs,
    /// Whether the repository has a commit-graph file, once the walk has looked.
    commit_graph: Option<bool>,
    /// The walk, while it runs.
    loading: Option<Loading>,
    problem: Option<Problem>,
}

/// A walk running in the background.
#[derive(Debug)]
struct Loading {
    pieces: Receiver<Piece>,
    cancel: CancelToken,
}

impl Drop for Loading {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What a walk hands over as it goes.
#[derive(Debug)]
enum Piece {
    /// The refs and whether there is a commit-graph file, before any row.
    Start { refs: Refs, commit_graph: bool },
    /// The next rows: their lanes and their commits' names.
    Rows {
        graph: Chunk,
        ids: Arc<[u8]>,
        widest: usize,
    },
    /// The walk's end.
    Done(Result<(), Problem>),
}

impl History {
    /// Starts reading `repository`'s history; rows arrive as [`Self::poll`] takes them in.
    pub(crate) fn load(
        git: &Git,
        repository: &Arc<Repository>,
        objects: &Objects,
        ctx: &egui::Context,
    ) -> Self {
        let (sender, pieces) = mpsc::channel();
        let cancel = CancelToken::new();
        let job = (
            git.clone(),
            Arc::clone(repository),
            objects.clone(),
            cancel.clone(),
            ctx.clone(),
        );
        jobs::detached(move || {
            let (git, repository, objects, cancel, ctx) = job;
            let walked = walk(&git, &repository, &objects, &cancel, &sender, &ctx);
            // When nobody listens any more, there is no one to tell.
            let _ = sender.send(Piece::Done(walked));
            ctx.request_repaint();
        });
        Self {
            graph: Vec::new(),
            names: Names::new(repository.object_format()),
            rows: 0,
            widest: 0,
            refs: Refs::default(),
            commit_graph: None,
            loading: Some(Loading { pieces, cancel }),
            problem: None,
        }
    }

    /// Takes in what the walk found since; returns whether it just ended.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(loading) = &self.loading else {
            return false;
        };
        loop {
            match loading.pieces.try_recv() {
                Ok(Piece::Start { refs, commit_graph }) => {
                    self.refs = refs;
                    self.commit_graph = Some(commit_graph);
                }
                Ok(Piece::Rows { graph, ids, widest }) => {
                    let rows = u64::try_from(graph.len()).unwrap_or(0);
                    self.rows = self.rows.saturating_add(rows);
                    self.graph.push(graph);
                    self.names.chunks.push(ids);
                    self.widest = widest;
                }
                Ok(Piece::Done(walked)) => {
                    self.problem = walked.err();
                    self.loading = None;
                    return true;
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.problem = Some(Problem::interrupted());
                    self.loading = None;
                    return true;
                }
            }
        }
    }

    /// How many rows (commits) it has so far.
    pub(crate) const fn rows(&self) -> u64 {
        self.rows
    }

    /// The most lanes a row has had open so far.
    pub(crate) const fn widest(&self) -> usize {
        self.widest
    }

    /// The rows' lanes.
    pub(crate) fn graph(&self) -> &[Chunk] {
        &self.graph
    }

    /// The refs pointing into it.
    pub(crate) const fn refs(&self) -> &Refs {
        &self.refs
    }

    /// Whether the repository has a commit-graph file (`None` until the walk has looked).
    pub(crate) const fn commit_graph(&self) -> Option<bool> {
        self.commit_graph
    }

    /// Notes that a commit-graph file was written.
    pub(crate) const fn wrote_commit_graph(&mut self) {
        self.commit_graph = Some(true);
    }

    /// Whether the walk is still running.
    pub(crate) const fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Why the walk failed, if it did.
    pub(crate) const fn problem(&self) -> Option<&Problem> {
        self.problem.as_ref()
    }

    /// The commit in `row`.
    pub(crate) fn id(&self, row: u64) -> Option<ObjectId> {
        self.names.id(row)
    }

    /// The row of commit `id`, looking from `near` on first (where it most likely is), then
    /// before. A scan of the names: Linux's 30 MB take about 10 ms.
    pub(crate) fn row_of(&self, id: &ObjectId, near: u64) -> Option<u64> {
        self.names.row_of(id, near)
    }

    /// Its rows' commits' names so far, for a job to read (shared, not copied).
    pub(crate) fn names(&self) -> Names {
        self.names.clone()
    }
}

/// Every row's commit's name, packed a chunk ([`CHUNK_ROWS`] rows) at a time. A clone shares
/// the chunks, so a job can read them while the history goes on.
#[derive(Clone, Debug)]
pub(crate) struct Names {
    format: ObjectFormat,
    /// Each chunk's names: `format.byte_len()` bytes a row.
    chunks: Vec<Arc<[u8]>>,
}

impl Names {
    const fn new(format: ObjectFormat) -> Self {
        Self {
            format,
            chunks: Vec::new(),
        }
    }

    /// How many rows it names.
    pub(crate) fn rows(&self) -> u64 {
        let width = self.format.byte_len();
        let packed: usize = self.chunks.iter().map(|chunk| chunk.len()).sum();
        u64::try_from(packed.checked_div(width).unwrap_or(0)).unwrap_or(0)
    }

    /// The name of the commit in `row`, as bytes.
    pub(crate) fn bytes(&self, row: u64) -> Option<&[u8]> {
        let (chunk, index) = locate(row)?;
        let width = self.format.byte_len();
        let start = index.checked_mul(width)?;
        self.chunks
            .get(chunk)?
            .get(start..start.checked_add(width)?)
    }

    /// The commit in `row`.
    pub(crate) fn id(&self, row: u64) -> Option<ObjectId> {
        ObjectId::from_bytes(self.bytes(row)?)
    }

    /// The row of commit `id`, looking from `near` on first, then before (a scan).
    pub(crate) fn row_of(&self, id: &ObjectId, near: u64) -> Option<u64> {
        let width = self.format.byte_len();
        let first = locate(near).map_or(0, |(chunk, _)| chunk.min(self.chunks.len()));
        let chunks = (first..self.chunks.len()).chain(0..first);
        for chunk in chunks {
            let found = self.chunks.get(chunk).and_then(|ids| {
                ids.chunks_exact(width)
                    .position(|bytes| bytes == id.as_bytes())
            });
            if let Some(index) = found {
                let row = chunk.checked_mul(CHUNK_ROWS)?.checked_add(index)?;
                return u64::try_from(row).ok();
            }
        }
        None
    }

    /// The names chunk by chunk, each with its first row.
    pub(crate) fn chunks(&self) -> impl Iterator<Item = (u64, &[u8])> {
        let per_chunk = u64::try_from(CHUNK_ROWS).unwrap_or(u64::MAX);
        (0_u64..)
            .zip(&self.chunks)
            .map(move |(chunk, names)| (chunk.saturating_mul(per_chunk), &**names))
    }

    /// The bytes one name takes.
    pub(crate) const fn width(&self) -> usize {
        self.format.byte_len()
    }
}

#[cfg(test)]
impl Names {
    /// `rows` made-up SHA-1 names, spread as real ones are, but every seventh starting with the
    /// same four bytes.
    pub(crate) fn made_up(rows: u32) -> Self {
        let mut names = Self::new(ObjectFormat::Sha1);
        let mut chunk = Vec::new();
        for row in 0..rows {
            // SplitMix64, a word at a time.
            let mut state = u64::from(row).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let mut name = [0_u8; 20];
            for slot in name.chunks_mut(8) {
                state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
                let mut mixed = (state ^ (state >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                mixed ^= mixed >> 31;
                slot.copy_from_slice(&mixed.to_le_bytes()[..slot.len()]);
            }
            if row.is_multiple_of(7) {
                name[..4].copy_from_slice(&[0xab, 0xcd, 0xef, 0x01]);
            }
            chunk.extend_from_slice(&name);
            if chunk.len() == CHUNK_ROWS * 20 {
                names.chunks.push(Arc::from(std::mem::take(&mut chunk)));
            }
        }
        if !chunk.is_empty() {
            names.chunks.push(Arc::from(chunk));
        }
        names
    }
}

/// The chunk holding `row`, and its index there.
fn locate(row: u64) -> Option<(usize, usize)> {
    let per_chunk = u64::try_from(CHUNK_ROWS).ok()?;
    Some((
        usize::try_from(row.checked_div(per_chunk)?).ok()?,
        usize::try_from(row.checked_rem(per_chunk)?).ok()?,
    ))
}

/// Walks the history, handing over the refs first, then the rows a chunk at a time. Its refs and
/// the commits it starts from come from gitoxide where it reads them as git does.
fn walk(
    git: &Git,
    repository: &Repository,
    objects: &Objects,
    cancel: &CancelToken,
    pieces: &Sender<Piece>,
    ctx: &egui::Context,
) -> Result<(), Problem> {
    let gix = objects.reopen(repository.work_tree());
    let read_by_gix = gix.as_ref().and_then(|gix| match gix.references() {
        Ok(refs) => Some(refs),
        Err(error) => {
            tracing::warn!(%error, "gitoxide couldn't list the refs; git lists them");
            None
        }
    });
    let refs = match read_by_gix {
        Some(refs) => refs,
        None => list_references(git, repository).map_err(|error| {
            Problem::git(
                "The repository's branches and tags couldn't be read",
                &error,
            )
        })?,
    };
    let start = Piece::Start {
        refs: Refs::new(refs),
        commit_graph: repository.has_commit_graph(),
    };
    if pieces.send(start).is_err() {
        return Ok(());
    }
    let capacity = CHUNK_ROWS.saturating_mul(repository.object_format().byte_len());
    let hand_over = |graph: Chunk, ids: Vec<u8>, widest: usize| {
        let ids = Arc::from(ids);
        let sent = pieces.send(Piece::Rows { graph, ids, widest }).is_ok();
        ctx.request_repaint();
        sent
    };
    let mut builder = GraphBuilder::new();
    let mut ids = Vec::with_capacity(capacity);
    let each = |id: ObjectId, parents: &[ObjectId]| {
        if let Some(graph) = builder.push(id, parents) {
            let full = std::mem::replace(&mut ids, Vec::with_capacity(capacity));
            if !hand_over(graph, full, builder.widest()) {
                return ControlFlow::Break(());
            }
        }
        ids.extend_from_slice(id.as_bytes());
        ControlFlow::Continue(())
    };
    let tips = gix.as_ref().and_then(|gix| gix.tips().ok());
    match tips {
        Some(tips) => walk_history_from(git, repository, tips, cancel, each),
        None => walk_history(git, repository, cancel, each),
    }
    .map_err(|error| Problem::git("The history couldn't be read", &error))?;
    let widest = builder.widest();
    if let Some(graph) = builder.finish() {
        hand_over(graph, ids, widest);
    }
    Ok(())
}

/// The refs pointing into a history, found by the commit they point at.
#[derive(Debug, Default)]
pub(crate) struct Refs(Vec<Reference>);

impl Refs {
    /// Sorts `references` by target, then branches before remote branches before tags, then by
    /// name.
    fn new(mut references: Vec<Reference>) -> Self {
        references.sort_unstable_by(|a, b| {
            a.target
                .cmp(&b.target)
                .then_with(|| rank(a.kind).cmp(&rank(b.kind)))
                .then_with(|| a.name.cmp(&b.name))
        });
        Self(references)
    }

    /// Whether there is a local branch called `name` (its short name).
    pub(crate) fn has_branch(&self, name: &BStr) -> bool {
        self.0
            .iter()
            .any(|reference| reference.kind == RefKind::Branch && reference.short_name() == name)
    }

    /// The refs pointing at `id`.
    pub(crate) fn at(&self, id: &ObjectId) -> &[Reference] {
        let start = self.0.partition_point(|reference| reference.target < *id);
        let rest = self.0.get(start..).unwrap_or_default();
        let len = rest.partition_point(|reference| reference.target == *id);
        rest.get(..len).unwrap_or_default()
    }
}

/// Where a kind of ref goes among those at one commit.
const fn rank(kind: RefKind) -> u8 {
    match kind {
        RefKind::Branch => 0,
        RefKind::RemoteBranch => 1,
        RefKind::Tag => 2,
        RefKind::Other => 3,
    }
}

/// What reads a repository's objects, shared by the jobs that read its commits and files:
/// gitoxide in process where it reads as git does (`GixRepository::open`; opened again with each
/// read of the history, so a change that leaves it to git is noticed), else one `git cat-file`,
/// started when first needed (one that failed is started again next time). `EGIT_GIX=0` leaves
/// every read to git.
#[derive(Clone, Debug, Default)]
pub(crate) struct Objects {
    git: Arc<Mutex<Option<ObjectReader>>>,
    gix: Arc<Mutex<Gix>>,
}

/// Whether gitoxide reads a repository's objects.
#[derive(Clone, Debug, Default)]
enum Gix {
    /// Not looked at yet.
    #[default]
    Unopened,
    Open(GixRepository),
    /// It can't (or mustn't): git reads them.
    Unavailable,
}

/// Whether gitoxide may read at all (`EGIT_GIX=0` says it mayn't).
pub(crate) fn gix_allowed() -> bool {
    static ALLOWED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALLOWED.get_or_init(|| std::env::var_os("EGIT_GIX").is_none_or(|value| value != "0"))
}

impl Objects {
    /// Opens the repository for gitoxide again (inside a job): what it reads, or `None` when
    /// git reads everything.
    pub(crate) fn reopen(&self, work_tree: &Path) -> Option<GixRepository> {
        let opened = gix_allowed()
            .then(|| GixRepository::open(work_tree))
            .flatten();
        *self.gix.lock() = opened.clone().map_or(Gix::Unavailable, Gix::Open);
        opened
    }

    /// gitoxide's reader, opened the first time it is asked for (inside a job).
    fn gix(&self, work_tree: &Path) -> Option<GixRepository> {
        let known = self.gix.lock().clone();
        match known {
            Gix::Open(gix) => Some(gix),
            Gix::Unavailable => None,
            Gix::Unopened => self.reopen(work_tree),
        }
    }

    /// Reads `ids` (see [`ObjectReader::read`]), inside a job: through gitoxide, or git; git
    /// reads what gitoxide didn't (a partial clone's objects it hasn't, which git fetches, or
    /// all after gitoxide failed partway).
    pub(crate) fn read(
        &self,
        git: &Git,
        work_tree: &Path,
        ids: &[ObjectId],
        mut each: impl FnMut(ObjectId, Option<Object<'_>>),
    ) -> Result<(), GitError> {
        let mut handed = 0_usize;
        if let Some(gix) = self.gix(work_tree) {
            let read = gix.read(ids, |id, object| {
                handed = handed.saturating_add(1);
                each(id, object);
            });
            match read {
                Ok(all) if all == ids.len() => return Ok(()),
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "gitoxide couldn't read objects; git reads them");
                    *self.gix.lock() = Gix::Unavailable;
                }
            }
        }
        let rest = ids.get(handed..).unwrap_or_default();
        let mut reader = self.git.lock();
        if reader.is_none() {
            *reader = Some(ObjectReader::start(git, work_tree)?);
        }
        let Some(active) = reader.as_mut() else {
            return Ok(());
        };
        let read = active.read(rest, each);
        if read.is_err() {
            *reader = None;
        }
        read
    }
}

/// A commit as its row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    /// The message's first paragraph, on one line.
    pub(crate) subject: Box<str>,
    /// The author's name.
    pub(crate) author: Box<str>,
    /// The author's email.
    pub(crate) email: Box<str>,
    /// When it was written, in the author's time zone.
    pub(crate) date: Box<str>,
    /// Whether it carries a signature (whether it holds is checked apart).
    pub(crate) signed: bool,
}

impl Summary {
    fn of(commit: &Commit) -> Self {
        let text = |bytes: &[u8]| bytes.to_str_lossy().into_owned().into_boxed_str();
        Self {
            subject: text(&commit.subject()),
            author: text(&commit.author.name),
            email: text(&commit.author.email),
            date: date_time(commit.author.time, commit.author.offset_minutes).into_boxed_str(),
            signed: commit.signed,
        }
    }

    /// What a row shows for a commit git couldn't hand over.
    fn unreadable() -> Self {
        Self {
            subject: "(this commit couldn't be read)".into(),
            author: Box::default(),
            email: Box::default(),
            date: Box::default(),
            signed: false,
        }
    }
}

/// The most signatures checked at once.
const CHECK_AT_ONCE: usize = 64;

/// About how many checked signatures are kept (each took a program's run to check).
const KEPT_CHECKS: usize = 8192;

/// The signatures of the signed commits on screen (and the one selected), checked as they come
/// into view: each check runs its format's program, so only what is shown is checked, a batch
/// at a time.
#[derive(Debug)]
pub(crate) struct Signatures {
    checked: HashMap<ObjectId, SignatureCheck>,
    checking: Bind<Checked, Problem>,
    /// Why the last check failed.
    pub(crate) problem: Option<Problem>,
}

/// A batch checked: the commits asked about, and what checking them found.
#[derive(Debug)]
pub(crate) struct Checked {
    ids: Vec<ObjectId>,
    found: Result<Vec<SignatureCheck>, Problem>,
}

impl Default for Signatures {
    fn default() -> Self {
        Self {
            checked: HashMap::new(),
            checking: Bind::new(true),
            problem: None,
        }
    }
}

impl Signatures {
    /// `id`'s signature, if it was checked.
    pub(crate) fn get(&self, id: &ObjectId) -> Option<&SignatureCheck> {
        self.checked.get(id)
    }

    /// Starts checking the signatures of `wanted` (signed commits) not checked yet, unless a
    /// check is under way. Called once a frame.
    pub(crate) fn want(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        wanted: impl IntoIterator<Item = ObjectId>,
    ) {
        if self.checking.is_pending() {
            return;
        }
        let missing: Vec<ObjectId> = wanted
            .into_iter()
            .filter(|id| !self.checked.contains_key(id))
            .take(CHECK_AT_ONCE)
            .collect();
        if missing.is_empty() {
            return;
        }
        let (git, repository) = (git.clone(), Arc::clone(repository));
        self.checking.request(jobs::blocking(move || {
            let found = check_signatures(&git, &repository, &missing, &CancelToken::new())
                .map_err(|error| Problem::git("Signatures couldn't be checked", &error));
            Ok(Checked {
                ids: missing,
                found,
            })
        }));
    }

    /// Keeps what a finished check found; commits a failed check asked about count as checked
    /// without a verdict (they aren't asked about again and again).
    pub(crate) fn poll(&mut self) {
        let Some(Ok(Checked { ids, found })) = self.checking.take() else {
            return;
        };
        if self.checked.len() > KEPT_CHECKS {
            self.checked.clear();
        }
        match found {
            Ok(checks) => {
                self.problem = None;
                self.checked
                    .extend(checks.into_iter().map(|check| (check.id, check)));
            }
            Err(problem) => {
                self.problem = Some(problem);
                self.checked.extend(ids.into_iter().map(|id| {
                    (
                        id,
                        SignatureCheck {
                            id,
                            verdict: Verdict::Unchecked,
                            signer: BString::default(),
                            key: BString::default(),
                        },
                    )
                }));
            }
        }
    }

    /// Forgets every check (the keys trusted may have changed).
    pub(crate) fn forget(&mut self) {
        self.checked.clear();
    }
}

/// The summaries of the commits on screen (and near it), read as they come into view.
#[derive(Debug)]
pub(crate) struct Summaries {
    kept: HashMap<ObjectId, Kept>,
    reading: Bind<Vec<(ObjectId, Summary)>, Problem>,
    /// Counts the frames that asked for summaries; what was used longest ago goes first.
    clock: u64,
    /// Why the last read failed.
    pub(crate) problem: Option<Problem>,
}

#[derive(Debug)]
struct Kept {
    summary: Summary,
    used: u64,
}

impl Default for Summaries {
    fn default() -> Self {
        Self {
            kept: HashMap::new(),
            reading: Bind::new(true),
            clock: 0,
            problem: None,
        }
    }
}

impl Summaries {
    /// `id`'s summary, if it was read.
    pub(crate) fn get(&mut self, id: &ObjectId) -> Option<&Summary> {
        let clock = self.clock;
        self.kept.get_mut(id).map(|kept| {
            kept.used = clock;
            &kept.summary
        })
    }

    /// `id`'s summary, if it was read, without counting it as used.
    pub(crate) fn peek(&self, id: &ObjectId) -> Option<&Summary> {
        self.kept.get(id).map(|kept| &kept.summary)
    }

    /// Starts reading the summaries of `wanted` not read yet (most urgent first), unless a read
    /// is under way. Called once a frame.
    pub(crate) fn want(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        objects: &Objects,
        wanted: impl IntoIterator<Item = ObjectId>,
    ) {
        self.clock = self.clock.wrapping_add(1);
        if self.reading.is_pending() {
            return;
        }
        let missing: Vec<ObjectId> = wanted
            .into_iter()
            .filter(|id| !self.kept.contains_key(id))
            .take(READ_AT_ONCE)
            .collect();
        if missing.is_empty() {
            return;
        }
        let (git, repository, objects) = (git.clone(), Arc::clone(repository), objects.clone());
        self.reading.request(jobs::blocking(move || {
            let mut found = Vec::with_capacity(missing.len());
            objects
                .read(&git, repository.work_tree(), &missing, |id, object| {
                    let summary = object
                        .filter(|object| object.kind == ObjectKind::Commit)
                        .and_then(|object| Commit::parse(object.data).ok())
                        .map_or_else(Summary::unreadable, |commit| Summary::of(&commit));
                    found.push((id, summary));
                })
                .map_err(|error| Problem::git("Commits couldn't be read", &error))?;
            Ok(found)
        }));
    }

    /// Whether a read is under way.
    #[cfg(test)]
    pub(crate) fn is_reading(&mut self) -> bool {
        self.reading.is_pending()
    }

    /// How many summaries are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.kept.len()
    }

    /// Keeps what a finished read found.
    pub(crate) fn poll(&mut self) {
        let Some(read) = self.reading.take() else {
            return;
        };
        match read {
            Ok(found) => {
                self.problem = None;
                for (id, summary) in found {
                    let used = self.clock;
                    self.kept.insert(id, Kept { summary, used });
                }
                self.forget_least_used();
            }
            Err(problem) => self.problem = Some(problem),
        }
    }

    /// Forgets the summaries used longest ago, down to about [`KEPT_SUMMARIES`]; those used in
    /// the same frame go or stay together.
    fn forget_least_used(&mut self) {
        let Some(excess) = self
            .kept
            .len()
            .checked_sub(KEPT_SUMMARIES.saturating_add(READ_AT_ONCE))
            .filter(|excess| *excess > 0)
        else {
            return;
        };
        let mut used: Vec<u64> = self.kept.values().map(|kept| kept.used).collect();
        let (_, &mut cutoff, _) = used.select_nth_unstable(excess);
        self.kept.retain(|_, kept| kept.used >= cutoff);
    }
}

/// A commit as its details show it: all it records, and the files it changed.
#[derive(Debug)]
pub(crate) struct CommitDetails {
    pub(crate) id: ObjectId,
    pub(crate) commit: Commit,
    pub(crate) changes: Vec<FileChange>,
}

/// A file as a file view shows it.
#[derive(Debug)]
pub(crate) enum FileContent {
    /// Text, measured (boxed: its highlighting's state is large).
    Text(Box<Plain>),
    /// What git would call binary: a NUL in its first 8,000 bytes.
    Binary {
        /// Its size, in bytes.
        size: u64,
    },
    /// A Git LFS pointer: the file itself is stored elsewhere.
    Lfs(LfsPointer),
    /// A submodule: a commit of another repository.
    Submodule {
        /// The commit.
        commit: ObjectId,
    },
    /// Larger than egit shows.
    TooLarge {
        /// Its size, in bytes.
        size: u64,
    },
}

impl FileContent {
    /// What `bytes` show as: binary, an LFS pointer, too large, or text.
    fn of(bytes: &[u8], path: &[u8]) -> Self {
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if bytes.len() > MAX_SHOWN {
            return Self::TooLarge { size };
        }
        let probe = bytes.get(..BINARY_PROBE).unwrap_or(bytes);
        if memchr::memchr(0, probe).is_some() {
            return Self::Binary { size };
        }
        if let Some(pointer) = LfsPointer::parse(bytes) {
            return Self::Lfs(pointer);
        }
        Text::new(Arc::<[u8]>::from(bytes)).map_or(Self::TooLarge { size }, |text| {
            Self::Text(Box::new(Plain::named(
                text,
                egit_text::Options::default(),
                path,
            )))
        })
    }
}

/// A Git LFS pointer: what the repository holds in place of a file stored elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LfsPointer {
    /// The file's size, as the pointer gives it.
    pub(crate) size: Option<u64>,
}

impl LfsPointer {
    /// The pointer `bytes` are, if they are one: under 1 KiB, git-lfs's version line first.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= 1024 {
            return None;
        }
        let rest = bytes.strip_prefix(LFS_POINTER)?;
        let size = rest
            .lines()
            .find_map(|line| line.strip_prefix(b"size "))
            .and_then(|size| size.to_str().ok()?.trim().parse().ok());
        Some(Self { size })
    }
}

/// Reads what `entry` names (the blob of a file at `path`) and makes it ready to show.
pub(crate) async fn read_file(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    entry: TreeEntry,
    path: BString,
) -> Result<Arc<FileContent>, Problem> {
    if entry.mode == TreeEntry::SUBMODULE {
        return Ok(Arc::new(FileContent::Submodule { commit: entry.id }));
    }
    jobs::blocking(move || {
        const UNREADABLE: &str = "The file couldn't be read";
        let mut content = None;
        objects
            .read(&git, repository.work_tree(), &[entry.id], |_, object| {
                content = object
                    .filter(|object| object.kind == ObjectKind::Blob)
                    .map(|object| FileContent::of(object.data, &path));
            })
            .map_err(|error| Problem::git(UNREADABLE, &error))?;
        content.map(Arc::new).ok_or_else(|| {
            Problem::new(
                UNREADABLE,
                format!("{} isn't in this repository.", entry.id),
            )
        })
    })
    .await
}

/// Reads the work tree's file at `path` (from its top) and makes it ready to show.
pub(crate) async fn read_work_tree_file(
    repository: Arc<Repository>,
    path: BString,
) -> Result<Arc<FileContent>, Problem> {
    jobs::blocking(move || {
        let file = repository
            .work_tree()
            .join(std::ffi::OsStr::from_bytes(&path));
        let bytes = std::fs::read(&file).map_err(|error| {
            Problem::new(
                "The file couldn't be read",
                format!("{}: {error}", file.display()),
            )
        })?;
        Ok(Arc::new(FileContent::of(&bytes, &path)))
    })
    .await
}

/// Reads commit `id`'s message (to amend it).
pub(crate) async fn read_message(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    id: ObjectId,
) -> Result<String, Problem> {
    jobs::blocking(move || {
        const UNREADABLE: &str = "The last commit's message couldn't be read";
        let mut message = None;
        objects
            .read(&git, repository.work_tree(), &[id], |_, object| {
                message = object
                    .filter(|object| object.kind == ObjectKind::Commit)
                    .and_then(|object| Commit::parse(object.data).ok())
                    .map(|commit| commit.message.to_str_lossy().into_owned());
            })
            .map_err(|error| Problem::git(UNREADABLE, &error))?;
        message.ok_or_else(|| Problem::new(UNREADABLE, format!("{id} isn't a commit here.")))
    })
    .await
}

/// Reads commit `id` and lists the files it changed (against its first parent).
pub(crate) async fn read_details(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    id: ObjectId,
) -> Result<Arc<CommitDetails>, Problem> {
    jobs::blocking(move || {
        const UNREADABLE: &str = "The commit couldn't be read";
        let mut parsed = None;
        objects
            .read(&git, repository.work_tree(), &[id], |_, object| {
                parsed = object
                    .filter(|object| object.kind == ObjectKind::Commit)
                    .map(|object| Commit::parse(object.data));
            })
            .map_err(|error| Problem::git(UNREADABLE, &error))?;
        let commit = match parsed {
            Some(Ok(commit)) => commit,
            Some(Err(error)) => return Err(Problem::new(UNREADABLE, error.to_string())),
            None => {
                return Err(Problem::new(
                    UNREADABLE,
                    format!("{id} isn't a commit in this repository."),
                ));
            }
        };
        let changes = commit_changes(&git, &repository, id, commit.parents.first().copied())
            .map_err(|error| Problem::git("The commit's changes couldn't be listed", &error))?;
        Ok(Arc::new(CommitDetails {
            id,
            commit,
            changes,
        }))
    })
    .await
}

/// The lines each file a commit changed gained and lost.
#[derive(Debug)]
pub(crate) struct ChangeCounts {
    /// The commit.
    pub(crate) id: ObjectId,
    /// Each file's, in the order of the commit's changes; `None` for a binary file.
    pub(crate) files: Vec<Option<LineCounts>>,
    /// Every file's together.
    pub(crate) total: LineCounts,
}

/// Counts the lines each file of `details` gained and lost (every file is diffed: for a large
/// commit, this takes a while after the list is shown).
pub(crate) async fn read_change_counts(
    git: Git,
    repository: Arc<Repository>,
    details: Arc<CommitDetails>,
    cancel: CancelToken,
) -> Result<Arc<ChangeCounts>, Problem> {
    jobs::blocking(move || {
        let parent = details.commit.parents.first().copied();
        let counted = line_counts(&git, &repository, details.id, parent, &cancel)
            .map_err(|error| Problem::git("The changed lines couldn't be counted", &error))?;
        let files = align_counts(&details.changes, counted);
        let total = files
            .iter()
            .flatten()
            .fold(LineCounts::default(), |total, lines| LineCounts {
                added: total.added.saturating_add(lines.added),
                removed: total.removed.saturating_add(lines.removed),
            });
        Ok(Arc::new(ChangeCounts {
            id: details.id,
            files,
            total,
        }))
    })
    .await
}

/// `counted`'s counts in the order of `changes`: git lists both in the same order, but should
/// they differ, by path.
fn align_counts(changes: &[FileChange], counted: Vec<CountedFile>) -> Vec<Option<LineCounts>> {
    let in_order = changes.len() == counted.len()
        && changes
            .iter()
            .zip(&counted)
            .all(|(change, file)| change.path == file.path);
    if in_order {
        return counted.into_iter().map(|file| file.lines).collect();
    }
    let by_path: HashMap<&[u8], Option<LineCounts>> = counted
        .iter()
        .map(|file| (file.path.as_slice(), file.lines))
        .collect();
    changes
        .iter()
        .map(|change| by_path.get(change.path.as_slice()).copied().flatten())
        .collect()
}

/// A repository's history as its tab keeps it.
#[derive(Debug)]
pub(crate) struct HistoryState {
    /// What the table shows.
    pub(crate) shown: History,
    /// A newer history being read: it takes over once complete, so the table stays put
    /// meanwhile.
    next: Option<History>,
    /// Whether [`Self::shown`] was just replaced (the view then finds its place in it again).
    pub(crate) replaced: bool,
    /// Whether [`Self::shown`] was restarted while incomplete: once complete, it counts as
    /// replaced.
    restarted: bool,
    pub(crate) summaries: Summaries,
    pub(crate) signatures: Signatures,
    pub(crate) objects: Objects,
}

impl HistoryState {
    /// Starts reading `repository`'s history.
    pub(crate) fn new(git: &Git, repository: &Arc<Repository>, ctx: &egui::Context) -> Self {
        let objects = Objects::default();
        Self {
            shown: History::load(git, repository, &objects, ctx),
            next: None,
            replaced: false,
            restarted: false,
            summaries: Summaries::default(),
            signatures: Signatures::default(),
            objects,
        }
    }

    /// Reads the history again. The table keeps showing the current one until the new one is
    /// complete, unless the current one isn't complete either.
    pub(crate) fn reload(&mut self, git: &Git, repository: &Arc<Repository>, ctx: &egui::Context) {
        let next = History::load(git, repository, &self.objects, ctx);
        if self.shown.is_loading() || self.shown.rows() == 0 {
            self.shown = next;
            self.next = None;
            self.restarted = true;
        } else {
            self.next = Some(next);
        }
    }

    /// Takes in what the walks and reads found since.
    pub(crate) fn poll(&mut self) {
        if self.shown.poll() && std::mem::take(&mut self.restarted) {
            self.replaced = true;
        }
        self.summaries.poll();
        self.signatures.poll();
        let Some(next) = &mut self.next else {
            return;
        };
        if !next.poll() {
            return;
        }
        if let Some(next) = self.next.take() {
            if next.problem.is_some() {
                // The rows already shown are kept, with what went wrong reading them again.
                self.shown.problem = next.problem;
            } else {
                self.shown = next;
                self.replaced = true;
            }
        }
    }

    /// Whether a history is being read.
    pub(crate) const fn is_loading(&self) -> bool {
        self.shown.is_loading() || self.next.is_some()
    }
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    fn id(byte: u8) -> ObjectId {
        ObjectId::Sha1([byte; 20])
    }

    fn reference(name: &str, kind: RefKind, target: u8) -> Reference {
        Reference {
            name: name.into(),
            kind,
            target: id(target),
        }
    }

    #[test]
    fn counts_line_up_with_the_changes_listed() {
        let change = |path: &str| FileChange {
            path: path.into(),
            kind: egit_git::ChangeKind::Modified,
            old: None,
            new: None,
        };
        let counted = |path: &str, added| CountedFile {
            path: path.into(),
            lines: Some(LineCounts { added, removed: 0 }),
        };
        let changes = [change("a"), change("b"), change("c")];
        let added = |files: Vec<Option<LineCounts>>| -> Vec<Option<u32>> {
            files
                .into_iter()
                .map(|lines| lines.map(|lines| lines.added))
                .collect()
        };
        let in_order = vec![counted("a", 1), counted("b", 2), counted("c", 3)];
        assert_eq!(
            added(align_counts(&changes, in_order)),
            [Some(1), Some(2), Some(3)]
        );
        // Otherwise by path; a file not counted has no counts.
        let apart = vec![counted("c", 3), counted("a", 1)];
        assert_eq!(
            added(align_counts(&changes, apart)),
            [Some(1), None, Some(3)]
        );
    }

    #[test]
    fn refs_are_found_by_commit_branches_first() {
        let refs = Refs::new(vec![
            reference("refs/tags/v1", RefKind::Tag, 2),
            reference("refs/heads/topic", RefKind::Branch, 3),
            reference("refs/remotes/origin/main", RefKind::RemoteBranch, 2),
            reference("refs/heads/main", RefKind::Branch, 2),
        ]);
        let names = |target| -> Vec<String> {
            refs.at(&id(target))
                .iter()
                .map(|reference| reference.name.to_string())
                .collect()
        };
        assert_eq!(
            names(2),
            [
                "refs/heads/main",
                "refs/remotes/origin/main",
                "refs/tags/v1"
            ]
        );
        assert_eq!(names(3), ["refs/heads/topic"]);
        assert_eq!(names(1), Vec::<String>::new());
        assert_eq!(names(4), Vec::<String>::new());
    }

    /// A history of `rows` made-up commits, as a walk would hand it over: whole, or (when
    /// `ended` is false) with the walk still running until the sender returned is dropped.
    fn walking(rows: u32, ended: bool) -> (History, Sender<Piece>) {
        let (sender, pieces) = mpsc::channel();
        let mut builder = GraphBuilder::new();
        let mut ids = Vec::new();
        for row in 0..rows {
            let commit = ObjectId::Sha1(name(row));
            let parents: Vec<ObjectId> = if row + 1 < rows {
                vec![ObjectId::Sha1(name(row + 1))]
            } else {
                Vec::new()
            };
            if let Some(graph) = builder.push(commit, &parents) {
                let ids = Arc::from(std::mem::take(&mut ids));
                let widest = builder.widest();
                sender.send(Piece::Rows { graph, ids, widest }).unwrap();
            }
            ids.extend_from_slice(commit.as_bytes());
        }
        let widest = builder.widest();
        if let Some(graph) = builder.finish() {
            let ids = Arc::from(ids);
            sender.send(Piece::Rows { graph, ids, widest }).unwrap();
        }
        if ended {
            sender.send(Piece::Done(Ok(()))).unwrap();
        }
        let history = History {
            graph: Vec::new(),
            names: Names::new(ObjectFormat::Sha1),
            rows: 0,
            widest: 0,
            refs: Refs::default(),
            commit_graph: None,
            loading: Some(Loading {
                pieces,
                cancel: CancelToken::new(),
            }),
            problem: None,
        };
        (history, sender)
    }

    /// A whole history of `rows` made-up commits.
    fn history(rows: u32) -> History {
        let (mut history, _) = walking(rows, true);
        assert!(history.poll(), "the walk ended");
        history
    }

    fn state(shown: History, next: Option<History>) -> HistoryState {
        HistoryState {
            shown,
            next,
            replaced: false,
            restarted: false,
            summaries: Summaries::default(),
            signatures: Signatures::default(),
            objects: Objects::default(),
        }
    }

    #[test]
    fn a_reread_takes_over_once_complete() {
        let (next, walk) = walking(7, false);
        let mut state = state(history(5), Some(next));
        state.poll();
        assert!(state.is_loading());
        assert!(!state.replaced);
        assert_eq!(state.shown.rows(), 5, "the history shown stays meanwhile");

        walk.send(Piece::Done(Ok(()))).unwrap();
        state.poll();
        assert!(!state.is_loading());
        assert!(state.replaced);
        assert_eq!(state.shown.rows(), 7);
    }

    #[test]
    fn a_reread_that_failed_keeps_the_rows_shown() {
        let (next, walk) = walking(7, false);
        let mut state = state(history(5), Some(next));
        let problem = Problem::new("The history couldn't be read", "git died");
        walk.send(Piece::Done(Err(problem.clone()))).unwrap();
        state.poll();
        assert!(!state.replaced);
        assert_eq!(state.shown.rows(), 5);
        assert_eq!(state.shown.problem(), Some(&problem));
    }

    #[test]
    fn a_restarted_walk_counts_as_replaced_once_complete() {
        let (shown, walk) = walking(3, false);
        let mut state = state(shown, None);
        state.restarted = true;
        state.poll();
        assert!(!state.replaced, "not while it runs");
        walk.send(Piece::Done(Ok(()))).unwrap();
        state.poll();
        assert!(state.replaced);
        state.replaced = false;
        state.poll();
        assert!(!state.replaced, "only once");
    }

    fn name(row: u32) -> [u8; 20] {
        let mut name = [0; 20];
        name[..4].copy_from_slice(&row.to_be_bytes());
        name
    }

    #[test]
    fn rows_arrive_in_chunks_and_name_their_commits() {
        let history = history(2_500);
        assert_eq!(history.rows(), 2_500);
        assert_eq!(history.graph().len(), 3);
        assert!(!history.is_loading());
        for row in [0, 1, 1023, 1024, 2047, 2048, 2499] {
            let expected = ObjectId::Sha1(name(u32::try_from(row).unwrap()));
            assert_eq!(history.id(row), Some(expected), "row {row}");
            for near in [0, row, 2_499] {
                assert_eq!(history.row_of(&expected, near), Some(row), "from {near}");
            }
        }
        assert_eq!(history.id(2_500), None);
        assert_eq!(history.row_of(&ObjectId::Sha1([0xff; 20]), 0), None);
    }

    #[test]
    fn a_walk_that_vanished_is_a_problem() {
        let (sender, pieces) = mpsc::channel();
        drop(sender);
        let mut history = History {
            graph: Vec::new(),
            names: Names::new(ObjectFormat::Sha1),
            rows: 0,
            widest: 0,
            refs: Refs::default(),
            commit_graph: None,
            loading: Some(Loading {
                pieces,
                cancel: CancelToken::new(),
            }),
            problem: None,
        };
        assert!(history.poll());
        assert_eq!(history.problem(), Some(&Problem::interrupted()));
    }

    #[test]
    fn summaries_used_longest_ago_are_forgotten_first() {
        let mut summaries = Summaries::default();
        let total = KEPT_SUMMARIES + READ_AT_ONCE + 100;
        for index in 0..total {
            summaries.clock = u64::try_from(index).unwrap();
            summaries.kept.insert(
                ObjectId::Sha1(name(u32::try_from(index).unwrap())),
                Kept {
                    summary: Summary::unreadable(),
                    used: summaries.clock,
                },
            );
        }
        // The oldest is used again, so it stays.
        summaries.clock += 1;
        assert!(summaries.get(&ObjectId::Sha1(name(0))).is_some());
        summaries.forget_least_used();
        assert!(summaries.kept.len() <= KEPT_SUMMARIES + READ_AT_ONCE);
        assert!(summaries.get(&ObjectId::Sha1(name(0))).is_some());
        assert!(summaries.get(&ObjectId::Sha1(name(1))).is_none());
        let newest = u32::try_from(total - 1).unwrap();
        assert!(summaries.get(&ObjectId::Sha1(name(newest))).is_some());
    }
}
