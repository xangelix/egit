//! A repository's statistics, for its Stats page: every commit the history shows read once and
//! tallied as it is read (each day's commits, each weekday's hours as their authors' clocks
//! said, merges, who wrote them and the months they were active, through the mailmap); the
//! files at `HEAD` by language, and the largest; and, after the rest, the lines the newest
//! commits changed.
//!
//! Commits are read through gitoxide on up to eight threads, each tallying a block of rows at a
//! time, else through one `cat-file`. Nothing a commit holds is kept: a counter a day, an
//! author, a month.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use bstr::{BStr, ByteSlice as _};
use egit_git::sync::Mutex;
use egit_git::{
    CancelToken, Git, GitError, ObjectId, ObjectKind, ObjectReader, Repository, commit_lines,
    map_contacts, tree_files,
};
use egit_gix::{GixError, GixRepository};

use crate::history::{Names, gix_allowed_for};
use crate::jobs;
use crate::problem::Problem;

/// The most threads reading commits.
const MAX_THREADS: usize = 8;

/// The rows a thread takes at a time.
const BLOCK_ROWS: u64 = 4096;

/// The newest commits whose lines are counted: each is diffed.
pub(crate) const LINES_COUNTED: u64 = 10_000;

/// The largest files listed.
const LARGEST: usize = 12;

/// A day, in seconds.
const DAY: i64 = 86_400;

/// A history's statistics.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub(crate) commits: u64,
    pub(crate) merges: u64,
    /// The first and the latest commit's time (their authors', seconds since the Unix epoch).
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
    /// Commits each day (days since the Unix epoch, as their authors' clocks said), by day.
    pub(crate) days: Vec<(i32, u32)>,
    /// Commits each month (months since January 1970), by month.
    pub(crate) months: Vec<(i32, u32)>,
    /// Commits by weekday (Monday first) and hour, as their authors' clocks said.
    pub(crate) punch: [[u32; 24]; 7],
    /// Who wrote them (through the mailmap), the most commits first.
    pub(crate) authors: Vec<AuthorStats>,
    /// How many authors made a commit each month (months since January 1970), by month.
    pub(crate) active: Vec<(i32, u32)>,
    /// The files at `HEAD` (none before the first commit, or where they couldn't be listed).
    pub(crate) files: Option<FileStats>,
}

impl Stats {
    /// The longest run of days, one after another, with a commit each: how many, and its
    /// last day.
    pub(crate) fn streak(&self) -> Option<(u32, i32)> {
        let mut best: Option<(u32, i32)> = None;
        let mut run: Option<(u32, i32)> = None;
        for &(day, _) in &self.days {
            run = match run {
                Some((length, last)) if last.checked_add(1) == Some(day) => {
                    Some((length.saturating_add(1), day))
                }
                _ => Some((1, day)),
            };
            if run.map(|(length, _)| length) > best.map(|(length, _)| length) {
                best = run;
            }
        }
        best
    }

    /// The day with the most commits (the latest such), and how many.
    pub(crate) fn busiest(&self) -> Option<(i32, u32)> {
        self.days
            .iter()
            .copied()
            .max_by_key(|&(day, commits)| (commits, day))
    }
}

/// Someone who wrote commits, through the mailmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthorStats {
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) commits: u64,
    /// Their first and latest commit's time.
    pub(crate) first: i64,
    pub(crate) last: i64,
}

/// The files a commit holds.
#[derive(Debug, Default)]
pub(crate) struct FileStats {
    pub(crate) files: u64,
    /// Their bytes, all told (none where sizes weren't read: a partial clone).
    pub(crate) bytes: Option<u64>,
    /// By language (from their names), the most bytes (or files) first.
    pub(crate) languages: Vec<Language>,
    /// The largest, the largest first.
    pub(crate) largest: Vec<(String, u64)>,
}

/// Files of one language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Language {
    pub(crate) name: &'static str,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
}

/// The lines the newest commits changed.
#[derive(Debug, Default)]
pub(crate) struct LineStats {
    /// The commits counted.
    pub(crate) commits: u64,
    pub(crate) added: u64,
    pub(crate) removed: u64,
    /// Lines added and removed each month (months since January 1970), by month.
    pub(crate) months: Vec<(i32, u64, u64)>,
    /// Each author's lines added and removed, by their email (through the mailmap) in lower
    /// case.
    pub(crate) authors: HashMap<String, (u64, u64)>,
    /// How many commits changed no line, 1–9, 10–99, 100–999, 1,000–9,999, and 10,000 or more.
    pub(crate) sizes: [u64; 6],
}

