//! A file in conflict, and resolving it.
//!
//! When a merge, rebase, cherry-pick, or revert stops at a conflict, the index keeps the file's
//! sides apart (`ls-files -u`): stage 1 is the base both sides started from, 2 the current side
//! (`HEAD`; git's "ours"), 3 the incoming one (the commit merged, picked, reverted, or
//! replayed; "theirs"). A side is missing where it deleted the file, and the base where both
//! sides added it.
//!
//! [`conflict_sides`] reads them. [`merge_sides`] merges two texts as `git merge` does
//! (`merge-file`, with the histogram diff where git has it), each conflict's lines found between
//! markers longer than any run of marker characters starting a line of the sides, so no line of
//! the file can pass for one; the result is git's output and ranges into it. [`resolve_conflict`]
//! saves a resolution through git.

use std::ffi::OsStr;
use std::ops::Range;
use std::os::unix::ffi::OsStrExt as _;

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::history::TreeEntry;
use crate::object_id::ObjectId;
use crate::objects::ObjectReader;
use crate::repo::{Operation, Repository};
use crate::version::GitVersion;

/// The shortest conflict marker git writes.
const MIN_MARKER: usize = 7;

/// How much of a file git looks at to tell whether it is binary (a NUL byte in it).
const BINARY_PROBE: usize = 8000;

/// The first git whose `merge-file` takes `--diff-algorithm`.
const MERGE_FILE_ALGORITHM: GitVersion = GitVersion::new(2, 44, 0);

/// A symbolic link's mode.
const LINK: u32 = 0o120_000;

/// The sides of a file in conflict, as the index holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConflictSides {
    /// What both sides started from (stage 1); missing where both added the file.
    pub base: Option<TreeEntry>,
    /// The current side (stage 2, `HEAD`'s); missing where it deleted the file.
    pub current: Option<TreeEntry>,
    /// The incoming side (stage 3); missing where it deleted the file.
    pub incoming: Option<TreeEntry>,
}

impl ConflictSides {
    /// The mode a merge of the two sides keeps: the side that changed it from the base's (the
    /// current side's when both did, or there is no base).
    #[must_use]
    pub fn merged_mode(&self) -> Option<u32> {
        let current = self.current?.mode;
        let incoming = self.incoming?.mode;
        let base = self.base.map(|base| base.mode);
        Some(if current == incoming || base != Some(current) {
            current
        } else {
            incoming
        })
    }

    /// Why only a whole side resolves it (or deleting the file), if one does, before reading
    /// the sides' contents.
    fn whole_only(&self) -> Option<WholeOnly> {
        let (Some(current), Some(incoming)) = (self.current, self.incoming) else {
            return Some(WholeOnly::Deleted);
        };
        let sides = [Some(current), Some(incoming), self.base];
        let any = |mode| sides.iter().flatten().any(|side| side.mode == mode);
        if any(TreeEntry::SUBMODULE) {
            Some(WholeOnly::Submodule)
        } else if any(LINK) {
            Some(WholeOnly::Link)
        } else {
            None
        }
    }
}

/// Reads the sides of the conflict at `path`: `None` when the index has none there (it is
/// resolved, or was never in conflict).
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't an index entry.
pub fn conflict_sides(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    cancel: &CancelToken,
) -> Result<Option<ConflictSides>, GitError> {
    let listed = git
        .read("ls-files")
        .args(["-u", "-z", "--"])
        .arg(OsStr::from_bytes(path))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    parse_unmerged(&listed.stdout, path)
}

/// The sides listed for `path` in `ls-files -u -z`'s output (`<mode> <object> <stage>\t<path>`;
/// a folder's path would list the files under it, which aren't it).
fn parse_unmerged(listed: &[u8], path: &BStr) -> Result<Option<ConflictSides>, GitError> {
    let unexpected = || GitError::unexpected("ls-files", "an unmerged index entry");
    let mut sides = ConflictSides::default();
    let mut found = false;
    for record in listed.split_str(b"\0").filter(|record| !record.is_empty()) {
        let (fields, listed_path) = record.split_once_str(b"\t").ok_or_else(unexpected)?;
        if listed_path != path.as_bytes() {
            continue;
        }
        let mut fields = fields.split_str(b" ");
        let mode = fields
            .next()
            .and_then(|mode| u32::from_str_radix(mode.to_str().ok()?, 8).ok())
            .ok_or_else(unexpected)?;
        let id = fields
            .next()
            .and_then(ObjectId::from_hex)
            .ok_or_else(unexpected)?;
        let side = match fields.next() {
            Some(b"1") => &mut sides.base,
            Some(b"2") => &mut sides.current,
            Some(b"3") => &mut sides.incoming,
            _ => return Err(unexpected()),
        };
        *side = Some(TreeEntry { mode, id });
        found = true;
    }
    Ok(found.then_some(sides))
}

