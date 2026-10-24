//! One file's changes as git shows them, parsed into lines ready to display.
//!
//! A commit's patch comes from `git diff-tree -p` (plumbing: no colour, pager, external diff,
//! textconv, or prefix settings apply); uncommitted changes from `git diff`, its every setting
//! that changes the patch pinned. Either is parsed as it streams: each line is kept without its
//! `+`, `-`, or ` ` prefix, numbered in the old and new file, so the raw patch is never held
//! whole.

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::command::GitCommand;
use crate::error::GitError;
use crate::git::Git;
use crate::lines::Lines;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// Context lines that mean "the whole file": far more than any file git diffs has.
pub const WHOLE_FILE: u32 = 1_000_000_000;

/// What a line of a diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// A hunk's header: `@@ -12,7 +12,9 @@` and the function it is in.
    Hunk,
    /// Unchanged, shown for context.
    Context,
    /// Added.
    Added,
    /// Removed.
    Removed,
}

/// A line of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// What it is.
    pub kind: DiffLineKind,
    /// Whether the file ends with it, without a newline (`\ No newline at end of file`).
    pub missing_newline: bool,
    /// Its number in the old file (from 1; 0 for an added line or a hunk header).
    pub old: u32,
    /// Its number in the new file (from 1; 0 for a removed line or a hunk header).
    pub new: u32,
}

/// One file's diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    /// Each line's text, without its `+`, `-`, or ` `, ended by `\n` (a hunk's, its header).
    pub text: Vec<u8>,
    /// Each line, in order.
    pub lines: Vec<DiffLine>,
    /// Whether git found the file binary (it has no lines then).
    pub binary: bool,
    /// The blobs it is between, as its `index` line names them (a work tree's side as git
    /// would store it: through its clean filters); `None` without one (a change of mode alone),
    /// or abbreviated.
    pub blobs: Option<(ObjectId, ObjectId)>,
}

/// How a diff is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffOptions {
    /// Unchanged lines shown around each change ([`WHOLE_FILE`] for all of them).
    pub context: u32,
    /// The diff algorithm (`myers`, `minimal`, `patience`, `histogram`), when the user chose one
    /// (`diff.algorithm`, which plumbing doesn't read).
    pub algorithm: Option<String>,
    /// Lines differing only in whitespace count as the same (`--ignore-all-space`): such a
    /// diff can't be applied back, so lines are never staged from it.
    pub ignore_whitespace: bool,
}

impl Default for DiffOptions {
    /// Three lines of context, as git shows by default.
    fn default() -> Self {
        Self {
            context: 3,
            algorithm: None,
            ignore_whitespace: false,
        }
    }
}

/// Which of a file's uncommitted changes a diff shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uncommitted {
    /// The work tree against the index: what isn't staged.
    Unstaged,
    /// The index against `HEAD` (against nothing before the first commit): what is staged.
    Staged,
    /// An untracked file, all of it new.
    Untracked,
}

/// The diff of the uncommitted changes `which` to the file at `paths` (a staged rename's old
/// path first).
///
/// The work tree's side is as git would store it: through the file's clean filters and
/// line-ending conversion, as `git add` makes it.
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't a patch.
pub fn uncommitted_diff(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    which: Uncommitted,
    options: &DiffOptions,
    cancel: &CancelToken,
) -> Result<FileDiff, GitError> {
    let mut command = git
        .read("diff")
        .args([
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ])
        .arg(format!("--unified={}", options.context))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if let Some(algorithm) = &options.algorithm {
        command = command.arg(format!("--diff-algorithm={algorithm}"));
    }
    if options.ignore_whitespace {
        command = command.arg("--ignore-all-space");
    }
    let paths = paths.iter().map(|path| path.to_os_str_lossy());
    match which {
        Uncommitted::Unstaged => parse_streamed(command.arg("--").args(paths)),
        Uncommitted::Staged => parse_streamed(command.args(["--cached", "-M", "--"]).args(paths)),
        Uncommitted::Untracked => {
            // Against nothing: `--no-index` says whether they differ by exiting 1.
            let output = command
                .args(["--no-index", "--", "/dev/null"])
                .args(paths)
                .output_unchecked()?;
            if !matches!(output.status.code(), Some(0 | 1)) {
                return Err(GitError::failed("diff", output.status, output.stderr));
            }
            let mut parser = Parser::default();
            let mut failed = None;
            let _ = Lines::default().feed(&output.stdout, &mut |line| {
                parser
                    .line(line)
                    .map_err(|error| failed = Some(error))
                    .map_or(
                        std::ops::ControlFlow::Break(()),
                        std::ops::ControlFlow::Continue,
                    )
            });
            failed.map_or(Ok(parser.diff), Err)
        }
    }
}

