//! Interactive rebase: the current branch's commits from a chosen one up to `HEAD`, each
//! picked, reworded, squashed or fixed up into the one before it, or dropped, in the order a
//! plan gives.
//!
//! [`rebase_range`] reads the commits (oldest first, with their messages), refusing a range
//! holding a merge: an interactive rebase would flatten it. [`rebase_interactive`] runs a plan
//! through `git rebase --interactive`, whose todo egit writes itself (git's sequence editor
//! copies it in). A reworded commit is picked, then amended by an `exec` line carrying its new
//! message, after any squashed or fixed-up into it: the todo holds everything, so a rebase
//! stopped at a conflict and continued later still rewords.

use std::fmt::Write as _;
use std::io::Write as _;

use bstr::ByteSlice as _;

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::integrate::{Integrated, stopped};
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// The most commits a plan holds.
pub const MAX_PLANNED: usize = 10_000;

/// A commit of a range to rebase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeCommit {
    /// Its name.
    pub id: ObjectId,
    /// Its author's name.
    pub author: String,
    /// Its message, whole (`%B`).
    pub message: String,
}

impl RangeCommit {
    /// Its subject: its message's first line.
    #[must_use]
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or_default()
    }
}

/// The commits an interactive rebase would rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseRange {
    /// What they are replayed onto: the first one's parent (`None` from the root).
    pub base: Option<ObjectId>,
    /// The commits, oldest first.
    pub commits: Vec<RangeCommit>,
}

/// Why a range can't be rebased interactively.
#[derive(Debug, thiserror::Error)]
pub enum RangeError {
    /// The commit isn't in the current branch (`HEAD` doesn't reach it).
    #[error("the commit isn't in the current branch")]
    NotInBranch,
    /// The range holds merges, which an interactive rebase would flatten.
    #[error("the range holds {0} merge(s)")]
    Merges(usize),
    /// It holds more commits than a plan does.
    #[error("the range holds more than {MAX_PLANNED} commits")]
    TooMany,
    /// Git failed or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// The commits from `from` up to `HEAD`, oldest first, with what they are replayed onto.
///
/// # Errors
///
/// [`RangeError::NotInBranch`], [`RangeError::Merges`], or [`RangeError::TooMany`] when the
/// range can't be rebased so, and [`RangeError::Git`] when git fails.
pub fn rebase_range(
    git: &Git,
    repository: &Repository,
    from: ObjectId,
    cancel: &CancelToken,
) -> Result<RebaseRange, RangeError> {
    let work_tree = repository.work_tree();
    let reached = git
        .read("merge-base")
        .args(["--is-ancestor", &from.to_string(), "HEAD"])
        .current_dir(work_tree)
        .cancel_token(cancel)
        .output_unchecked()?;
    match reached.status.code() {
        Some(0) => {}
        Some(1) => return Err(RangeError::NotInBranch),
        _ => {
            return Err(GitError::failed("merge-base", reached.status, reached.stderr).into());
        }
    }
    // From `from`'s parent (`from` itself is replayed), or from the root.
    let parents = git
        .read("rev-list")
        .args(["--parents", "--max-count=1", &from.to_string()])
        .current_dir(work_tree)
        .cancel_token(cancel)
        .output()?;
    let parents = parents.stdout.trim_ascii().split_str(b" ").skip(1).count();
    if parents > 1 {
        return Err(RangeError::Merges(1));
    }
    let range = if parents == 1 {
        format!("{from}^..HEAD")
    } else {
        "HEAD".to_owned()
    };
    let listed = git
        .read("log")
        .config("log.showSignature", "false")
        .args([
            "-z",
            "--reverse",
            "--topo-order",
            "--format=%H%x1f%P%x1f%an%x1f%B",
        ])
        .arg(format!("--max-count={}", MAX_PLANNED.saturating_add(1)))
        .arg(range)
        .arg("--")
        .current_dir(work_tree)
        .cancel_token(cancel)
        .output()?;
    parse_range(&listed.stdout, from)
}

/// The range from `log -z --reverse --format=%H%x1f%P%x1f%an%x1f%B`: the commits from `from`
/// on (those before it are dropped: a root's listing holds every commit).
fn parse_range(listed: &[u8], from: ObjectId) -> Result<RebaseRange, RangeError> {
    let unexpected = || GitError::unexpected("log", "a commit");
    let mut base = None;
    let mut commits = Vec::new();
    let mut merges = 0_usize;
    let mut started = false;
    for record in listed.split_str(b"\0").filter(|record| !record.is_empty()) {
        let record = record.strip_prefix(b"\n").unwrap_or(record);
        let mut fields = record.splitn_str(4, b"\x1f");
        let id = fields
            .next()
            .and_then(ObjectId::from_hex)
            .ok_or_else(unexpected)?;
        let parents: Vec<ObjectId> = fields
            .next()
            .ok_or_else(unexpected)?
            .split_str(b" ")
            .filter(|parent| !parent.is_empty())
            .map(|parent| ObjectId::from_hex(parent).ok_or_else(unexpected))
            .collect::<Result<_, _>>()?;
        if !started {
            if id != from {
                continue;
            }
            started = true;
            base = parents.first().copied();
        }
        if parents.len() > 1 {
            merges = merges.saturating_add(1);
        }
        let author = fields
            .next()
            .ok_or_else(unexpected)?
            .to_str_lossy()
            .into_owned();
        let message = fields
            .next()
            .unwrap_or_default()
            .trim_end_with(|c| c == '\n')
            .to_str_lossy()
            .into_owned();
        commits.push(RangeCommit {
            id,
            author,
            message,
        });
    }
    if merges > 0 {
        return Err(RangeError::Merges(merges));
    }
    if commits.len() > MAX_PLANNED {
        return Err(RangeError::TooMany);
    }
    if !started {
        return Err(RangeError::NotInBranch);
    }
    Ok(RebaseRange { base, commits })
}

/// What becomes of a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Kept as it is.
    Pick,
    /// Kept, with this message (for a commit others are squashed into: the group's).
    Reword(String),
    /// Melded into the commit before it (in the plan's order), their messages joined.
    Squash,
    /// Melded into the commit before it, its message dropped.
    Fixup,
    /// Left out.
    Drop,
}

