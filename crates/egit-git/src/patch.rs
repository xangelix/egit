//! Staging and unstaging some of a file's changed lines: the index's content edited in memory,
//! then written over the index's entry if it is still the one the diff was made from.
//!
//! [`apply_lines`] works on the index's blob and a diff of it (the index against the work tree,
//! to stage; `HEAD` against the index, to unstage). Lines picked move across; lines not picked
//! stay as the index has them. Every line the diff says the index holds is checked against it,
//! so a diff read before the file changed is refused rather than misapplied.
//! [`write_index_content`] then stores the result as a blob (`hash-object --no-filters`: it is
//! already in the form git stores) and puts it in the index (`update-index --index-info`),
//! after checking the index's entry is the one the diff was made from.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::diff::{DiffLine, DiffLineKind, FileDiff, HunkRanges};
use crate::error::GitError;
use crate::git::Git;
use crate::history::TreeEntry;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// Which way picked lines move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the work tree into the index: the diff is the index against the work tree.
    Stage,
    /// From the index back to `HEAD`: the diff is `HEAD` against the index.
    Unstage,
}

/// Staging or unstaging lines failed.
#[derive(Debug, thiserror::Error)]
pub enum LinesError {
    /// The file changed since its diff was read (in the index, or under the diff's lines).
    #[error("the file changed since its diff was read")]
    Stale,
    /// Running git failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// What a diff line does to the index's content.
#[derive(Clone, Copy, Debug)]
enum Op<'a> {
    /// A hunk starts: the base's lines before it are copied.
    Hunk(HunkRanges),
    /// Base line `number`, unchanged.
    Keep { text: &'a [u8], number: u32 },
    /// Base line `number`, which the change removes.
    Drop {
        text: &'a [u8],
        number: u32,
        picked: bool,
    },
    /// A line the change adds; `last` when it ends its file without a newline.
    Insert {
        text: &'a [u8],
        picked: bool,
        last: bool,
    },
}

/// `base` (the index's content) with the lines of `diff` that `picked` chooses (by their index
/// in `diff.lines`) moved across as `direction` says; the rest of the index is kept.
///
/// To stage, `diff` is the index against the work tree: picked added lines go in, picked removed
/// lines go out. To unstage, `diff` is `HEAD` against the index: picked added lines go out and
/// picked removed lines come back. A line left without a newline at the end of its file gains
/// one when picked lines follow it.
///
/// # Errors
///
/// [`LinesError::Stale`] when `diff` isn't of `base` (a line it keeps or removes isn't there).
pub fn apply_lines(
    base: &[u8],
    diff: &FileDiff,
    direction: Direction,
    picked: impl Fn(usize) -> bool,
) -> Result<Vec<u8>, LinesError> {
    let mut out = Output {
        bytes: Vec::with_capacity(base.len().saturating_add(diff.text.len())),
        open: false,
    };
    let mut cursor = Cursor {
        bytes: base,
        position: 0,
        next: 1,
    };
    let mut texts = diff.text.split_str(b"\n");
    for (index, line) in diff.lines.iter().enumerate() {
        let text = texts.next().ok_or(LinesError::Stale)?;
        match op(line, text, direction, picked(index)).ok_or(LinesError::Stale)? {
            Op::Hunk(ranges) => {
                let (start, count) = match direction {
                    Direction::Stage => (ranges.old_start, ranges.old_count),
                    Direction::Unstage => (ranges.new_start, ranges.new_count),
                };
                // With no lines of the base, a hunk's start is the line its lines follow.
                let before = if count == 0 {
                    start
                } else {
                    start.saturating_sub(1)
                };
                cursor.copy_through(before, &mut out)?;
            }
            Op::Keep { text, number } => {
                cursor.copy_through(number.saturating_sub(1), &mut out)?;
                let (line, newline) = cursor.take(text)?;
                out.line(line, newline);
            }
            Op::Drop {
                text,
                number,
                picked,
            } => {
                cursor.copy_through(number.saturating_sub(1), &mut out)?;
                let (line, newline) = cursor.take(text)?;
                if !picked {
                    out.line(line, newline);
                }
            }
            Op::Insert { text, picked, last } => {
                if picked {
                    out.line(text, !last);
                }
            }
        }
    }
    cursor.copy_rest(&mut out);
    Ok(out.bytes)
}

/// What `line` (its text `text`) does when applied in `direction`; `None` for a hunk header
/// that can't be read.
fn op<'a>(line: &DiffLine, text: &'a [u8], direction: Direction, picked: bool) -> Option<Op<'a>> {
    let (removes, adds, number) = match direction {
        Direction::Stage => (DiffLineKind::Removed, DiffLineKind::Added, line.old),
        Direction::Unstage => (DiffLineKind::Added, DiffLineKind::Removed, line.new),
    };
    Some(match line.kind {
        DiffLineKind::Hunk => Op::Hunk(HunkRanges::parse(text)?),
        DiffLineKind::Context => Op::Keep { text, number },
        kind if kind == removes => Op::Drop {
            text,
            number,
            picked,
        },
        kind if kind == adds => Op::Insert {
            text,
            picked,
            last: line.missing_newline,
        },
        _ => return None,
    })
}

