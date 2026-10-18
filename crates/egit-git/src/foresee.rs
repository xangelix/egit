//! What bringing commits into a branch would do, worked out without touching the work tree,
//! the index, or any ref: a merge (`merge-tree --write-tree`, as `git merge` merges), a
//! cherry-pick or a revert (the same, the commit's parent or the commit itself as the merge
//! base), or a rebase (each commit replayed in turn onto unreferenced commits `commit-tree`
//! makes). The trees, blobs, and commits worked out are written, referred to by nothing (git
//! collects them).

use bstr::{BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::format_patch::commit_of;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// The most commits a rebase is replayed to foresee it.
pub const MAX_REPLAYED: usize = 100;

/// What bringing commits in would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreseen {
    /// Nothing: they are in already.
    UpToDate,
    /// The branch would only move forward to them.
    FastForward,
    /// It goes through without conflicts.
    Clean,
    /// It goes through, changing nothing (a pick or revert whose changes are in already).
    Empty,
    /// It would stop at conflicts in `files`; a rebase at its commit `at`.
    Conflicts {
        /// The files in conflict, as git names them.
        files: Vec<BString>,
        /// The branch's commit a rebase would stop at.
        at: Option<ObjectId>,
    },
    /// Not worked out: a rebase would replay this many commits, past [`MAX_REPLAYED`].
    TooLong(usize),
}

impl Foreseen {
    /// Whether it would stop at conflicts.
    #[must_use]
    pub const fn conflicts(&self) -> bool {
        matches!(self, Self::Conflicts { .. })
    }
}

/// What merging `other` into `head` would do.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn foresee_merge(
    git: &Git,
    repository: &Repository,
    head: ObjectId,
    other: ObjectId,
    cancel: &CancelToken,
) -> Result<Foreseen, GitError> {
    if reaches(git, repository, head, other, cancel)? {
        return Ok(Foreseen::UpToDate);
    }
    if reaches(git, repository, other, head, cancel)? {
        return Ok(Foreseen::FastForward);
    }
    let merged = merge_tree(git, repository, None, (head, other), cancel)?;
    Ok(merged.foreseen(None))
}

/// What cherry-picking `commit` onto `head` would do, or (`revert`) reverting it there; `None`
/// for a commit without a parent, which has no changes against one to work out.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn foresee_pick(
    git: &Git,
    repository: &Repository,
    head: ObjectId,
    (commit, revert): (ObjectId, bool),
    cancel: &CancelToken,
) -> Result<Option<Foreseen>, GitError> {
    // A merge's changes are those against its first parent, as egit picks and reverts them.
    let Some(parent) = first_parent(git, repository, commit, cancel)? else {
        return Ok(None);
    };
    let (base, theirs) = if revert {
        (commit, parent)
    } else {
        (parent, commit)
    };
    let merged = merge_tree(git, repository, Some(base), (head, theirs), cancel)?;
    if merged.conflicts.is_empty() && Some(merged.tree) == tree_of(git, repository, head, cancel)? {
        return Ok(Some(Foreseen::Empty));
    }
    Ok(Some(merged.foreseen(None)))
}

/// What rebasing `head` onto `onto` would do: its commits not in `onto`, merges left out (as
/// `git rebase` leaves them), replayed in turn; [`Foreseen::TooLong`] past [`MAX_REPLAYED`].
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn foresee_rebase(
    git: &Git,
    repository: &Repository,
    head: ObjectId,
    onto: ObjectId,
    cancel: &CancelToken,
) -> Result<Foreseen, GitError> {
    if reaches(git, repository, head, onto, cancel)? {
        return Ok(Foreseen::UpToDate);
    }
    if reaches(git, repository, onto, head, cancel)? {
        return Ok(Foreseen::FastForward);
    }
    let range = format!("{onto}..{head}");
    let counted = git
        .read("rev-list")
        .args(["--count", "--no-merges", &range])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    let count = counted
        .stdout
        .trim_ascii()
        .to_str()
        .ok()
        .and_then(|count| count.parse::<usize>().ok())
        .ok_or_else(|| GitError::unexpected("rev-list", "a count"))?;
    if count > MAX_REPLAYED {
        return Ok(Foreseen::TooLong(count));
    }
    let listed = git
        .read("rev-list")
        .args(["--reverse", "--no-merges", "--parents", &range])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    let mut current = onto;
    for line in listed.stdout.lines() {
        let mut names = line.fields_with(|c| c == ' ').map(ObjectId::from_hex);
        let (Some(Some(commit)), Some(Some(parent))) = (names.next(), names.next()) else {
            return Err(GitError::unexpected("rev-list", "a commit and its parent"));
        };
        let merged = merge_tree(git, repository, Some(parent), (current, commit), cancel)?;
        if !merged.conflicts.is_empty() {
            return Ok(merged.foreseen(Some(commit)));
        }
        current = commit_of(git, repository, merged.tree, cancel)?;
    }
    Ok(Foreseen::Clean)
}