/// A commit, and what becomes of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The commit.
    pub id: ObjectId,
    /// What becomes of it.
    pub step: Step,
}

/// Why a plan can't run.
#[derive(Debug, thiserror::Error)]
pub enum RebaseError {
    /// A squash or fixup has no commit kept before it to meld into.
    #[error("the first commit kept can't be squashed into one before it")]
    NothingToSquashInto,
    /// A reword's message is empty.
    #[error("a reworded commit's message is empty")]
    EmptyMessage,
    /// The plan has no commits, or more than [`MAX_PLANNED`].
    #[error("the plan holds no commits, or too many")]
    Size,
    /// The todo couldn't be written for git.
    #[error("the plan couldn't be written for git: {0}")]
    Todo(#[source] std::io::Error),
    /// Git failed (local changes in the way, say) or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Rebases the current branch onto `base` (`None`: from the root), its commits as `plan`
/// says, in its order (oldest first).
///
/// # Errors
///
/// A [`RebaseError`] for a plan git would refuse (nothing changes then), and
/// [`RebaseError::Git`] when git fails (local changes in the way, say) or is cancelled.
pub fn rebase_interactive(
    git: &Git,
    repository: &Repository,
    base: Option<ObjectId>,
    plan: &[Planned],
    cancel: &CancelToken,
) -> Result<Integrated, RebaseError> {
    let todo = todo(plan)?;
    let mut file = tempfile::Builder::new()
        .prefix("egit-rebase-")
        .tempfile()
        .map_err(RebaseError::Todo)?;
    file.write_all(todo.as_bytes())
        .and_then(|()| file.flush())
        .map_err(RebaseError::Todo)?;
    // Git runs the sequence editor with the todo's path after it: `cp -- ours theirs`.
    let path = file.path().to_string_lossy().into_owned();
    let editor = format!("cp -- {}", quote(&path));
    let mut command = git
        .write("rebase")
        .args([
            "--interactive",
            "--quiet",
            "--no-autosquash",
            "--no-update-refs",
        ])
        .env("GIT_SEQUENCE_EDITOR", editor)
        .prompt_context("Rebasing")
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    command = match base {
        Some(base) => command.arg(base.to_string()),
        None => command.arg("--root"),
    };
    let done = command.output().map(drop);
    Ok(stopped(git, repository, done, cancel)?)
}

/// The todo git runs for `plan`.
fn todo(plan: &[Planned]) -> Result<String, RebaseError> {
    if plan.is_empty() || plan.len() > MAX_PLANNED {
        return Err(RebaseError::Size);
    }
    let mut todo = String::new();
    // The reword of the group being written: amended in once the group's melded commits are.
    let mut group: Option<&str> = None;
    let mut kept = false;
    for planned in plan {
        let id = planned.id;
        match &planned.step {
            Step::Drop => {
                let _ = writeln!(todo, "drop {id}");
                continue;
            }
            Step::Squash | Step::Fixup if !kept => return Err(RebaseError::NothingToSquashInto),
            Step::Squash => {
                let _ = writeln!(todo, "squash {id}");
                continue;
            }
            Step::Fixup => {
                let _ = writeln!(todo, "fixup {id}");
                continue;
            }
            Step::Pick | Step::Reword(_) => {}
        }
        if let Some(message) = group.take() {
            reword(&mut todo, message);
        }
        if let Step::Reword(message) = &planned.step {
            if message.trim().is_empty() {
                return Err(RebaseError::EmptyMessage);
            }
            group = Some(message);
        }
        kept = true;
        let _ = writeln!(todo, "pick {id}");
    }
    if let Some(message) = group {
        reword(&mut todo, message);
    }
    Ok(todo)
}

/// An `exec` line giving the commit just made `message` (`printf` prints each line; the commit
/// keeps its author and date).
fn reword(todo: &mut String, message: &str) {
    todo.push_str("exec printf '%s\\n'");
    for line in message.trim_end().lines() {
        todo.push(' ');
        todo.push_str(&quote(line));
    }
    todo.push_str(" | git commit --amend --quiet --allow-empty --cleanup=whitespace --file=-\n");
}

/// `text` quoted for the shell: in single quotes, each `'` closed, escaped, and reopened.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn id(byte: char) -> ObjectId {
        ObjectId::from_hex(byte.to_string().repeat(40).as_bytes()).unwrap()
    }