/// Reads `names`' commits and the files at `HEAD` (their sizes when `sizes`), counting the
/// commits read in `read`.
pub(crate) async fn read_stats(
    git: Git,
    repository: Arc<Repository>,
    names: Names,
    sizes: bool,
    read: Arc<AtomicU64>,
    cancel: CancelToken,
) -> Result<Arc<Stats>, Problem> {
    jobs::blocking(async move {
        const TITLE: &str = "The statistics couldn't be read";
        let through_gix = gix_allowed_for(&git)
            .then(|| GixRepository::open(repository.work_tree()))
            .flatten()
            .and_then(|gix| match tally_gix(&gix, &names, &read, &cancel) {
                Ok(tally) => Some(tally),
                Err(error) => {
                    tracing::warn!(%error, "gitoxide couldn't read the commits; git reads them");
                    read.store(0, Ordering::Relaxed);
                    None
                }
            });
        let tally = match through_gix {
            Some(tally) => tally,
            None => tally_git(&git, &repository, &names, &read, &cancel)
                .await
                .map_err(|error| Problem::git(TITLE, &error))?,
        };
        let mut stats = finish(&git, &repository, tally, &cancel)
            .await
            .map_err(|error| Problem::git(TITLE, &error))?;
        // Before the first commit, or where they can't be listed, the page goes without them.
        stats.files = file_stats(&git, &repository, sizes, &cancel).await.ok();
        Ok(Arc::new(stats))
    })
    .await
}