/// Why a conflict is resolved only by keeping a whole side, or deleting the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WholeOnly {
    /// A side deleted the file.
    Deleted,
    /// It is a submodule on some side.
    Submodule,
    /// It is a symbolic link on some side.
    Link,
    /// A side's contents are binary (a NUL byte among its first 8,000, as git tells).
    Binary,
}

/// A conflict's sides merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Two texts: what merged, and each conflict's lines.
    Text(MergedText),
    /// Something only a whole side resolves.
    WholeOnly(WholeOnly),
}

/// Two texts merged as `merge-file` merges them: git's output, and where in it each piece is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedText {
    /// `merge-file`'s output: what merged, and each conflict's sides between markers.
    text: Vec<u8>,
    pieces: Vec<Piece>,
    /// Which sides' last lines have no newline (git gives one to a side ending before a
    /// marker): the base's, the current side's, and the incoming one's.
    open_ends: [bool; 3],
}

/// A stretch of a merged text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Lines that merged (bytes of [`MergedText::bytes`]).
    Merged(Range<u32>),
    /// A conflict.
    Conflict(ConflictLines),
}

/// A conflict's lines on each side (bytes of [`MergedText::bytes`]), each ending with a newline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictLines {
    /// The current side's.
    pub current: Range<u32>,
    /// The base's (what git calls the "zealous" diff3 base: lines both sides kept are outside).
    pub base: Range<u32>,
    /// The incoming side's.
    pub incoming: Range<u32>,
}

/// What a conflict is resolved with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    /// The current side's lines.
    Current,
    /// The incoming side's.
    Incoming,
    /// The base's (neither side's change).
    Base,
    /// The current side's, then the incoming side's.
    CurrentThenIncoming,
    /// The incoming side's, then the current side's.
    IncomingThenCurrent,
    /// Text of the user's own, as it is.
    Text(&'a [u8]),
}

/// A side of a conflict's lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Base,
    Current,
    Incoming,
}

impl Side {
    /// Its place in the base, current, incoming order.
    const fn index(self) -> usize {
        match self {
            Self::Base => 0,
            Self::Current => 1,
            Self::Incoming => 2,
        }
    }
}

impl MergedText {
    /// Its pieces, in order.
    #[must_use]
    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// The bytes of a piece's range.
    #[must_use]
    pub fn bytes(&self, range: &Range<u32>) -> &[u8] {
        let (Ok(start), Ok(end)) = (usize::try_from(range.start), usize::try_from(range.end))
        else {
            return &[];
        };
        self.text.get(start..end).unwrap_or_default()
    }

    /// How many conflicts it has.
    #[must_use]
    pub fn conflicts(&self) -> usize {
        self.pieces
            .iter()
            .filter(|piece| matches!(piece, Piece::Conflict(_)))
            .count()
    }

    /// What conflict `index` is resolved with when `pick` is taken: its bytes exactly (a last
    /// line without a newline keeps it so). `None` past the last conflict.
    #[must_use]
    pub fn picked(&self, index: usize, pick: Pick<'_>) -> Option<Vec<u8>> {
        let (lines, last) = self.conflict(index)?;
        let mut out = Vec::new();
        self.extend_picked(&mut out, lines, last, pick);
        Some(out)
    }

