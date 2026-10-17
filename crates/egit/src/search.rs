//! Searching the history: the commits whose message or author holds some text, that changed a
//! path, or that a name names. Found in the background, each marked by its row in a bitmap the
//! commit table reads as it draws (a bit a row: 186 KB for Linux's 1.5M commits).
//!
//! - Messages and authors: egit reads every row's commit itself through gitoxide, on up to
//!   [`MAX_THREADS`] threads, a block of rows at a time from the top (Linux's in 0.6 s), with
//!   smart case (a capital matches case; otherwise ASCII letters match either way). The
//!   repository is opened for the search alone and closed after, so the packs read don't stay
//!   mapped. Where gitoxide doesn't read the repository, `git rev-list --grep` or `--author`
//!   (as fixed strings) does, alike.
//! - Paths: `git rev-list --full-history -- <path>` (a file, or anything in a folder), which
//!   the commit-graph's changed-path filters speed up. Not in a partial clone without its
//!   trees: git would fetch them all.
//! - Names: an abbreviated name (4 hex digits or more) among the rows, at once; else what
//!   `git rev-parse` makes of it (a branch, a tag, `HEAD~3`).
//!
//! What git finds is given its row through a [`RowIndex`] of the rows by name (8 bytes a row),
//! built the first time a search needs it and kept until the history is replaced.

use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use bstr::{BStr, ByteSlice as _};
use egit_git::{
    CancelToken, CommitFilter, Git, GitError, ObjectKind, Repository, find_commits, resolve_commit,
};
use egit_gix::{GixError, GixRepository};
use egit_text::{Needle, matches_case};
use parking_lot::Mutex;

use crate::history::{Names, gix_allowed};
use crate::jobs;
use crate::problem::Problem;

/// The most threads reading commits for one search.
const MAX_THREADS: usize = 8;

/// The rows a thread takes at a time.
const BLOCK_ROWS: u64 = 4096;

/// The fewest hex digits searched for as an abbreviated name (git's own shortest).
const MIN_NAME_DIGITS: usize = 4;

/// What a search looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SearchKind {
    /// Commits' messages.
    #[default]
    Message,
    /// Commits' authors: `Name <email>`.
    Author,
    /// The files commits changed.
    Path,
    /// Commits' names, and what a branch, a tag, or `HEAD~3` names.
    Commit,
}

impl SearchKind {
    /// Each kind, as offered.
    pub(crate) const ALL: [Self; 4] = [Self::Message, Self::Author, Self::Path, Self::Commit];

    /// Its name in the search bar.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Message => "Message",
            Self::Author => "Author",
            Self::Path => "Path",
            Self::Commit => "Commit",
        }
    }

    /// What to type, shown in the empty field.
    pub(crate) const fn hint(self) -> &'static str {
        match self {
            Self::Message => "Words in a message",
            Self::Author => "A name or email",
            Self::Path => "A file or folder",
            Self::Commit => "A commit's name, a branch, a tag",
        }
    }
}

/// The rows a search has found, shared with the job finding them.
#[derive(Debug)]
pub(crate) struct Found {
    /// A bit a row.
    bits: Box<[AtomicU64]>,
    rows: u64,
    count: AtomicU64,
    /// How many rows have been read, when the search reads them all (messages, authors).
    read: AtomicU64,
    reads_every_row: AtomicBool,
    /// How it ended: `None` while it runs.
    end: Mutex<Option<Result<(), Problem>>>,
}

impl Found {
    /// Nothing found yet among `rows` rows.
    fn new(rows: u64) -> Self {
        let words = usize::try_from(rows.div_ceil(64)).unwrap_or(0);
        Self {
            bits: (0..words).map(|_| AtomicU64::new(0)).collect(),
            rows,
            count: AtomicU64::new(0),
            read: AtomicU64::new(0),
            reads_every_row: AtomicBool::new(false),
            end: Mutex::new(None),
        }
    }