/// A merge worked out: its tree, and the files in conflict.
struct Merged {
    tree: ObjectId,
    conflicts: Vec<BString>,
}

impl Merged {
    fn foreseen(self, at: Option<ObjectId>) -> Foreseen {
        if self.conflicts.is_empty() {
            Foreseen::Clean
        } else {
            Foreseen::Conflicts {
                files: self.conflicts,
                at,
            }
        }
    }
}

/// `theirs` merged into `ours` (`merge-tree --write-tree`, from `base` when given): the tree
/// and the files in conflict. Nothing is fetched (`GIT_NO_LAZY_FETCH`, where git honours it)
/// and nothing asked.
fn merge_tree(
    git: &Git,
    repository: &Repository,
    base: Option<ObjectId>,
    (ours, theirs): (ObjectId, ObjectId),
    cancel: &CancelToken,
) -> Result<Merged, GitError> {
    let mut command =
        git.read("merge-tree")
            .args(["--write-tree", "-z", "--name-only", "--no-messages"]);
    if let Some(base) = base {
        command = command.arg(format!("--merge-base={base}"));
    }
    let merged = command
        .arg(ours.to_string())
        .arg(theirs.to_string())
        .env("GIT_NO_LAZY_FETCH", "1")
        .non_interactive()
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    // 1: conflicts.
    if !matches!(merged.status.code(), Some(0 | 1)) {
        return Err(GitError::failed("merge-tree", merged.status, merged.stderr));
    }
    let mut records = merged.stdout.split_str(b"\0");
    let tree = records
        .next()
        .and_then(ObjectId::from_hex)
        .ok_or_else(|| GitError::unexpected("merge-tree", "a tree"))?;
    let mut conflicts: Vec<BString> = records
        .filter(|path| !path.is_empty())
        .map(BString::from)
        .collect();
    conflicts.dedup();
    Ok(Merged { tree, conflicts })
}

/// Whether `ancestor` is in `of`'s history (`merge-base --is-ancestor`).
fn reaches(
    git: &Git,
    repository: &Repository,
    of: ObjectId,
    ancestor: ObjectId,
    cancel: &CancelToken,
) -> Result<bool, GitError> {
    let reached = git
        .read("merge-base")
        .args(["--is-ancestor", &ancestor.to_string(), &of.to_string()])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    match reached.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(GitError::failed(
            "merge-base",
            reached.status,
            reached.stderr,
        )),
    }
}

/// `commit`'s first parent, if it has one.
fn first_parent(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Option<ObjectId>, GitError> {
    let listed = git
        .read("rev-list")
        .args(["--parents", "-n", "1", &commit.to_string()])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    Ok(listed
        .stdout
        .fields_with(|c| c.is_ascii_whitespace())
        .nth(1)
        .and_then(ObjectId::from_hex))
}

/// `commit`'s tree.
fn tree_of(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Option<ObjectId>, GitError> {
    let found = git
        .read("rev-parse")
        .args(["--verify", "--quiet", &format!("{commit}^{{tree}}")])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    Ok(found
        .status
        .success()
        .then(|| ObjectId::from_hex(found.stdout.trim_ascii()))
        .flatten())
}