    /// The file resolved: what merged, and each conflict as `pick` picks it (by its index).
    #[must_use]
    pub fn resolve<'a>(&self, mut pick: impl FnMut(usize) -> Pick<'a>) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.text.len());
        let mut index = 0_usize;
        let count = self.pieces.len();
        for (at, piece) in self.pieces.iter().enumerate() {
            match piece {
                Piece::Merged(range) => out.extend_from_slice(self.bytes(range)),
                Piece::Conflict(lines) => {
                    let last = at.saturating_add(1) == count;
                    self.extend_picked(&mut out, lines, last, pick(index));
                    index = index.saturating_add(1);
                }
            }
        }
        out
    }

    /// Conflict `index`'s lines, and whether nothing follows it.
    fn conflict(&self, index: usize) -> Option<(&ConflictLines, bool)> {
        let count = self.pieces.len();
        self.pieces
            .iter()
            .enumerate()
            .filter_map(|(at, piece)| match piece {
                Piece::Conflict(lines) => Some((lines, at.saturating_add(1) == count)),
                Piece::Merged(_) => None,
            })
            .nth(index)
    }

    fn extend_picked(&self, out: &mut Vec<u8>, lines: &ConflictLines, last: bool, pick: Pick<'_>) {
        let parts: &[Side] = match pick {
            Pick::Text(text) => {
                out.extend_from_slice(text);
                return;
            }
            Pick::Current => &[Side::Current],
            Pick::Incoming => &[Side::Incoming],
            Pick::Base => &[Side::Base],
            Pick::CurrentThenIncoming => &[Side::Current, Side::Incoming],
            Pick::IncomingThenCurrent => &[Side::Incoming, Side::Current],
        };
        let range = |side| match side {
            Side::Base => &lines.base,
            Side::Current => &lines.current,
            Side::Incoming => &lines.incoming,
        };
        for &side in parts {
            out.extend_from_slice(self.bytes(range(side)));
        }
        // The file's end: the side whose lines come last ends as that side's file does (git
        // gave a last line without a newline one, to end it before a marker).
        let ends_with = parts
            .iter()
            .rev()
            .find(|&&side| !self.bytes(range(side)).is_empty());
        if last
            && let Some(&side) = ends_with
            && self.open_ends.get(side.index()).copied().unwrap_or(false)
            && out.last() == Some(&b'\n')
        {
            out.pop();
        }
    }
}

/// What reading a side found: whether it is binary, whether its last line has no newline, and
/// its longest run of a marker character starting a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Scanned {
    binary: bool,
    open_end: bool,
    longest_run: usize,
}

impl Scanned {
    fn of(data: &[u8]) -> Self {
        let probe = data.get(..BINARY_PROBE).unwrap_or(data);
        let mut longest_run = 0;
        for line in data.lines_with_terminator() {
            let Some(&first) = line.first() else {
                continue;
            };
            if matches!(first, b'<' | b'=' | b'>' | b'|') {
                let run = line.iter().take_while(|&&byte| byte == first).count();
                longest_run = longest_run.max(run);
            }
        }
        Self {
            binary: probe.contains(&0),
            open_end: data.last().is_some_and(|&last| last != b'\n'),
            longest_run,
        }
    }
}

/// Merges `sides` as `git merge` merges two texts; a conflict that isn't two texts (a side
/// deleted, a symbolic link, a submodule, or binary contents) is [`Merge::WholeOnly`].
///
/// # Errors
///
/// When git fails or is cancelled, a side's object is missing, or the merge is over 4 GiB.
pub fn merge_sides(
    git: &Git,
    repository: &Repository,
    sides: &ConflictSides,
    cancel: &CancelToken,
) -> Result<Merge, GitError> {
    if let Some(reason) = sides.whole_only() {
        return Ok(Merge::WholeOnly(reason));
    }
    let (Some(current), Some(incoming)) = (sides.current, sides.incoming) else {
        return Ok(Merge::WholeOnly(WholeOnly::Deleted));
    };
    let work_tree = repository.work_tree();
    let base = match sides.base {
        Some(base) => base.id,
        // Added on both sides: merged as git merges them, from nothing.
        None => empty_blob(git, repository, cancel)?,
    };
    // Base, current, incoming.
    let ids = [base, current.id, incoming.id];
    let mut scanned = [None; 3];
    let mut reader = ObjectReader::start(git, work_tree)?;
    reader.read(&ids, |id, object| {
        let Some(object) = object else {
            return;
        };
        // The same object may be two sides: each answer fills the first side still empty.
        if let Some(slot) = ids
            .iter()
            .zip(scanned.iter_mut())
            .find_map(|(side, slot)| (*side == id && slot.is_none()).then_some(slot))
        {
            *slot = Some(Scanned::of(object.data));
        }
    })?;
    let mut sides_scanned = [Scanned::default(); 3];
    for (slot, found) in sides_scanned.iter_mut().zip(scanned) {
        *slot = found.ok_or_else(|| GitError::unexpected("cat-file", "the conflict's sides"))?;
    }
    if sides_scanned.iter().any(|side| side.binary) {
        return Ok(Merge::WholeOnly(WholeOnly::Binary));
    }
    let longest = sides_scanned
        .iter()
        .map(|side| side.longest_run)
        .max()
        .unwrap_or(0);
    let marker = longest.saturating_add(1).max(MIN_MARKER);
    let mut command = git
        .read("merge-file")
        .args(["--object-id", "-p", "--zdiff3", "-q"])
        .arg(format!("--marker-size={marker}"));
    if git.version() >= MERGE_FILE_ALGORITHM {
        // As `git merge` diffs.
        command = command.arg("--diff-algorithm=histogram");
    }
    let merged = command
        .args(["-L", "current", "-L", "base", "-L", "incoming"])
        .arg(current.id.to_string())
        .arg(base.to_string())
        .arg(incoming.id.to_string())
        .current_dir(work_tree)
        .stdout_limit(usize::try_from(u32::MAX).unwrap_or(usize::MAX))
        .cancel_token(cancel)
        .output_unchecked()?;
    // The exit code counts the conflicts (up to 127); anything above is an error.
    let conflicts = match merged.status.code() {
        Some(code @ 0..=127) => usize::try_from(code).unwrap_or(0),
        _ => {
            return Err(GitError::failed("merge-file", merged.status, merged.stderr));
        }
    };
    let pieces = parse_merged(&merged.stdout, marker)
        .ok_or_else(|| GitError::unexpected("merge-file", "a merge with conflict markers"))?;
    let text = MergedText {
        text: merged.stdout,
        pieces,
        open_ends: sides_scanned.map(|side| side.open_end),
    };
    if conflicts < 127 && text.conflicts() != conflicts {
        return Err(GitError::unexpected(
            "merge-file",
            format!("{conflicts} conflicts, as its exit code says"),
        ));
    }
    Ok(Merge::Text(text))
}