    /// The word holding `row`'s bit, and its bit there.
    fn place(&self, row: u64) -> Option<(&AtomicU64, u64)> {
        if row >= self.rows {
            return None;
        }
        let word = self.bits.get(usize::try_from(row.checked_div(64)?).ok()?)?;
        Some((
            word,
            1_u64.checked_shl(u32::try_from(row.checked_rem(64)?).ok()?)?,
        ))
    }

    /// Notes `row` found.
    fn mark(&self, row: u64) {
        if let Some((word, bit)) = self.place(row)
            && word.fetch_or(bit, Ordering::Relaxed) & bit == 0
        {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether `row` was found.
    pub(crate) fn contains(&self, row: u64) -> bool {
        self.place(row)
            .is_some_and(|(word, bit)| word.load(Ordering::Relaxed) & bit != 0)
    }

    /// How many rows were found so far.
    pub(crate) fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// How far it got, when it reads every row: rows read, of all.
    pub(crate) fn progress(&self) -> Option<(u64, u64)> {
        self.reads_every_row
            .load(Ordering::Relaxed)
            .then(|| (self.read.load(Ordering::Relaxed), self.rows))
    }

    /// Whether it ended (well or not).
    pub(crate) fn is_done(&self) -> bool {
        self.end.lock().is_some()
    }

    /// Why it failed, if it did.
    pub(crate) fn problem(&self) -> Option<Problem> {
        self.end.lock().as_ref().and_then(|end| end.clone().err())
    }

    /// The first row found from `from` on.
    fn first_from(&self, from: u64) -> Option<u64> {
        let (_, bit) = self.place(from)?;
        let mut index = usize::try_from(from.checked_div(64)?).ok()?;
        // The bits from `from`'s on, in its word.
        let mut word = self.bits.get(index)?.load(Ordering::Relaxed) & !bit.wrapping_sub(1);
        loop {
            if word != 0 {
                let row = u64::try_from(index)
                    .ok()?
                    .checked_mul(64)?
                    .checked_add(u64::from(word.trailing_zeros()))?;
                return (row < self.rows).then_some(row);
            }
            index = index.checked_add(1)?;
            word = self.bits.get(index)?.load(Ordering::Relaxed);
        }
    }

    /// The last row found before `before`.
    fn last_before(&self, before: u64) -> Option<u64> {
        let last = before.min(self.rows).checked_sub(1)?;
        let (_, bit) = self.place(last)?;
        let mut index = usize::try_from(last.checked_div(64)?).ok()?;
        // The bits up to `last`'s, in its word.
        let mut word = self.bits.get(index)?.load(Ordering::Relaxed) & (bit | bit.wrapping_sub(1));
        loop {
            if word != 0 {
                let top = 63_u32.checked_sub(word.leading_zeros())?;
                return u64::try_from(index)
                    .ok()?
                    .checked_mul(64)?
                    .checked_add(u64::from(top));
            }
            index = index.checked_sub(1)?;
            word = self.bits.get(index)?.load(Ordering::Relaxed);
        }
    }

    /// The next row found after `row` (from the top without one), wrapping around.
    pub(crate) fn next(&self, row: Option<u64>) -> Option<u64> {
        let from = row.map_or(0, |row| row.saturating_add(1));
        self.first_from(from).or_else(|| self.first_from(0))
    }

    /// The row found before `row` (from the bottom without one), wrapping around.
    pub(crate) fn previous(&self, row: Option<u64>) -> Option<u64> {
        row.and_then(|row| self.last_before(row))
            .or_else(|| self.last_before(self.rows))
    }

    /// How many rows found come before `row`.
    pub(crate) fn rank(&self, row: u64) -> u64 {
        let Some((word, bit)) = self.place(row) else {
            return self.count();
        };
        let whole = usize::try_from(row.checked_div(64).unwrap_or(0)).unwrap_or(0);
        let before: u64 = self
            .bits
            .iter()
            .take(whole)
            .map(|word| u64::from(word.load(Ordering::Relaxed).count_ones()))
            .sum();
        before.saturating_add(u64::from(
            (word.load(Ordering::Relaxed) & bit.wrapping_sub(1)).count_ones(),
        ))
    }
}

/// The rows by their commits' names: each row's name's first four bytes beside the row, sorted
/// (8 bytes a row: 12 MB for Linux), so a name is found by a binary search, then checked whole
/// against the names.
#[derive(Debug)]
pub(crate) struct RowIndex {
    keys: Vec<u64>,
}

/// A row's key: its name's first four bytes (the order) above the row.
fn key(prefix: u32, row: u32) -> u64 {
    u64::from(prefix).wrapping_shl(32) | u64::from(row)
}

/// A key's name's first four bytes.
fn key_prefix(key: u64) -> u32 {
    u32::try_from(key.wrapping_shr(32)).unwrap_or(u32::MAX)
}

/// A key's row.
fn key_row(key: u64) -> u64 {
    key & u64::from(u32::MAX)
}

/// A name's first four bytes, as a number.
fn prefix(name: &[u8]) -> u32 {
    name.first_chunk::<4>()
        .map_or(0, |bytes| u32::from_be_bytes(*bytes))
}

impl RowIndex {
    /// Indexes the rows `names` names.
    pub(crate) fn build(names: &Names) -> Self {
        let mut keys = Vec::with_capacity(usize::try_from(names.rows()).unwrap_or(0));
        for (first, chunk) in names.chunks() {
            for (row, name) in (first..).zip(chunk.chunks_exact(names.width())) {
                let Ok(row) = u32::try_from(row) else {
                    break;
                };
                keys.push(key(prefix(name), row));
            }
        }
        keys.sort_unstable();
        Self { keys }
    }

    /// Each row whose name's first four bytes are from `low` to `high`.
    fn each_between(&self, low: u32, high: u32, mut each: impl FnMut(u64)) {
        let start = self.keys.partition_point(|&found| found < key(low, 0));
        for &found in self.keys.get(start..).unwrap_or_default() {
            if key_prefix(found) > high {
                break;
            }
            each(key_row(found));
        }
    }

    /// The row of the commit named `name`.
    pub(crate) fn row_of(&self, names: &Names, name: &[u8]) -> Option<u64> {
        let mut found = None;
        let first = prefix(name);
        self.each_between(first, first, |row| {
            if found.is_none() && names.bytes(row) == Some(name) {
                found = Some(row);
            }
        });
        found
    }

    /// Each row whose name starts with the hex digits `digits` (lowercase).
    fn each_abbreviated(&self, names: &Names, digits: &[u8], mut each: impl FnMut(u64)) {
        // The first eight digits bound the first four bytes; the rest are checked whole.
        let bound = |fill: u8| {
            let mut padded = [fill; 8];
            for (slot, digit) in padded.iter_mut().zip(digits) {
                *slot = *digit;
            }
            std::str::from_utf8(&padded)
                .ok()
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        };
        let (Some(low), Some(high)) = (bound(b'0'), bound(b'f')) else {
            return;
        };
        self.each_between(low, high, |row| {
            if names
                .bytes(row)
                .is_some_and(|name| starts_with_digits(name, digits))
            {
                each(row);
            }
        });
    }
}

/// Whether `name`'s hex digits start with `digits` (lowercase).
fn starts_with_digits(name: &[u8], digits: &[u8]) -> bool {
    let nibbles = name
        .iter()
        .flat_map(|byte| [byte.checked_shr(4).unwrap_or(0), byte & 0x0f]);
    let mut wanted = digits.iter().map(|digit| char::from(*digit).to_digit(16));
    nibbles
        .zip(&mut wanted)
        .all(|(nibble, digit)| digit == Some(u32::from(nibble)))
        && wanted.next().is_none()
}

/// Whether `query` (trimmed) is an abbreviated name: hex digits, at least
/// [`MIN_NAME_DIGITS`], and no more than a name of `width` bytes has.
fn abbreviated_name(query: &str, width: usize) -> Option<Vec<u8>> {
    let longest = width.checked_mul(2)?;
    (query.len() >= MIN_NAME_DIGITS
        && query.len() <= longest
        && query.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then(|| {
        query
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .collect()
    })
}

/// A commit's message: what follows the blank line ending its headers.
fn message_of(commit: &[u8]) -> &[u8] {
    commit
        .find(b"\n\n")
        .and_then(|end| commit.get(end.saturating_add(2)..))
        .unwrap_or_default()
}

/// A commit's author as `Name <email>` (its time left out), as `git log --author` sees it.
fn author_of(commit: &[u8]) -> &[u8] {
    let headers = commit
        .find(b"\n\n")
        .and_then(|end| commit.get(..end))
        .unwrap_or(commit);
    headers
        .lines()
        .find_map(|line| line.strip_prefix(b"author "))
        .and_then(|ident| {
            let end = ident.rfind_byte(b'>')?;
            ident.get(..=end)
        })
        .unwrap_or_default()
}

/// A search running or done: what it has found, and the means to stop it.
#[derive(Debug)]
pub(crate) struct Search {
    found: Arc<Found>,
    cancel: CancelToken,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Search {
    /// What it has found.
    pub(crate) const fn found(&self) -> &Arc<Found> {
        &self.found
    }
}

/// What a search is for, and where.
pub(crate) struct Asked<'a> {
    pub(crate) git: &'a Git,
    pub(crate) repository: &'a Arc<Repository>,
    /// The history's rows' names (it must be read whole).
    pub(crate) names: Names,
    /// The rows by name, for the history searched: built by the first search needing it.
    pub(crate) index: &'a Arc<OnceLock<RowIndex>>,
    /// Whether every commit's trees are here (a path can be searched).
    pub(crate) every_tree: bool,
    pub(crate) kind: SearchKind,
    pub(crate) query: &'a str,
}

/// Starts searching; the commit table is woken when it ends.
pub(crate) fn start(asked: Asked<'_>, ctx: &egui::Context) -> Search {
    let found = Arc::new(Found::new(asked.names.rows()));
    let cancel = CancelToken::new();
    let job = Job {
        git: asked.git.clone(),
        repository: Arc::clone(asked.repository),
        names: asked.names,
        index: Arc::clone(asked.index),
        every_tree: asked.every_tree,
        kind: asked.kind,
        query: asked.query.to_owned(),
        found: Arc::clone(&found),
        cancel: cancel.clone(),
    };
    let ctx = ctx.clone();
    jobs::detached(move || {
        let ended = job.run();
        if !job.cancel.is_cancelled() {
            *job.found.end.lock() = Some(ended);
        }
        ctx.request_repaint();
    });
    Search { found, cancel }
}

/// A search's work, in its job.
struct Job {
    git: Git,
    repository: Arc<Repository>,
    names: Names,
    index: Arc<OnceLock<RowIndex>>,
    every_tree: bool,
    kind: SearchKind,
    query: String,
    found: Arc<Found>,
    cancel: CancelToken,
}

impl Job {
    fn run(&self) -> Result<(), Problem> {
        let fold_case = !matches_case(&self.query);
        match self.kind {
            SearchKind::Message | SearchKind::Author => {
                if let Some(gix) = gix_allowed()
                    .then(|| GixRepository::open(self.repository.work_tree()))
                    .flatten()
                {
                    match self.read_every_row(&gix, fold_case) {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            tracing::warn!(%error, "gitoxide couldn't search; git searches");
                        }
                    }
                }
                let text = &self.query;
                let filter = if self.kind == SearchKind::Message {
                    CommitFilter::Message { text, fold_case }
                } else {
                    CommitFilter::Author { text, fold_case }
                };
                self.through_git(filter)
            }
            SearchKind::Path => {
                if !self.every_tree {
                    return Err(Problem::new(
                        "Files aren't searched in this partial clone",
                        "It was cloned without its trees: git would fetch every commit's from \
                         the remote.",
                    ));
                }
                let path = self.query.trim().trim_matches('/');
                self.through_git(CommitFilter::Path(BStr::new(path)))
            }
            SearchKind::Commit => self.names_or_refs(),
        }
    }

    /// Reads every row's commit through gitoxide, on up to [`MAX_THREADS`] threads, a block
    /// at a time from the top, marking those whose message (or author) holds the query.
    fn read_every_row(&self, gix: &GixRepository, fold_case: bool) -> Result<(), GixError> {
        let needle = Needle::new(self.query.as_bytes(), fold_case);
        let of_author = self.kind == SearchKind::Author;
        let rows = self.names.rows();
        let threads = std::thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .min(MAX_THREADS);
        let next_block = AtomicUsize::new(0);
        let failed: Mutex<Option<GixError>> = Mutex::new(None);
        self.found.reads_every_row.store(true, Ordering::Relaxed);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut reader = gix.reader();
                    loop {
                        let block = next_block.fetch_add(1, Ordering::Relaxed);
                        let Some(start) = u64::try_from(block)
                            .ok()
                            .and_then(|block| block.checked_mul(BLOCK_ROWS))
                            .filter(|&start| start < rows)
                        else {
                            return;
                        };
                        if self.cancel.is_cancelled() || failed.lock().is_some() {
                            return;
                        }
                        let end = start.saturating_add(BLOCK_ROWS).min(rows);
                        for row in start..end {
                            let Some(id) = self.names.id(row) else {
                                continue;
                            };
                            match reader.read(&id) {
                                Ok(Some(object)) if object.kind == ObjectKind::Commit => {
                                    let text = if of_author {
                                        author_of(object.data)
                                    } else {
                                        message_of(object.data)
                                    };
                                    if needle.find(text).is_some() {
                                        self.found.mark(row);
                                    }
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    *failed.lock() = Some(error);
                                    return;
                                }
                            }
                        }
                        self.found
                            .read
                            .fetch_add(end.saturating_sub(start), Ordering::Relaxed);
                    }
                });
            }
        });
        let failed = failed.into_inner();
        self.found.reads_every_row.store(false, Ordering::Relaxed);
        failed.map_or(Ok(()), Err)
    }

    /// Has git find the commits, and marks their rows.
    fn through_git(&self, filter: CommitFilter<'_>) -> Result<(), Problem> {
        let index = self.index.get_or_init(|| RowIndex::build(&self.names));
        find_commits(&self.git, &self.repository, filter, &self.cancel, |id| {
            if let Some(row) = index.row_of(&self.names, id.as_bytes()) {
                self.found.mark(row);
            }
            ControlFlow::Continue(())
        })
        .map_err(|error| searched(&error))
    }

    /// The rows whose commits' names start as the query does, else the commit git makes of it.
    fn names_or_refs(&self) -> Result<(), Problem> {
        let query = self.query.trim();
        let index = self.index.get_or_init(|| RowIndex::build(&self.names));
        if let Some(digits) = abbreviated_name(query, self.names.width()) {
            index.each_abbreviated(&self.names, &digits, |row| self.found.mark(row));
        }
        if self.found.count() > 0 {
            return Ok(());
        }
        let resolved =
            resolve_commit(&self.git, &self.repository, query).map_err(|error| searched(&error))?;
        if let Some(row) = resolved.and_then(|id| index.row_of(&self.names, id.as_bytes())) {
            self.found.mark(row);
        }
        Ok(())
    }
}