/// Counts the lines the newest of `names`' commits (at most [`LINES_COUNTED`]) changed,
/// counting those counted in `counted`.
pub(crate) async fn read_lines(
    git: Git,
    repository: Arc<Repository>,
    names: Names,
    counted: Arc<AtomicU64>,
    cancel: CancelToken,
) -> Result<Arc<LineStats>, Problem> {
    jobs::blocking(async move {
        let newest: Vec<ObjectId> = (0..names.rows().min(LINES_COUNTED))
            .filter_map(|row| names.id(row))
            .collect();
        let mut stats = LineStats::default();
        let mut months: HashMap<i32, (u64, u64)> = HashMap::new();
        let mut cache = HashMap::new();
        commit_lines(&git, &repository, &newest, &cancel, |lines| {
            counted.fetch_add(1, Ordering::Relaxed);
            stats.commits = stats.commits.saturating_add(1);
            stats.added = stats.added.saturating_add(lines.added);
            stats.removed = stats.removed.saturating_add(lines.removed);
            if let Some(month) = day_of(lines.time).and_then(|day| month_cached(&mut cache, day)) {
                let month = months.entry(month).or_default();
                month.0 = month.0.saturating_add(lines.added);
                month.1 = month.1.saturating_add(lines.removed);
            }
            let author = stats
                .authors
                .entry(lines.email.to_str_lossy().to_lowercase())
                .or_default();
            author.0 = author.0.saturating_add(lines.added);
            author.1 = author.1.saturating_add(lines.removed);
            let changed = lines.added.saturating_add(lines.removed);
            let bucket = match changed {
                0 => 0,
                1..=9 => 1,
                10..=99 => 2,
                100..=999 => 3,
                1_000..=9_999 => 4,
                _ => 5,
            };
            if let Some(size) = stats.sizes.get_mut(bucket) {
                *size = size.saturating_add(1);
            }
            if cancel.is_cancelled() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .await
        .map_err(|error| Problem::git("The changed lines couldn't be counted", &error))?;
        let mut months: Vec<(i32, u64, u64)> = months
            .into_iter()
            .map(|(month, (added, removed))| (month, added, removed))
            .collect();
        months.sort_unstable_by_key(|&(month, ..)| month);
        stats.months = months;
        Ok(Arc::new(stats))
    })
    .await
}

/// A commit's figures, as a reading thread hands them over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Read {
    /// Its author line's `Name <email>`, hashed ([`fnv`]).
    contact: u64,
    /// When its author made it, and their clock's offset from UTC, in seconds.
    time: i64,
    offset: i64,
    merge: bool,
}

impl Read {
    /// A commit object's figures, and its author line's `Name <email>`.
    fn of(data: &[u8]) -> Option<(Self, &[u8])> {
        let header = Header::parse(data)?;
        Some((
            Self {
                contact: fnv(header.contact),
                time: header.time,
                offset: header.offset,
                merge: header.parents > 1,
            },
            header.contact,
        ))
    }
}

/// FNV-1a: a contact's 64 bits, for tallying it without its words.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Commits read on one thread, handed over together: their figures, and the contacts the
/// thread hadn't met before.
#[derive(Debug, Default)]
struct Batch {
    reads: Vec<Read>,
    names: Vec<(u64, Vec<u8>)>,
}

/// What the read has tallied.
#[derive(Debug, Default)]
struct Tally {
    commits: u64,
    merges: u64,
    first: Option<i64>,
    last: Option<i64>,
    days: HashMap<i32, u32>,
    punch: [[u32; 24]; 7],
    /// Each author line's commits, by its `Name <email>`'s hash.
    contacts: HashMap<u64, Contact>,
    /// Each author line's `Name <email>`, by its hash.
    names: HashMap<u64, Vec<u8>>,
    /// Each day's month, worked out once.
    months: HashMap<i32, i32>,
}

/// One `Name <email>`'s commits.
#[derive(Debug, Default)]
struct Contact {
    commits: u64,
    first: Option<i64>,
    last: Option<i64>,
    /// The months it made a commit in: each added when it isn't the last one added (commits
    /// come about in time's order), sorted and made unique at the end.
    months: Vec<i32>,
}

impl Contact {
    /// Its months, sorted, each once.
    fn settle(&mut self) {
        self.months.sort_unstable();
        self.months.dedup();
        self.months.shrink_to_fit();
    }
}

impl Tally {
    /// Tallies a commit object.
    fn add(&mut self, data: &[u8]) {
        if let Some((read, contact)) = Read::of(data) {
            self.names
                .entry(read.contact)
                .or_insert_with(|| contact.to_vec());
            self.count(&read);
        }
    }

    /// Tallies a commit's figures.
    fn count(&mut self, read: &Read) {
        self.commits = self.commits.saturating_add(1);
        if read.merge {
            self.merges = self.merges.saturating_add(1);
        }
        self.first = earliest(self.first, Some(read.time));
        self.last = latest(self.last, Some(read.time));
        let local = read.time.checked_add(read.offset);
        let day = local.and_then(day_of);
        let month = day.and_then(|day| month_cached(&mut self.months, day));
        if let (Some(local), Some(day)) = (local, day) {
            let count = self.days.entry(day).or_insert(0);
            *count = count.saturating_add(1);
            let weekday = i64::from(day)
                .checked_add(3)
                .map(|days| days.rem_euclid(7))
                .and_then(|weekday| usize::try_from(weekday).ok());
            let hour = local
                .rem_euclid(DAY)
                .checked_div(3_600)
                .and_then(|hour| usize::try_from(hour).ok());
            if let Some(cell) = weekday
                .zip(hour)
                .and_then(|(weekday, hour)| self.punch.get_mut(weekday)?.get_mut(hour))
            {
                *cell = cell.saturating_add(1);
            }
        }
        let contact = self.contacts.entry(read.contact).or_default();
        contact.commits = contact.commits.saturating_add(1);
        contact.first = earliest(contact.first, Some(read.time));
        contact.last = latest(contact.last, Some(read.time));
        if let Some(month) = month
            && contact.months.last() != Some(&month)
        {
            contact.months.push(month);
        }
    }

    /// Tallies a batch a thread read.
    fn take(&mut self, batch: Batch) {
        for (hash, contact) in batch.names {
            self.names.entry(hash).or_insert(contact);
        }
        for read in &batch.reads {
            self.count(read);
        }
    }
}

fn earliest(one: Option<i64>, other: Option<i64>) -> Option<i64> {
    match (one, other) {
        (Some(one), Some(other)) => Some(one.min(other)),
        (one, other) => one.or(other),
    }
}

fn latest(one: Option<i64>, other: Option<i64>) -> Option<i64> {
    one.max(other)
}

/// What a commit's headers say that its statistics need.
#[derive(Debug, PartialEq, Eq)]
struct Header<'a> {
    parents: u32,
    /// Its author line's `Name <email>`.
    contact: &'a [u8],
    /// When its author made it, and their clock's offset from UTC, in seconds.
    time: i64,
    offset: i64,
}