/// The empty blob's name (added to the repository: git doesn't know it otherwise).
fn empty_blob(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<ObjectId, GitError> {
    let hashed = git
        .write("hash-object")
        .args(["-w", "--stdin"])
        .current_dir(repository.work_tree())
        .stdin(Vec::new())
        .cancel_token(cancel)
        .output()?;
    ObjectId::from_hex(hashed.stdout.trim_ascii())
        .ok_or_else(|| GitError::unexpected("hash-object", "an object name"))
}

/// Where the pieces of `merge-file`'s output are, its markers `marker` characters long: `None`
/// when they aren't in order.
fn parse_merged(text: &[u8], marker: usize) -> Option<Vec<Piece>> {
    /// Where the parse is.
    enum At {
        Merged(u32),
        Current(u32),
        Base(Range<u32>, u32),
        Incoming(Range<u32>, Range<u32>, u32),
    }
    let is_marker = |line: &[u8], of: u8| {
        line.get(..marker)
            .is_some_and(|run| run.iter().all(|&byte| byte == of))
            && matches!(line.get(marker), None | Some(b' ' | b'\n' | b'\r'))
    };
    let mut pieces = Vec::new();
    let mut at = At::Merged(0);
    let mut start = 0_u32;
    for line in text.lines_with_terminator() {
        let end = start.checked_add(u32::try_from(line.len()).ok()?)?;
        at = match at {
            At::Merged(from) if is_marker(line, b'<') => {
                if from < start {
                    pieces.push(Piece::Merged(from..start));
                }
                At::Current(end)
            }
            At::Current(from) if is_marker(line, b'|') => At::Base(from..start, end),
            At::Current(from) if is_marker(line, b'=') => {
                At::Incoming(from..start, start..start, end)
            }
            At::Base(current, from) if is_marker(line, b'=') => {
                At::Incoming(current, from..start, end)
            }
            At::Incoming(current, base, from) if is_marker(line, b'>') => {
                pieces.push(Piece::Conflict(ConflictLines {
                    current,
                    base,
                    incoming: from..start,
                }));
                At::Merged(end)
            }
            at => at,
        };
        start = end;
    }
    let At::Merged(from) = at else {
        return None;
    };
    if from < start {
        pieces.push(Piece::Merged(from..start));
    }
    Some(pieces)
}

/// How a conflict is resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// New contents, in the form git stores them, with a mode.
    Content {
        /// The contents.
        content: Vec<u8>,
        /// The mode (see [`ConflictSides::merged_mode`]).
        mode: u32,
    },
    /// A side as it is (a submodule's commit, a symbolic link, or a file).
    Side(TreeEntry),
    /// The file deleted.
    Delete,
}