/// The content being made.
struct Output {
    bytes: Vec<u8>,
    /// Whether the last line written lacks its newline (it gains one if another follows).
    open: bool,
}

impl Output {
    /// Writes a line, and its newline unless `newline` is false.
    fn line(&mut self, text: &[u8], newline: bool) {
        self.close();
        self.bytes.extend_from_slice(text);
        if newline {
            self.bytes.push(b'\n');
        }
        self.open = !newline;
    }

    /// Ends the last line, should more follow it.
    fn close(&mut self) {
        if std::mem::take(&mut self.open) {
            self.bytes.push(b'\n');
        }
    }
}

/// A place in the base content.
struct Cursor<'a> {
    bytes: &'a [u8],
    /// The byte the next line starts at.
    position: usize,
    /// The next line's number (from 1).
    next: u32,
}

impl<'a> Cursor<'a> {
    /// Copies the lines up to and including `last` to `out`, all at once.
    fn copy_through(&mut self, last: u32, out: &mut Output) -> Result<(), LinesError> {
        if last.saturating_add(1) < self.next {
            // A hunk over lines already passed: the diff isn't of this content.
            return Err(LinesError::Stale);
        }
        let start = self.position;
        while self.next <= last {
            let rest = self.bytes.get(self.position..).unwrap_or_default();
            if rest.is_empty() {
                return Err(LinesError::Stale);
            }
            self.position = rest.find_byte(b'\n').map_or(self.bytes.len(), |end| {
                self.position.saturating_add(end).saturating_add(1)
            });
            self.next = self.next.saturating_add(1);
        }
        let copied = self.bytes.get(start..self.position).unwrap_or_default();
        if !copied.is_empty() {
            out.close();
            out.bytes.extend_from_slice(copied);
            out.open = !copied.ends_with(b"\n");
        }
        Ok(())
    }

    /// Takes the next line, which must read `expected`; returns it and whether a newline ends
    /// it.
    fn take(&mut self, expected: &[u8]) -> Result<(&'a [u8], bool), LinesError> {
        let rest = self.bytes.get(self.position..).unwrap_or_default();
        if rest.is_empty() {
            return Err(LinesError::Stale);
        }
        let (line, newline) = rest.find_byte(b'\n').map_or((rest, false), |end| {
            (rest.get(..end).unwrap_or_default(), true)
        });
        if line != expected {
            return Err(LinesError::Stale);
        }
        self.position = self
            .position
            .saturating_add(line.len())
            .saturating_add(usize::from(newline));
        self.next = self.next.saturating_add(1);
        Ok((line, newline))
    }

    /// Copies whatever is left.
    fn copy_rest(&mut self, out: &mut Output) {
        let rest = self.bytes.get(self.position..).unwrap_or_default();
        if !rest.is_empty() {
            out.close();
            out.bytes.extend_from_slice(rest);
            out.open = !rest.ends_with(b"\n");
        }
        self.position = self.bytes.len();
    }
}