/// A search git failed.
fn searched(error: &GitError) -> Problem {
    Problem::git("The history couldn't be searched", error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(rows: u64, marked: &[u64]) -> Found {
        let found = Found::new(rows);
        for &row in marked {
            found.mark(row);
        }
        found
    }

    #[test]
    fn rows_found_are_stepped_through_both_ways_and_wrap() {
        let found = found(200, &[3, 64, 130, 199]);
        assert_eq!(found.count(), 4);
        assert!(found.contains(64) && !found.contains(65));
        assert_eq!(found.next(None), Some(3));
        assert_eq!(found.next(Some(3)), Some(64));
        assert_eq!(found.next(Some(64)), Some(130));
        assert_eq!(found.next(Some(131)), Some(199));
        assert_eq!(found.next(Some(199)), Some(3), "wraps to the top");
        assert_eq!(found.previous(Some(130)), Some(64));
        assert_eq!(found.previous(Some(64)), Some(3));
        assert_eq!(found.previous(Some(3)), Some(199), "wraps to the bottom");
        assert_eq!(found.previous(None), Some(199));
        assert_eq!(
            [3, 64, 130, 199].map(|row| found.rank(row)),
            [0, 1, 2, 3],
            "each one's place"
        );
        assert_eq!(found.rank(100), 2);
        // Marked twice, counted once; past the end, not at all.
        found.mark(64);
        found.mark(200);
        assert_eq!(found.count(), 4);
        let none = Found::new(100);
        assert_eq!((none.next(None), none.previous(None)), (None, None));
    }

    #[test]
    fn rows_are_found_by_name_and_by_abbreviated_name() {
        let names = Names::made_up(3_000);
        let index = RowIndex::build(&names);
        for row in [0, 1, 1023, 1024, 2_999] {
            let id = names.id(row).unwrap();
            assert_eq!(index.row_of(&names, id.as_bytes()), Some(row));
        }
        assert_eq!(index.row_of(&names, &[0xff; 20]), None);
        let id = names.id(1_500).unwrap().to_string();
        for digits in [4, 7, 8, 9, 40] {
            let mut rows = Vec::new();
            index.each_abbreviated(&names, &id.as_bytes()[..digits], |row| rows.push(row));
            assert!(rows.contains(&1_500), "{digits} digits");
            for row in rows {
                assert!(
                    names
                        .id(row)
                        .unwrap()
                        .to_string()
                        .starts_with(&id[..digits])
                );
            }
        }
    }

    #[test]
    fn a_query_is_an_abbreviated_name_only_when_it_could_be() {
        assert_eq!(abbreviated_name("ABCD", 20), Some(b"abcd".to_vec()));
        assert_eq!(abbreviated_name("abc", 20), None);
        assert_eq!(abbreviated_name("main", 20), None);
        assert_eq!(abbreviated_name(&"a".repeat(41), 20), None);
        assert_eq!(
            abbreviated_name(&"a".repeat(64), 32).map(|d| d.len()),
            Some(64)
        );
        assert!(starts_with_digits(&[0xab, 0xcd], b"abc"));
        assert!(!starts_with_digits(&[0xab, 0xcd], b"abd"));
        assert!(!starts_with_digits(&[0xab], b"abc"), "longer than the name");
    }

    #[test]
    fn a_commit_s_message_and_author_are_read_from_it() {
        let commit = b"tree 1234\nauthor Ada Lovelace <ada@example.org> 1700000000 +0100\n\
                       committer Bo <bo@example.org> 1700000000 +0000\ngpgsig -----BEGIN\n \n \
                       -----END\n\nFix a leak\n\nIn detail.\n";
        assert_eq!(author_of(commit), b"Ada Lovelace <ada@example.org>");
        assert_eq!(message_of(commit), b"Fix a leak\n\nIn detail.\n");
        assert_eq!(message_of(b"tree 1234\n"), b"");
        assert_eq!(author_of(b"tree 1234\n\nauthor Not <a header>"), b"");
    }
}