impl<'a> Header<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        let mut parents = 0_u32;
        let mut author = None;
        for line in data.lines() {
            if line.is_empty() {
                break;
            }
            if line.starts_with(b"parent ") {
                parents = parents.saturating_add(1);
            } else if let Some(rest) = line.strip_prefix(b"author ") {
                author = Some(rest);
            }
        }
        let author = author?;
        let close = author.rfind_byte(b'>')?;
        let contact = author.get(..=close)?;
        let mut rest = author
            .get(close.checked_add(1)?..)?
            .fields_with(char::is_whitespace);
        let time = rest.next()?.to_str().ok()?.parse().ok()?;
        let offset = rest.next().and_then(zone_offset).unwrap_or(0);
        Some(Self {
            parents,
            contact,
            time,
            offset,
        })
    }
}

/// `+0200` (or `-0530`) in seconds.
fn zone_offset(zone: &[u8]) -> Option<i64> {
    let (sign, digits) = zone.split_first()?;
    let value: i64 = digits.to_str().ok()?.parse().ok()?;
    let hours = value.checked_div(100)?;
    let minutes = value.checked_rem(100)?;
    let seconds = hours
        .checked_mul(3_600)?
        .checked_add(minutes.checked_mul(60)?)?;
    match sign {
        b'+' => Some(seconds),
        b'-' => seconds.checked_neg(),
        _ => None,
    }
}

/// The day (since the Unix epoch) `seconds` falls on.
pub(crate) fn day_of(seconds: i64) -> Option<i32> {
    i32::try_from(seconds.div_euclid(DAY)).ok()
}

/// The month (since January 1970) `day` falls in, worked out once for each day.
fn month_cached(cache: &mut HashMap<i32, i32>, day: i32) -> Option<i32> {
    if let Some(&month) = cache.get(&day) {
        return Some(month);
    }
    let month = month_of(day)?;
    cache.insert(day, month);
    Some(month)
}

/// The month (since January 1970) `day` (since the Unix epoch) falls in.
pub(crate) fn month_of(day: i32) -> Option<i32> {
    let date = date_of(day)?;
    let years = i32::from(date.year()).checked_sub(1970)?;
    years
        .checked_mul(12)?
        .checked_add(i32::from(date.month()).checked_sub(1)?)
}

/// The date `day` (since the Unix epoch) is.
pub(crate) fn date_of(day: i32) -> Option<jiff::civil::Date> {
    let span = jiff::Span::new().try_days(i64::from(day)).ok()?;
    jiff::civil::date(1970, 1, 1).checked_add(span).ok()
}

/// Reads every row's commit through gitoxide, on up to [`MAX_THREADS`] threads, a block of rows
/// at a time; each hands its block's figures over, and this thread tallies them (one tally:
/// threads keep no more than the contacts they met, as hashes).
fn tally_gix(
    gix: &GixRepository,
    names: &Names,
    read: &AtomicU64,
    cancel: &CancelToken,
) -> Result<Tally, GixError> {
    let rows = names.rows();
    let threads = std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(MAX_THREADS);
    let next_block = AtomicUsize::new(0);
    let failed: Mutex<Option<GixError>> = Mutex::new(None);
    let (sender, batches) = std::sync::mpsc::sync_channel::<Batch>(threads.saturating_mul(2));
    let mut tally = Tally::default();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let sender = sender.clone();
            let (next_block, failed) = (&next_block, &failed);
            scope.spawn(move || {
                let mut reader = gix.reader();
                let mut met: HashSet<u64> = HashSet::new();
                loop {
                    let block = next_block.fetch_add(1, Ordering::Relaxed);
                    let Some(start) = u64::try_from(block)
                        .ok()
                        .and_then(|block| block.checked_mul(BLOCK_ROWS))
                        .filter(|&start| start < rows)
                    else {
                        return;
                    };
                    if cancel.is_cancelled() || failed.lock().is_some() {
                        return;
                    }
                    let end = start.saturating_add(BLOCK_ROWS).min(rows);
                    let mut batch = Batch::default();
                    for row in start..end {
                        let Some(id) = names.id(row) else {
                            continue;
                        };
                        match reader.read(&id) {
                            Ok(Some(object)) if object.kind == ObjectKind::Commit => {
                                if let Some((commit, contact)) = Read::of(object.data) {
                                    if met.insert(commit.contact) {
                                        batch.names.push((commit.contact, contact.to_vec()));
                                    }
                                    batch.reads.push(commit);
                                }
                            }
                            Ok(_) => {}
                            Err(error) => {
                                *failed.lock() = Some(error);
                                return;
                            }
                        }
                    }
                    if sender.send(batch).is_err() {
                        return;
                    }
                    read.fetch_add(end.saturating_sub(start), Ordering::Relaxed);
                }
            });
        }
        // The threads hold the only senders left: the batches end with them.
        drop(sender);
        for batch in batches {
            tally.take(batch);
        }
    });
    failed.into_inner().map_or(Ok(tally), Err)
}