/// Puts `content` in the index at `path` with `mode`, if the index still holds `expected` there
/// (`None`: nothing, for a file not yet tracked). Returns the blob's name.
///
/// `content` is stored as it is (`hash-object --no-filters`): it must already be in the form git
/// stores, as content made from the index and a diff is.
///
/// # Errors
///
/// [`LinesError::Stale`] when the index holds something else at `path` (or a conflict), and
/// [`LinesError::Git`] when git fails or is cancelled.
pub fn write_index_content(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    expected: Option<TreeEntry>,
    mode: u32,
    content: Vec<u8>,
    cancel: &CancelToken,
) -> Result<ObjectId, LinesError> {
    let work_tree = repository.work_tree();
    let listed = git
        .read("ls-files")
        .args(["--stage", "-z", "--"])
        .arg(OsStr::from_bytes(path))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(work_tree)
        .cancel_token(cancel)
        .output()?;
    if index_entry(&listed.stdout)? != expected {
        return Err(LinesError::Stale);
    }
    let hashed = git
        .write("hash-object")
        .args(["-w", "--no-filters", "--stdin"])
        .current_dir(work_tree)
        .stdin(content)
        .cancel_token(cancel)
        .output()?;
    let id = ObjectId::from_hex(hashed.stdout.trim_ascii())
        .ok_or_else(|| GitError::unexpected("hash-object", "an object name"))?;
    let mut record = format!("{mode:o} {id} 0\t").into_bytes();
    record.extend_from_slice(path);
    record.push(b'\0');
    git.write("update-index")
        .args(["-z", "--index-info"])
        .current_dir(work_tree)
        .stdin(record)
        .cancel_token(cancel)
        .output()?;
    Ok(id)
}

