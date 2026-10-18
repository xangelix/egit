//! Discarding changes not staged, as a task: whole files (tracked ones back to what the index
//! holds, untracked ones deleted) or some lines of one file's diff. Each file is kept as it was
//! and as the discard left it (`egit_git::discard_files`, `discard_lines`), so Undo can put it
//! back.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, DiffLineKind, DiscardError, EntryKind, FileDiff, Git, GitError, Repository,
    StatusEntry, TreeEntry, discard_files, discard_lines,
};

use crate::problem::Problem;
use crate::tasks::Outcome;

/// What a discard throws away: changes not staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Discard {
    /// Whole files: `tracked` go back to what the index holds, `untracked` are deleted.
    Files {
        tracked: Vec<BString>,
        untracked: Vec<BString>,
    },
    /// Some lines of one file.
    Lines(Box<DiscardLines>),
}

/// Some lines of a file's diff not staged, picked to discard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiscardLines {
    pub(crate) path: BString,
    /// The index against the work tree (an untracked file's: nothing against it).
    pub(crate) diff: Arc<FileDiff>,
    /// Which of its lines were picked.
    pub(crate) picked: Vec<bool>,
    /// What the index held at the path when the diff was read (`None`: untracked).
    pub(crate) entry: Option<TreeEntry>,
    /// Whether whole hunks were picked, rather than lines (for the words).
    pub(crate) hunks: bool,
}

impl Discard {
    /// What it throws away, in words: "the changes to src/main.rs", "a hunk of src/main.rs".
    pub(crate) fn words(&self) -> String {
        match self {
            Self::Files { tracked, untracked } => {
                match (tracked.as_slice(), untracked.as_slice()) {
                    ([path], []) => format!("the changes to {}", path.to_str_lossy()),
                    ([], [path]) => path.to_str_lossy().into_owned(),
                    ([], paths) => format!("{} untracked files", paths.len()),
                    _ => format!(
                        "the changes to {} files",
                        tracked.len().saturating_add(untracked.len())
                    ),
                }
            }
            Self::Lines(lines) => {
                let path = lines.path.to_str_lossy();
                let (count, one, many) = if lines.hunks {
                    (lines.hunk_count(), "a hunk", "hunks")
                } else {
                    let count = lines.picked.iter().filter(|picked| **picked).count();
                    (count, "a line", "lines")
                };
                if count == 1 {
                    format!("{one} of {path}")
                } else {
                    format!("{count} {many} of {path}")
                }
            }
        }
    }

    /// How many files it names.
    pub(crate) const fn files(&self) -> usize {
        match self {
            Self::Files { tracked, untracked } => tracked.len().saturating_add(untracked.len()),
            Self::Lines(_) => 1,
        }
    }
}

impl DiscardLines {
    /// How many hunks hold a picked line.
    fn hunk_count(&self) -> usize {
        let mut count = 0_usize;
        let mut counted = false;
        for (line, picked) in self.diff.lines.iter().zip(&self.picked) {
            if line.kind == DiffLineKind::Hunk {
                counted = false;
            } else if *picked && !counted {
                counted = true;
                count = count.saturating_add(1);
            }
        }
        count
    }
}

/// Why the changes not staged of `entry` can't be discarded here (`None`: they can).
pub(crate) fn refusal(entry: StatusEntry<'_>) -> Option<&'static str> {
    if entry.kind == EntryKind::Unmerged {
        Some("It is in conflict: resolve it, or abort the operation")
    } else if entry.submodule().is_some() {
        Some("A submodule: update it to the commit recorded from its page")
    } else if entry.kind == EntryKind::Untracked && entry.path.ends_with(b"/") {
        Some("A repository of its own: delete its folder yourself")
    } else {
        None
    }
}

/// Discarding the changes not staged of every one of `entries` that can be: the tracked go
/// back to what the index holds, the untracked are deleted (`None`: none can).
pub(crate) fn of_entries<'a>(
    entries: impl IntoIterator<Item = StatusEntry<'a>>,
) -> Option<Discard> {
    let (mut tracked, mut untracked) = (Vec::new(), Vec::new());
    for entry in entries {
        if !entry.is_unstaged() || refusal(entry).is_some() {
            continue;
        }
        if entry.kind == EntryKind::Untracked {
            untracked.push(entry.path.to_owned());
        } else {
            tracked.push(entry.path.to_owned());
        }
    }
    (!tracked.is_empty() || !untracked.is_empty()).then_some(Discard::Files { tracked, untracked })
}