/// Reads every row's commit through one `cat-file`, a block at a time.
async fn tally_git(
    git: &Git,
    repository: &Repository,
    names: &Names,
    read: &AtomicU64,
    cancel: &CancelToken,
) -> Result<Tally, GitError> {
    let mut reader = ObjectReader::start(git, repository.work_tree()).await?;
    let mut tally = Tally::default();
    let rows = names.rows();
    let mut start = 0;
    while start < rows && !cancel.is_cancelled() {
        let end = start.saturating_add(BLOCK_ROWS).min(rows);
        let ids: Vec<ObjectId> = (start..end).filter_map(|row| names.id(row)).collect();
        reader
            .read(&ids, |_, object| {
                if let Some(object) = object.filter(|object| object.kind == ObjectKind::Commit) {
                    tally.add(object.data);
                }
            })
            .await?;
        read.fetch_add(end.saturating_sub(start), Ordering::Relaxed);
        start = end;
    }
    Ok(tally)
}

/// Someone, as the mailmap names them, while their contacts are gathered.
#[derive(Debug, Default)]
struct Person {
    name: String,
    email: String,
    /// The commits of the contact their name was taken from (the busiest).
    named_by: u64,
    commits: u64,
    first: Option<i64>,
    last: Option<i64>,
    months: HashSet<i32>,
}

/// The statistics a tally makes: its contacts through the mailmap, as people by email.
async fn finish(
    git: &Git,
    repository: &Repository,
    tally: Tally,
    cancel: &CancelToken,
) -> Result<Stats, GitError> {
    let mut names = tally.names;
    let mut contacts: Vec<(Vec<u8>, Contact)> = tally
        .contacts
        .into_iter()
        .filter_map(|(hash, contact)| Some((names.remove(&hash)?, contact)))
        .collect();
    drop(names);
    for (_, contact) in &mut contacts {
        contact.settle();
    }
    let keys: Vec<&BStr> = contacts.iter().map(|(key, _)| BStr::new(key)).collect();
    let mapped = map_contacts(git, repository, &keys, cancel).await?;
    let mut people: HashMap<String, Person> = HashMap::new();
    for ((_, contact), mapped) in contacts.into_iter().zip(mapped) {
        let mapped = mapped.to_str_lossy();
        let (name, email) = mapped
            .rsplit_once(" <")
            .map_or((&*mapped, ""), |(name, email)| {
                (name, email.trim_end_matches('>'))
            });
        let person = people.entry(email.to_lowercase()).or_default();
        if contact.commits > person.named_by {
            person.named_by = contact.commits;
            name.clone_into(&mut person.name);
            email.clone_into(&mut person.email);
        }
        person.commits = person.commits.saturating_add(contact.commits);
        person.first = earliest(person.first, contact.first);
        person.last = latest(person.last, contact.last);
        person.months.extend(contact.months);
    }
    let mut active: HashMap<i32, u32> = HashMap::new();
    for person in people.values() {
        for &month in &person.months {
            let count = active.entry(month).or_insert(0);
            *count = count.saturating_add(1);
        }
    }
    let mut authors: Vec<AuthorStats> = people
        .into_values()
        .map(|person| AuthorStats {
            name: person.name,
            email: person.email,
            commits: person.commits,
            first: person.first.unwrap_or_default(),
            last: person.last.unwrap_or_default(),
        })
        .collect();
    authors.sort_unstable_by(|a, b| b.commits.cmp(&a.commits).then_with(|| a.name.cmp(&b.name)));
    let mut days: Vec<(i32, u32)> = tally.days.into_iter().collect();
    days.sort_unstable_by_key(|&(day, _)| day);
    let mut cache = tally.months;
    let mut months: Vec<(i32, u32)> = Vec::new();
    for &(day, commits) in &days {
        let Some(month) = month_cached(&mut cache, day) else {
            continue;
        };
        match months.last_mut() {
            Some((last, count)) if *last == month => *count = count.saturating_add(commits),
            _ => months.push((month, commits)),
        }
    }
    let mut active: Vec<(i32, u32)> = active.into_iter().collect();
    active.sort_unstable_by_key(|&(month, _)| month);
    Ok(Stats {
        commits: tally.commits,
        merges: tally.merges,
        first: tally.first,
        last: tally.last,
        days,
        months,
        punch: tally.punch,
        authors,
        active,
        files: None,
    })
}

