//! One file's changes as git shows them, parsed into lines ready to display.
//!
//! The patch comes from `git diff-tree -p` (plumbing: no colour, pager, external diff, textconv,
//! or prefix settings apply) and is parsed as it streams: each line is kept without its `+`,
//! `-`, or ` ` prefix, numbered in the old and new file, so the raw patch is never held whole.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
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
}

/// How a diff is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffOptions {
    /// Unchanged lines shown around each change ([`WHOLE_FILE`] for all of them).
    pub context: u32,
    /// The diff algorithm (`myers`, `minimal`, `patience`, `histogram`), when the user chose one
    /// (`diff.algorithm`, which plumbing doesn't read).
    pub algorithm: Option<String>,
}

impl Default for DiffOptions {
    /// Three lines of context, as git shows by default.
    fn default() -> Self {
        Self {
            context: 3,
            algorithm: None,
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
    command = match parent {
        Some(parent) => command.arg(parent.to_string()).arg(commit.to_string()),
        None => command.arg("--root").arg(commit.to_string()),
    };
    command = command
        .arg("--")
        .args(paths.iter().map(|path| OsStr::from_bytes(path)));
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
        if let Some(header) = line.strip_prefix(b"@@ ") {
            let (old, new) = hunk_starts(header).ok_or_else(|| {
                GitError::unexpected(
                    "diff-tree",
                    format!("{} isn't a hunk's header", BStr::new(line)),
                )
            })?;
            self.old = old;
            self.new = new;
            self.in_hunk = true;
            self.push(DiffLineKind::Hunk, line, 0, 0);
            return Ok(());
        }
        if !self.in_hunk {
            if line.starts_with(b"Binary files ") && line.ends_with(b" differ") {
                self.diff.binary = true;
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

/// The first old and new line numbers of a hunk, from its header after `@@ `:
/// `-12,7 +12,9 @@ …` (a count of one is left out: `-12 +12 @@`).
fn hunk_starts(header: &[u8]) -> Option<(u32, u32)> {
    let mut fields = header.split_str(b" ");
    let start = |field: &[u8], sign: u8| -> Option<u32> {
        let range = field.strip_prefix(&[sign])?;
        let start = range.split_str(b",").next()?;
        start.to_str().ok()?.parse().ok()
    };
    let old = start(fields.next()?, b'-')?;
    let new = start(fields.next()?, b'+')?;
    (fields.next()? == b"@@").then_some((old, new))
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
        assert_eq!(hunk_starts(b"-12,7 +12,9 @@ fn x()"), Some((12, 12)));
        assert_eq!(hunk_starts(b"-1 +1 @@"), Some((1, 1)));
        assert_eq!(hunk_starts(b"-0,0 +1,3 @@"), Some((0, 1)));
        assert_eq!(hunk_starts(b"+1 -1 @@"), None);
        assert_eq!(hunk_starts(b"-1 +1"), None);
        assert_eq!(hunk_starts(b"-x +1 @@"), None);
    }

    #[test]
    fn a_malformed_hunk_header_is_an_error() {
        let mut parser = Parser::default();
        assert!(parser.line(b"@@ nonsense @@").is_err());
    }
}