    fn planned(byte: char, step: Step) -> Planned {
        Planned { id: id(byte), step }
    }

    #[test]
    fn rewords_come_after_what_is_melded_into_them() {
        let plan = [
            planned('a', Step::Reword("One 'quoted'\n\n%s 100% #1\n".to_owned())),
            planned('b', Step::Drop),
            planned('c', Step::Squash),
            planned('d', Step::Pick),
            planned('e', Step::Fixup),
            planned('f', Step::Reword("Last".to_owned())),
        ];
        let todo = todo(&plan).unwrap_or_default();
        let amend = "| git commit --amend --quiet --allow-empty --cleanup=whitespace --file=-";
        let expected = [
            format!("pick {}", id('a')),
            format!("drop {}", id('b')),
            format!("squash {}", id('c')),
            format!("exec printf '%s\\n' 'One '\\''quoted'\\''' '' '%s 100% #1' {amend}"),
            format!("pick {}", id('d')),
            format!("fixup {}", id('e')),
            format!("pick {}", id('f')),
            format!("exec printf '%s\\n' 'Last' {amend}"),
        ];
        assert_eq!(todo.lines().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn a_plan_git_would_refuse_is_refused() {
        let squash_first = [planned('a', Step::Drop), planned('b', Step::Squash)];
        assert!(matches!(
            todo(&squash_first),
            Err(RebaseError::NothingToSquashInto)
        ));
        let empty = [planned('a', Step::Reword(" \n".to_owned()))];
        assert!(matches!(todo(&empty), Err(RebaseError::EmptyMessage)));
        assert!(matches!(todo(&[]), Err(RebaseError::Size)));
    }

    #[test]
    fn a_range_starts_at_its_commit_and_refuses_merges() {
        let record = |id: char, parents: &str, message: &str| {
            format!(
                "{}\x1f{parents}\x1fAda\x1f{message}\n\0",
                id.to_string().repeat(40)
            )
        };
        let parent = 'a'.to_string().repeat(40);
        let listed = [
            record('a', "", "Root"),
            record('b', &parent, "Two\n\nBody"),
            record('c', &'b'.to_string().repeat(40), "Three"),
        ]
        .concat();
        let range = parse_range(listed.as_bytes(), id('b')).unwrap();
        assert_eq!(range.base, Some(id('a')));
        assert_eq!(range.commits.len(), 2);
        assert_eq!(range.commits[0].message, "Two\n\nBody");
        assert_eq!(range.commits[0].subject(), "Two");
        let root = parse_range(listed.as_bytes(), id('a')).unwrap();
        assert_eq!((root.base, root.commits.len()), (None, 3));
        let merged = [
            record('b', &parent, "Two"),
            record(
                'c',
                &format!("{parent} {}", 'b'.to_string().repeat(40)),
                "Merge",
            ),
        ]
        .concat();
        assert!(matches!(
            parse_range(merged.as_bytes(), id('b')),
            Err(RangeError::Merges(1))
        ));
    }
}