/// The files at `HEAD`: by language, the largest (with sizes when `sizes`).
async fn file_stats(
    git: &Git,
    repository: &Repository,
    sizes: bool,
    cancel: &CancelToken,
) -> Result<FileStats, GitError> {
    let mut stats = FileStats::default();
    let mut languages: HashMap<&'static str, (u64, u64)> = HashMap::new();
    let mut largest: BinaryHeap<Reverse<(u64, String)>> = BinaryHeap::new();
    let mut bytes = 0_u64;
    tree_files(git, repository, "HEAD", sizes, cancel, |path, size| {
        stats.files = stats.files.saturating_add(1);
        let size = size.unwrap_or(0);
        bytes = bytes.saturating_add(size);
        let language = languages.entry(language_of(path)).or_default();
        language.0 = language.0.saturating_add(1);
        language.1 = language.1.saturating_add(size);
        if sizes
            && (largest.len() < LARGEST
                || largest
                    .peek()
                    .is_some_and(|Reverse((least, _))| size > *least))
        {
            largest.push(Reverse((size, path.to_str_lossy().into_owned())));
            if largest.len() > LARGEST {
                largest.pop();
            }
        }
    })
    .await?;
    stats.bytes = sizes.then_some(bytes);
    let mut languages: Vec<Language> = languages
        .into_iter()
        .map(|(name, (files, bytes))| Language { name, files, bytes })
        .collect();
    languages.sort_unstable_by(|a, b| (b.bytes, b.files, a.name).cmp(&(a.bytes, a.files, b.name)));
    stats.languages = languages;
    let mut largest: Vec<(String, u64)> = largest
        .into_iter()
        .map(|Reverse((size, path))| (path, size))
        .collect();
    largest.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    stats.largest = largest;
    Ok(stats)
}

/// The language a file's name says it is in, as people call it ("Other" when it says none).
pub(crate) fn language_of(path: &[u8]) -> &'static str {
    let name = path.rsplit_str(b"/").next().unwrap_or(path);
    match name {
        b"Makefile" | b"makefile" | b"GNUmakefile" => return "Makefile",
        b"Dockerfile" | b"Containerfile" => return "Dockerfile",
        b"CMakeLists.txt" => return "CMake",
        b"Kconfig" => return "Kconfig",
        _ => {}
    }
    let Some((_, extension)) = name
        .rsplit_once_str(b".")
        .filter(|(stem, _)| !stem.is_empty())
    else {
        return "Other";
    };
    let extension = extension.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|(_, extensions)| extensions.contains(&extension.as_slice()))
        .map_or("Other", |(language, _)| language)
}