/// Why a conflict couldn't be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// The index no longer holds the sides the resolution was made from.
    #[error("the conflict changed since it was read")]
    Stale,
    /// Git failed or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Resolves the conflict at `path`, if the index still holds `expected` there.
///
/// `resolution` becomes the index's entry (the conflict's stages go) and the work tree's file
/// is written from it (`checkout-index`, through the repository's filters); or the file is
/// deleted.
///
/// # Errors
///
/// [`ResolveError::Stale`] when the index holds other sides, or none, and
/// [`ResolveError::Git`] when git fails or is cancelled.
pub fn resolve_conflict(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    expected: &ConflictSides,
    resolution: Resolution,
    cancel: &CancelToken,
) -> Result<(), ResolveError> {
    if conflict_sides(git, repository, path, cancel)?.as_ref() != Some(expected) {
        return Err(ResolveError::Stale);
    }
    let work_tree = repository.work_tree();
    let entry = match resolution {
        Resolution::Delete => {
            git.write("rm")
                .args(["--quiet", "--force", "--"])
                .arg(OsStr::from_bytes(path))
                .env("GIT_LITERAL_PATHSPECS", "1")
                .current_dir(work_tree)
                .cancel_token(cancel)
                .output()?;
            return Ok(());
        }
        Resolution::Side(entry) => entry,
        Resolution::Content { content, mode } => {
            let hashed = git
                .write("hash-object")
                .args(["-w", "--no-filters", "--stdin"])
                .current_dir(work_tree)
                .stdin(content)
                .cancel_token(cancel)
                .output()?;
            let id = ObjectId::from_hex(hashed.stdout.trim_ascii())
                .ok_or_else(|| GitError::unexpected("hash-object", "an object name"))?;
            TreeEntry { mode, id }
        }
    };
    // A merged entry (stage 0) replaces the conflict's stages.
    let mut record = format!("{:o} {} 0\t", entry.mode, entry.id).into_bytes();
    record.extend_from_slice(path);
    record.push(b'\0');
    git.write("update-index")
        .args(["-z", "--index-info"])
        .current_dir(work_tree)
        .stdin(record)
        .cancel_token(cancel)
        .output()?;
    let mut paths = path.to_vec();
    paths.push(b'\0');
    git.write("checkout-index")
        .args(["--force", "-z", "--stdin"])
        .current_dir(work_tree)
        .stdin(paths)
        .cancel_token(cancel)
        .output()?;
    Ok(())
}

/// The commit an operation in progress brings in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingCommit {
    /// Its name.
    pub id: ObjectId,
    /// Its subject (`%s`).
    pub subject: String,
}