/// Discards as `discard` says; the outcome keeps each file as it was and as it is now, for Undo
/// (none when they were too many or too large to keep).
pub(crate) fn run(
    git: &Git,
    repository: &Repository,
    discard: &Discard,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let done = match discard {
        Discard::Files { tracked, untracked } => {
            discard_files(git, repository, &paths(tracked), &paths(untracked), cancel)
        }
        Discard::Lines(lines) => discard_lines(
            git,
            repository,
            (lines.path.as_bstr(), &lines.diff),
            |index| lines.picked.get(index).copied().unwrap_or(false),
            lines.entry,
            cancel,
        )
        .map(|file| Some(vec![file])),
    };
    match done {
        Ok(files) => Ok(Outcome::Discarded(files)),
        Err(DiscardError::Git(GitError::Cancelled { .. })) => Ok(Outcome::Cancelled),
        Err(error) => Err(problem(&error)),
    }
}

/// `paths`, borrowed.
fn paths(paths: &[BString]) -> Vec<&BStr> {
    paths.iter().map(AsRef::as_ref).collect()
}

/// Why the changes weren't discarded, for people.
fn problem(error: &DiscardError) -> Problem {
    const TITLE: &str = "The changes weren't discarded";
    match error {
        DiscardError::Stale(path) => Problem::new(
            TITLE,
            format!(
                "{} changed since its diff was read. Look at it again, then pick the lines \
                 anew.",
                path.to_str_lossy()
            ),
        ),
        DiscardError::NotLines(path) => Problem::new(
            TITLE,
            format!(
                "{} is a symbolic link or a submodule: only a file's lines can be discarded. \
                 Discard the file instead.",
                path.to_str_lossy()
            ),
        ),
        DiscardError::Git(error) => Problem::git(TITLE, error),
        DiscardError::Io { path, source } => {
            Problem::new(TITLE, format!("{}: {source}.", path.to_str_lossy()))
        }
    }
}

#[cfg(test)]
mod tests {
    use egit_git::DiffLine;

    use super::*;

    fn lines(kinds: &[DiffLineKind], picked: &[bool], hunks: bool) -> Discard {
        let line = |kind| DiffLine {
            kind,
            missing_newline: false,
            old: 0,
            new: 0,
        };
        Discard::Lines(Box::new(DiscardLines {
            path: "src/main.rs".into(),
            diff: Arc::new(FileDiff {
                lines: kinds.iter().copied().map(line).collect(),
                ..FileDiff::default()
            }),
            picked: picked.to_vec(),
            entry: None,
            hunks,
        }))
    }

    #[test]
    fn says_what_it_throws_away() {
        use DiffLineKind::{Added, Context, Hunk, Removed};
        let files = |tracked: &[&str], untracked: &[&str]| {
            let owned = |paths: &[&str]| paths.iter().map(|path| BString::from(*path)).collect();
            Discard::Files {
                tracked: owned(tracked),
                untracked: owned(untracked),
            }
            .words()
        };
        assert_eq!(files(&["a.rs"], &[]), "the changes to a.rs");
        assert_eq!(files(&[], &["new.rs"]), "new.rs");
        assert_eq!(files(&[], &["x", "y"]), "2 untracked files");
        assert_eq!(files(&["a"], &["x", "y"]), "the changes to 3 files");
        let kinds = [Hunk, Context, Removed, Added, Hunk, Added, Added];
        let picked = [false, false, true, true, false, true, false];
        assert_eq!(
            lines(&kinds, &picked, false).words(),
            "3 lines of src/main.rs"
        );
        assert_eq!(
            lines(&kinds, &picked, true).words(),
            "2 hunks of src/main.rs"
        );
        let first = [false, false, true, true, false, false, false];
        assert_eq!(lines(&kinds, &first, true).words(), "a hunk of src/main.rs");
    }
}