/// Languages by their files' extensions (in lower case).
const LANGUAGES: &[(&str, &[&[u8]])] = &[
    ("Rust", &[b"rs"]),
    ("C", &[b"c", b"h"]),
    (
        "C++",
        &[b"cc", b"cpp", b"cxx", b"hh", b"hpp", b"hxx", b"ipp"],
    ),
    ("C#", &[b"cs"]),
    ("Go", &[b"go"]),
    ("Python", &[b"py", b"pyi"]),
    ("JavaScript", &[b"js", b"mjs", b"cjs", b"jsx"]),
    ("TypeScript", &[b"ts", b"tsx", b"mts", b"cts"]),
    ("Java", &[b"java"]),
    ("Kotlin", &[b"kt", b"kts"]),
    ("Swift", &[b"swift"]),
    ("Objective-C", &[b"m", b"mm"]),
    ("Ruby", &[b"rb"]),
    ("PHP", &[b"php"]),
    ("Scala", &[b"scala", b"sc"]),
    ("Dart", &[b"dart"]),
    ("Elixir", &[b"ex", b"exs"]),
    ("Erlang", &[b"erl", b"hrl"]),
    ("Haskell", &[b"hs"]),
    ("OCaml", &[b"ml", b"mli"]),
    ("Clojure", &[b"clj", b"cljs", b"cljc"]),
    ("Lua", &[b"lua"]),
    ("Perl", &[b"pl", b"pm"]),
    ("R", &[b"r"]),
    ("Julia", &[b"jl"]),
    ("Zig", &[b"zig"]),
    ("Nim", &[b"nim"]),
    ("Shell", &[b"sh", b"bash", b"zsh", b"fish"]),
    ("PowerShell", &[b"ps1", b"psm1"]),
    ("Assembly", &[b"s", b"asm"]),
    ("HTML", &[b"html", b"htm"]),
    ("CSS", &[b"css", b"scss", b"sass", b"less"]),
    ("Vue", &[b"vue"]),
    ("Svelte", &[b"svelte"]),
    ("Markdown", &[b"md", b"markdown"]),
    ("reStructuredText", &[b"rst"]),
    ("Text", &[b"txt"]),
    ("JSON", &[b"json"]),
    ("YAML", &[b"yaml", b"yml"]),
    ("TOML", &[b"toml"]),
    ("XML", &[b"xml"]),
    ("SQL", &[b"sql"]),
    ("Protocol Buffers", &[b"proto"]),
    ("Device Tree", &[b"dts", b"dtsi"]),
    ("TeX", &[b"tex"]),
    ("Nix", &[b"nix"]),
    (
        "Images",
        &[
            b"png", b"jpg", b"jpeg", b"gif", b"webp", b"svg", b"ico", b"bmp",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &[u8] = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\n\
        parent 1111111111111111111111111111111111111111\n\
        parent 2222222222222222222222222222222222222222\n\
        author Ada Lovelace <ada@example.com> 1700000000 +0130\n\
        committer Ada Lovelace <ada@example.com> 1700000100 +0130\n\
        \n\
        Merge the parser\n";

    #[test]
    fn a_commit_s_headers_are_read() {
        let header = Header::parse(COMMIT).unwrap();
        assert_eq!(
            header,
            Header {
                parents: 2,
                contact: b"Ada Lovelace <ada@example.com>",
                time: 1_700_000_000,
                offset: 5_400,
            }
        );
        assert_eq!(zone_offset(b"-0530"), Some(-19_800));
        assert_eq!(Header::parse(b"tree x\n\nno author\n"), None);
    }

    #[test]
    fn commits_are_tallied_by_their_authors_clocks() {
        let mut tally = Tally::default();
        tally.add(COMMIT);
        // 1700000000 is Tuesday 14 November 2023, 22:13 UTC: 23:43 at +01:30.
        assert_eq!(tally.commits, 1);
        assert_eq!(tally.merges, 1);
        assert_eq!(tally.punch[1][23], 1, "a Tuesday, at 23:00");
        let day = day_of(1_700_000_000 + 5_400).unwrap();
        assert_eq!(tally.days.get(&day), Some(&1));
        assert_eq!(
            date_of(day).map(|date| date.to_string()),
            Some("2023-11-14".to_owned())
        );
        assert_eq!(month_of(day), Some((2023 - 1970) * 12 + 10));
        tally.add(COMMIT);
        assert_eq!(tally.commits, 2);
        assert_eq!(tally.contacts.len(), 1);
        assert_eq!(tally.contacts.values().next().unwrap().commits, 2);
        assert_eq!(
            tally.names.get(&fnv(b"Ada Lovelace <ada@example.com>")),
            Some(&b"Ada Lovelace <ada@example.com>".to_vec())
        );
    }

    #[test]
    fn streaks_and_the_busiest_day() {
        let stats = Stats {
            days: vec![(10, 1), (11, 4), (12, 1), (20, 4), (21, 2)],
            ..Stats::default()
        };
        assert_eq!(stats.streak(), Some((3, 12)));
        assert_eq!(stats.busiest(), Some((20, 4)), "the latest of the busiest");
    }

    #[test]
    fn languages_are_named_from_files_names() {
        assert_eq!(language_of(b"src/main.rs"), "Rust");
        assert_eq!(language_of(b"include/linux/list.H"), "C");
        assert_eq!(language_of(b"arch/arm/boot/dts/a.dtsi"), "Device Tree");
        assert_eq!(language_of(b"Makefile"), "Makefile");
        assert_eq!(
            language_of(b".gitignore"),
            "Other",
            "a dotfile has no extension"
        );
        assert_eq!(language_of(b"LICENSE"), "Other");
    }
}