/// The commit `operation` brings in, if git keeps one.
///
/// The one merged (`MERGE_HEAD`; the first, merging several), picked (`CHERRY_PICK_HEAD`),
/// reverted (`REVERT_HEAD`), or replayed (`REBASE_HEAD`); `None` for other operations.
///
/// # Errors
///
/// When git can't be run or is cancelled.
pub fn incoming_commit(
    git: &Git,
    repository: &Repository,
    operation: Operation,
    cancel: &CancelToken,
) -> Result<Option<IncomingCommit>, GitError> {
    let name = match operation {
        Operation::Merge => "MERGE_HEAD",
        Operation::CherryPick => "CHERRY_PICK_HEAD",
        Operation::Revert => "REVERT_HEAD",
        Operation::Rebase => "REBASE_HEAD",
        Operation::ApplyMailbox | Operation::Bisect => return Ok(None),
    };
    let shown = git
        .read("log")
        .config("log.showSignature", "false")
        .args(["-1", "--no-walk", "--format=%H%x00%s", name, "--"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    if !shown.status.success() {
        return Ok(None);
    }
    let line = shown.stdout.trim_end_with(|c| c == '\n');
    let Some((id, subject)) = line.split_once_str(b"\0") else {
        return Ok(None);
    };
    Ok(ObjectId::from_hex(id).map(|id| IncomingCommit {
        id,
        subject: subject.to_str_lossy().into_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(text: &str, open_ends: [bool; 3]) -> MergedText {
        let pieces = parse_merged(text.as_bytes(), 7).unwrap_or_default();
        MergedText {
            text: text.as_bytes().to_vec(),
            pieces,
            open_ends,
        }
    }

    const TWO: &str = "a\n<<<<<<< current\nB1\n||||||| base\nb\n=======\nB2\n>>>>>>> incoming\n\
                       c\n<<<<<<< current\nE1\n||||||| base\ne\n=======\nE2\n>>>>>>> incoming\n";

    #[test]
    fn finds_each_conflicts_sides() {
        let text = merged(TWO, [false; 3]);
        assert_eq!(text.conflicts(), 2);
        let pieces = text.pieces();
        assert!(matches!(&pieces[0], Piece::Merged(range) if text.bytes(range) == b"a\n"));
        let Piece::Conflict(first) = &pieces[1] else {
            panic!("{pieces:?}");
        };
        assert_eq!(text.bytes(&first.current), b"B1\n");
        assert_eq!(text.bytes(&first.base), b"b\n");
        assert_eq!(text.bytes(&first.incoming), b"B2\n");
        assert!(matches!(&pieces[2], Piece::Merged(range) if text.bytes(range) == b"c\n"));
        assert_eq!(pieces.len(), 4);
        // Without a base section (git's "merge" style), and with an empty side.
        let plain = merged("<<<<<<< x\n=======\nnew\n>>>>>>> y\n", [false; 3]);
        let Piece::Conflict(lines) = &plain.pieces()[0] else {
            panic!();
        };
        assert!(lines.current.is_empty() && lines.base.is_empty());
        assert_eq!(plain.bytes(&lines.incoming), b"new\n");
        // Markers out of order aren't a merge.
        assert_eq!(parse_merged(b"<<<<<<< x\na\n", 7), None);
        // A shorter run is a line of the file.
        let content = merged("<<<<<< not a marker\n", [false; 3]);
        assert_eq!(content.conflicts(), 0);
    }

    #[test]
    fn resolves_each_conflict_as_picked() {
        let text = merged(TWO, [false; 3]);
        let picks = [Pick::Current, Pick::IncomingThenCurrent];
        assert_eq!(
            text.resolve(|index| picks[index]),
            b"a\nB1\nc\nE2\nE1\n".to_vec()
        );
        assert_eq!(
            text.resolve(|_| Pick::Text(b"x\n")),
            b"a\nx\nc\nx\n".to_vec()
        );
        assert_eq!(text.picked(0, Pick::Base), Some(b"b\n".to_vec()));
        assert_eq!(text.picked(2, Pick::Base), None);
    }

    #[test]
    fn a_last_line_without_a_newline_keeps_it_so() {
        // The current side's file ends "E1" and the incoming side's "E2\n".
        let text = merged(TWO, [false, true, false]);
        assert_eq!(text.resolve(|_| Pick::Current), b"a\nB1\nc\nE1".to_vec());
        assert_eq!(text.resolve(|_| Pick::Incoming), b"a\nB2\nc\nE2\n".to_vec());
        assert_eq!(
            text.picked(1, Pick::CurrentThenIncoming),
            Some(b"E1\nE2\n".to_vec())
        );
        assert_eq!(
            text.picked(1, Pick::IncomingThenCurrent),
            Some(b"E2\nE1".to_vec())
        );
        // Only at the file's end.
        assert_eq!(text.picked(0, Pick::Current), Some(b"B1\n".to_vec()));
    }

    #[test]
    fn reads_the_sides_listed() {
        let id = |byte: char| byte.to_string().repeat(40);
        let listed = format!(
            "100644 {} 1\tf\0100755 {} 2\tf\0100644 {} 3\tf\0100644 {} 2\tf/g\0",
            id('a'),
            id('b'),
            id('c'),
            id('d')
        );
        let sides = parse_unmerged(listed.as_bytes(), BStr::new("f")).unwrap();
        let sides = sides.unwrap();
        assert_eq!(sides.current.map(|side| side.mode), Some(0o100_755));
        assert_eq!(sides.base.map(|side| side.id.to_string()), Some(id('a')));
        // The base's mode, changed by the current side alone: the current side's is kept.
        assert_eq!(sides.merged_mode(), Some(0o100_755));
        assert_eq!(parse_unmerged(b"", BStr::new("f")).unwrap(), None);
        let deleted = ConflictSides {
            current: None,
            ..sides
        };
        assert_eq!(deleted.whole_only(), Some(WholeOnly::Deleted));
    }

    #[test]
    fn scans_for_marker_runs_and_binary_contents() {
        let scanned = Scanned::of(b"x\n<<<<<<<<<< long\n=== short\nno newline");
        assert_eq!(scanned.longest_run, 10);
        assert!(scanned.open_end && !scanned.binary);
        assert!(Scanned::of(b"a\0b").binary);
        assert!(!Scanned::of(b"").open_end);
    }
}