/// The diff of the file at `paths` (a rename's or copy's old path first) between `parent` and
/// `commit` (or everything `commit` holds, when it is a root commit).
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't a patch.
pub fn file_diff(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    parent: Option<ObjectId>,
    paths: &[&BStr],
    options: &DiffOptions,
    cancel: &CancelToken,
) -> Result<FileDiff, GitError> {
    let mut command = git
        .read("diff-tree")
        .args([
            "-p",
            "-r",
            "-M",
            "--no-commit-id",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ])
        .arg(format!("--unified={}", options.context))
        // Paths are paths, never patterns.
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if let Some(algorithm) = &options.algorithm {
        command = command.arg(format!("--diff-algorithm={algorithm}"));
    }
    if options.ignore_whitespace {
        command = command.arg("--ignore-all-space");
    }
    command = match parent {
        Some(parent) => command.arg(parent.to_string()).arg(commit.to_string()),
        None => command.arg("--root").arg(commit.to_string()),
    };
    command = command
        .arg("--")
        .args(paths.iter().map(|path| path.to_os_str_lossy()));
    parse_streamed(command)
}

/// Runs `command`, parsing the patch it prints as it streams.
fn parse_streamed(command: GitCommand) -> Result<FileDiff, GitError> {
    let mut parser = Parser::default();
    let mut lines = Lines::default();
    let mut failed = None;
    command.stream(|chunk| {
        lines.feed(chunk, &mut |line| match parser.line(line) {
            Ok(()) => std::ops::ControlFlow::Continue(()),
            Err(error) => {
                failed = Some(error);
                std::ops::ControlFlow::Break(())
            }
        })
    })?;
    match failed {
        Some(error) => Err(error),
        None => Ok(parser.diff),
    }
}

/// `<old>..<new>`, and maybe a mode after a space: an `index` line's blobs.
fn index_blobs(names: &[u8]) -> Option<(ObjectId, ObjectId)> {
    let names = names.split_str(b" ").next()?;
    let (old, new) = names.split_once_str(b"..")?;
    Some((ObjectId::from_hex(old)?, ObjectId::from_hex(new)?))
}

/// Parses a patch a line at a time.
#[derive(Debug, Default)]
struct Parser {
    diff: FileDiff,
    /// The next line's numbers in the old and new file.
    old: u32,
    new: u32,
    /// Whether the lines are a hunk's (rather than a file's header).
    in_hunk: bool,
}

impl Parser {
    fn line(&mut self, line: &[u8]) -> Result<(), GitError> {
        if line.starts_with(b"@@ ") {
            let ranges = HunkRanges::parse(line).ok_or_else(|| {
                GitError::unexpected("diff", format!("{} isn't a hunk's header", BStr::new(line)))
            })?;
            self.old = ranges.old_start;
            self.new = ranges.new_start;
            self.in_hunk = true;
            self.push(DiffLineKind::Hunk, line, 0, 0);
            return Ok(());
        }
        if !self.in_hunk {
            if line.starts_with(b"Binary files ") && line.ends_with(b" differ") {
                self.diff.binary = true;
            }
            // The first file's (a rename's both paths name one).
            if self.diff.blobs.is_none()
                && let Some(names) = line.strip_prefix(b"index ")
            {
                self.diff.blobs = index_blobs(names);
            }
            return Ok(());
        }
        let (kind, text) = match line.split_first() {
            Some((b' ', text)) => (DiffLineKind::Context, text),
            Some((b'+', text)) => (DiffLineKind::Added, text),
            Some((b'-', text)) => (DiffLineKind::Removed, text),
            Some((b'\\', _)) => {
                if let Some(last) = self.diff.lines.last_mut() {
                    last.missing_newline = true;
                }
                return Ok(());
            }
            // An empty context line, from a git configured to show it so.
            None => (DiffLineKind::Context, line),
            Some(_) => {
                // The next file's header (when the paths named more than one file).
                self.in_hunk = false;
                return Ok(());
            }
        };
        let old = if kind == DiffLineKind::Added {
            0
        } else {
            self.old
        };
        let new = if kind == DiffLineKind::Removed {
            0
        } else {
            self.new
        };
        if kind != DiffLineKind::Added {
            self.old = self.old.saturating_add(1);
        }
        if kind != DiffLineKind::Removed {
            self.new = self.new.saturating_add(1);
        }
        self.push(kind, text, old, new);
        Ok(())
    }

    fn push(&mut self, kind: DiffLineKind, text: &[u8], old: u32, new: u32) {
        self.diff.text.extend_from_slice(text);
        self.diff.text.push(b'\n');
        self.diff.lines.push(DiffLine {
            kind,
            missing_newline: false,
            old,
            new,
        });
    }
}

/// The lines a hunk covers in the old and the new file, from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HunkRanges {
    /// The old file's first line (when `old_count` is 0, the line after which the new lines go).
    pub(crate) old_start: u32,
    pub(crate) old_count: u32,
    /// The new file's first line (likewise).
    pub(crate) new_start: u32,
    pub(crate) new_count: u32,
}

