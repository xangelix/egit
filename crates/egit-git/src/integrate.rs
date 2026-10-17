//! Commits brought into the current branch: merging a branch or commit, rebasing onto one,
//! cherry-picking or reverting a commit; and resetting the branch to a commit.
//!
//! Each may stop at conflicts, leaving the operation in progress for the merge editor and
//! Continue or Abort ([`Integrated::Conflicts`]); a pick or revert that changes nothing is
//! skipped ([`Integrated::Empty`]) rather than left waiting.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::{Operation, Repository};

/// How bringing commits in ended, when git didn't fail outright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrated {
    /// Done (or there was nothing to do).
    Done,
    /// Stopped at conflicts: they are in the work tree, the operation in progress.
    Conflicts,
    /// The commit changed nothing here (its changes were already in): skipped.
    Empty,
    /// Stopped with nothing to resolve: a patch that doesn't apply, the operation in progress
    /// (to fix by hand, skip, or abort).
    Stopped,
}

/// Merges `what` (a branch's name, or a commit's) into the current branch (`git merge
/// --no-edit`: git's message; a fast-forward when the configuration allows one).
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub fn merge(
    git: &Git,
    repository: &Repository,
    what: &BStr,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let done = git
        .write("merge")
        .args(["--no-edit", "--no-stat", "--quiet"])
        .arg(OsStr::from_bytes(what))
        .prompt_context("Merging")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    stopped(git, repository, done.map(drop), cancel)
}

/// Rebases the current branch onto `onto` (`git rebase`): its commits not in `onto`,
/// replayed on it.
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub fn rebase(
    git: &Git,
    repository: &Repository,
    onto: &BStr,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let done = git
        .write("rebase")
        .arg("--quiet")
        .arg(OsStr::from_bytes(onto))
        .prompt_context("Rebasing")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    stopped(git, repository, done.map(drop), cancel)
}

/// Applies `commit`'s changes to the current branch as a new commit (`git cherry-pick`; a
/// merge's changes against its first parent).
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub fn cherry_pick(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    pick(git, repository, "cherry-pick", &[], commit, cancel)
}

/// Undoes `commit`'s changes in the current branch with a new commit (`git revert`, git's
/// message; a merge's against its first parent).
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub fn revert(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    pick(git, repository, "revert", &["--no-edit"], commit, cancel)
}

/// Cherry-picks or reverts `commit` (`subcommand`), skipping it if it changes nothing.
fn pick(
    git: &Git,
    repository: &Repository,
    subcommand: &'static str,
    args: &[&str],
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let work_tree = repository.work_tree();
    let mut command = git
        .write(subcommand)
        .args(args)
        .prompt_context(if subcommand == "revert" {
            "Reverting"
        } else {
            "Cherry-picking"
        })
        .current_dir(work_tree)
        .cancel_token(cancel);
    if parents(git, repository, commit, cancel)? > 1 {
        // A merge's changes are those against its first parent.
        command = command.args(["--mainline", "1"]);
    }
    let done = command.arg(commit.to_string()).output();
    if let Err(error) = &done
        && error
            .failure()
            .is_some_and(|failure| failure.stderr.as_bytes().contains_str("is now empty"))
    {
        git.write(subcommand)
            .arg("--skip")
            .current_dir(work_tree)
            .cancel_token(cancel)
            .output()?;
        return Ok(Integrated::Empty);
    }
    stopped(git, repository, done.map(drop), cancel)
}

/// How many parents `commit` has.
fn parents(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<usize, GitError> {
    let listed = git
        .read("rev-list")
        .args(["--parents", "--max-count=1"])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    // The commit, then its parents.
    Ok(listed
        .stdout
        .trim_ascii()
        .split_str(b" ")
        .count()
        .saturating_sub(1))
}

/// What `done` means: a failure that left an operation stopped at conflicts is
/// [`Integrated::Conflicts`].
pub(crate) fn stopped(
    git: &Git,
    repository: &Repository,
    done: Result<(), GitError>,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let Err(error) = done else {
        return Ok(Integrated::Done);
    };
    if error.failure().is_none() {
        return Err(error);
    }
    let operations = repository.operations().map_err(|source| GitError::Io {
        subcommand: "status",
        source,
    })?;
    let in_progress = operations.iter().any(|operation| {
        matches!(
            operation,
            Operation::Merge | Operation::Rebase | Operation::CherryPick | Operation::Revert
        )
    });
    if in_progress && has_conflicts(git, repository, cancel)? {
        Ok(Integrated::Conflicts)
    } else {
        Err(error)
    }
}

/// How an operation carried on after a skip ended: done, stopped at the next conflicts, or (a
/// `git am`) stopped at a patch that doesn't apply.
pub(crate) fn carried_on(
    git: &Git,
    repository: &Repository,
    done: Result<(), GitError>,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let Err(error) = done else {
        return Ok(Integrated::Done);
    };
    if error.failure().is_none() {
        return Err(error);
    }
    let operations = repository.operations().map_err(|source| GitError::Io {
        subcommand: "status",
        source,
    })?;
    if operations.is_empty() {
        Err(error)
    } else if has_conflicts(git, repository, cancel)? {
        Ok(Integrated::Conflicts)
    } else if operations.contains(&Operation::ApplyMailbox) {
        Ok(Integrated::Stopped)
    } else {
        Err(error)
    }
}

/// Whether the index holds a conflict.
pub(crate) fn has_conflicts(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<bool, GitError> {
    let listed = git
        .read("ls-files")
        .args(["-u", "-z"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    Ok(!listed.stdout.is_empty())
}

/// What a reset keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    /// The index and the work tree: the changes since the commit stay staged.
    Soft,
    /// The work tree: the changes since the commit stay, unstaged.
    Mixed,
    /// Nothing: the index and the work tree become the commit's (uncommitted changes are lost).
    Hard,
}

/// Moves the current branch (or `HEAD`) to `commit` (`git reset`), keeping what `mode` says.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn reset(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    mode: ResetMode,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mode = match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    git.write("reset")
        .args(["--quiet", mode])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}