/// The index's entry, from `ls-files --stage -z` for one path: `None` when there is none.
fn index_entry(listed: &[u8]) -> Result<Option<TreeEntry>, LinesError> {
    let mut records = listed.split_str(b"\0").filter(|record| !record.is_empty());
    let Some(record) = records.next() else {
        return Ok(None);
    };
    // More than one entry is a conflict: its sides are staged apart.
    if records.next().is_some() {
        return Err(LinesError::Stale);
    }
    let unexpected = || GitError::unexpected("ls-files", "an index entry");
    let (fields, _path) = record.split_once_str(b"\t").ok_or_else(unexpected)?;
    let mut fields = fields.split_str(b" ");
    let mode = fields
        .next()
        .and_then(|mode| u32::from_str_radix(mode.to_str().ok()?, 8).ok())
        .ok_or_else(unexpected)?;
    let id = fields
        .next()
        .and_then(ObjectId::from_hex)
        .ok_or_else(unexpected)?;
    if fields.next() != Some(b"0") {
        return Err(LinesError::Stale);
    }
    Ok(Some(TreeEntry { mode, id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A diff from a patch's lines (`@@`, ` `, `+`, `-`, and `\` for a missing newline).
    // The relaxations `clippy.toml` gives `#[test]` functions, for this helper.
    #[allow(clippy::arithmetic_side_effects, clippy::unwrap_used)]
    fn diff(patch: &str) -> FileDiff {
        let mut diff = FileDiff::default();
        let (mut old, mut new) = (0, 0);
        for line in patch.lines() {
            let (kind, text) = match line.split_at(1) {
                ("@", _) => {
                    let ranges = HunkRanges::parse(line.as_bytes()).unwrap();
                    (old, new) = (ranges.old_start, ranges.new_start);
                    (DiffLineKind::Hunk, line)
                }
                (" ", text) => (DiffLineKind::Context, text),
                ("+", text) => (DiffLineKind::Added, text),
                ("-", text) => (DiffLineKind::Removed, text),
                _ => {
                    diff.lines.last_mut().unwrap().missing_newline = true;
                    continue;
                }
            };
            let numbers = match kind {
                DiffLineKind::Hunk => (0, 0),
                DiffLineKind::Context => (old, new),
                DiffLineKind::Added => (0, new),
                DiffLineKind::Removed => (old, 0),
            };
            if matches!(kind, DiffLineKind::Context | DiffLineKind::Removed) {
                old += 1;
            }
            if matches!(kind, DiffLineKind::Context | DiffLineKind::Added) {
                new += 1;
            }
            diff.text.extend_from_slice(text.as_bytes());
            diff.text.push(b'\n');
            diff.lines.push(DiffLine {
                kind,
                missing_newline: false,
                old: numbers.0,
                new: numbers.1,
            });
        }
        diff
    }

    fn apply(base: &str, patch: &str, direction: Direction, picked: &[usize]) -> String {
        let result = apply_lines(base.as_bytes(), &diff(patch), direction, |index| {
            picked.contains(&index)
        })
        .unwrap();
        String::from_utf8(result).unwrap()
    }

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
    const PATCH: &str =
        "@@ -2,5 +2,5 @@\n two\n-three\n+THREE\n four\n-five\n+FIVE\n+five and a half\n six\n";

    #[test]
    fn stages_only_the_lines_picked() {
        assert_eq!(apply(BASE, PATCH, Direction::Stage, &[]), BASE);
        assert_eq!(
            apply(BASE, PATCH, Direction::Stage, &[2, 3]),
            "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n"
        );
        // A removal alone, and an addition alone.
        assert_eq!(
            apply(BASE, PATCH, Direction::Stage, &[5]),
            "one\ntwo\nthree\nfour\nsix\nseven\n"
        );
        assert_eq!(
            apply(BASE, PATCH, Direction::Stage, &[7]),
            "one\ntwo\nthree\nfour\nfive\nfive and a half\nsix\nseven\n"
        );
        assert_eq!(
            apply(BASE, PATCH, Direction::Stage, &[2, 3, 5, 6, 7]),
            "one\ntwo\nTHREE\nfour\nFIVE\nfive and a half\nsix\nseven\n"
        );
    }

    #[test]
    fn unstages_by_taking_picked_lines_back_out() {
        // The index has every change; `HEAD` had none.
        let index = "one\ntwo\nTHREE\nfour\nFIVE\nfive and a half\nsix\nseven\n";
        assert_eq!(apply(index, PATCH, Direction::Unstage, &[]), index);
        assert_eq!(
            apply(index, PATCH, Direction::Unstage, &[7]),
            "one\ntwo\nTHREE\nfour\nFIVE\nsix\nseven\n"
        );
        assert_eq!(
            apply(index, PATCH, Direction::Unstage, &[2, 3, 5, 6, 7]),
            BASE
        );
    }

    #[test]
    fn a_new_file_takes_only_its_picked_lines() {
        let patch = "@@ -0,0 +1,3 @@\n+a\n+b\n+c\n";
        assert_eq!(apply("", patch, Direction::Stage, &[1, 3]), "a\nc\n");
        assert_eq!(apply("", patch, Direction::Stage, &[]), "");
    }

    #[test]
    fn a_last_line_without_a_newline_gains_one_when_lines_follow() {
        // `b` lost its newline's absence: `-b`, `\`, `+b`, `+c`, `\`.
        let patch = "@@ -1,2 +1,3 @@\n a\n-b\n\\ No newline at end of file\n+b\n+c\n\\ No newline at end of file\n";
        let base = "a\nb";
        assert_eq!(apply(base, patch, Direction::Stage, &[]), base);
        assert_eq!(apply(base, patch, Direction::Stage, &[2, 3, 4]), "a\nb\nc");
        // Only `c`: the old `b` stays, and gains a newline since `c` follows it.
        assert_eq!(apply(base, patch, Direction::Stage, &[4]), "a\nb\nc");
        // Only the removal: `b` goes, `a` keeps its newline.
        assert_eq!(apply(base, patch, Direction::Stage, &[2]), "a\n");
    }

    #[test]
    fn a_diff_of_other_content_is_stale() {
        let picked = |_| true;
        assert!(matches!(
            apply_lines(b"one\nTWO\nthree\n", &diff(PATCH), Direction::Stage, picked),
            Err(LinesError::Stale)
        ));
        assert!(matches!(
            apply_lines(b"one\n", &diff(PATCH), Direction::Stage, picked),
            Err(LinesError::Stale)
        ));
    }

    #[test]
    fn reads_an_index_entry() {
        let id = "ce013625030ba8dba906f756967f9e9ca394464a";
        let entry = index_entry(format!("100755 {id} 0\tsome path\0").as_bytes()).unwrap();
        assert_eq!(entry.unwrap().mode, 0o100_755);
        assert_eq!(index_entry(b"").unwrap(), None);
        let conflict = format!("100644 {id} 1\tf\0100644 {id} 2\tf\0");
        assert!(matches!(
            index_entry(conflict.as_bytes()),
            Err(LinesError::Stale)
        ));
    }
}