impl HunkRanges {
    /// Reads a hunk's header line: `@@ -12,7 +12,9 @@ …` (a count of one is left out:
    /// `@@ -12 +12 @@`).
    pub(crate) fn parse(line: &[u8]) -> Option<Self> {
        let mut fields = line.strip_prefix(b"@@ ")?.split_str(b" ");
        let range = |field: &[u8], sign: u8| -> Option<(u32, u32)> {
            let mut parts = field.strip_prefix(&[sign])?.split_str(b",");
            let number = |part: &[u8]| part.to_str().ok()?.parse().ok();
            let start = number(parts.next()?)?;
            let count = parts.next().map_or(Some(1), number)?;
            parts.next().is_none().then_some((start, count))
        };
        let (old_start, old_count) = range(fields.next()?, b'-')?;
        let (new_start, new_count) = range(fields.next()?, b'+')?;
        (fields.next()? == b"@@").then_some(Self {
            old_start,
            old_count,
            new_start,
            new_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(patch: &str) -> FileDiff {
        let mut parser = Parser::default();
        for line in patch.lines() {
            parser.line(line.as_bytes()).unwrap();
        }
        parser.diff
    }

    #[test]
    fn numbers_each_line_in_both_files() {
        let diff = parse(
            "diff --git a/f b/f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             --- a/f\n\
             +++ b/f\n\
             @@ -2,4 +2,4 @@ fn main()\n \
             keep\n\
             -old\n\
             +new\n\
             +more\n \
             end\n\
             -gone\n\
             \\ No newline at end of file\n",
        );
        assert!(!diff.binary);
        let blob = |digit: &str| ObjectId::from_hex(digit.repeat(40).as_bytes()).unwrap();
        assert_eq!(diff.blobs, Some((blob("1"), blob("2"))));
        let lines: Vec<(DiffLineKind, u32, u32)> = diff
            .lines
            .iter()
            .map(|line| (line.kind, line.old, line.new))
            .collect();
        assert_eq!(
            lines,
            [
                (DiffLineKind::Hunk, 0, 0),
                (DiffLineKind::Context, 2, 2),
                (DiffLineKind::Removed, 3, 0),
                (DiffLineKind::Added, 0, 3),
                (DiffLineKind::Added, 0, 4),
                (DiffLineKind::Context, 4, 5),
                (DiffLineKind::Removed, 5, 0),
            ]
        );
        assert!(diff.lines[6].missing_newline);
        assert!(!diff.lines[5].missing_newline);
        assert_eq!(
            diff.text,
            b"@@ -2,4 +2,4 @@ fn main()\nkeep\nold\nnew\nmore\nend\ngone\n"
        );
    }

    #[test]
    fn reads_the_blobs_of_the_index_line() {
        let zeros = "0".repeat(40);
        let blobs = |line: &str| parse(line).blobs;
        let (old, new) = blobs(&format!("index {zeros}..{}\n", "e".repeat(40))).unwrap();
        assert!(old == old.format().zero() && new != new.format().zero());
        assert_eq!(
            blobs("index 1234567..89abcde 100644\n"),
            None,
            "abbreviated"
        );
        assert_eq!(blobs("old mode 100644\nnew mode 100755\n"), None);
    }

    #[test]
    fn a_binary_file_has_no_lines() {
        let diff = parse(
            "diff --git a/image.png b/image.png\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             Binary files a/image.png and b/image.png differ\n",
        );
        assert!(diff.binary);
        assert_eq!(diff.lines, []);
    }

    #[test]
    fn reads_hunk_headers_with_and_without_counts() {
        let ranges = |line: &[u8]| {
            HunkRanges::parse(line).map(|ranges| {
                (
                    ranges.old_start,
                    ranges.old_count,
                    ranges.new_start,
                    ranges.new_count,
                )
            })
        };
        assert_eq!(ranges(b"@@ -12,7 +12,9 @@ fn x()"), Some((12, 7, 12, 9)));
        assert_eq!(ranges(b"@@ -1 +1 @@"), Some((1, 1, 1, 1)));
        assert_eq!(ranges(b"@@ -0,0 +1,3 @@"), Some((0, 0, 1, 3)));
        assert_eq!(ranges(b"@@ +1 -1 @@"), None);
        assert_eq!(ranges(b"@@ -1 +1"), None);
        assert_eq!(ranges(b"@@ -x +1 @@"), None);
        assert_eq!(ranges(b"@@ -1,2,3 +1 @@"), None);
    }

    #[test]
    fn a_malformed_hunk_header_is_an_error() {
        let mut parser = Parser::default();
        assert!(parser.line(b"@@ nonsense @@").is_err());
    }
}
