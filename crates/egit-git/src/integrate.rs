//! Commits brought into the current branch: merging a branch or commit, rebasing onto one,
//! cherry-picking or reverting a commit; and resetting the branch to a commit.
//!
//! Each may stop at conflicts, leaving the operation in progress for the merge editor and
//! Continue or Abort ([`Integrated::Conflicts`]); a pick or revert that changes nothing is
//! skipped ([`Integrated::Empty`]) rather than left waiting.

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::hooks::Checks;
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

/// Whether a merge may just move the branch forward, when it can.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FastForward {
    /// As the configuration says (`merge.ff`; unset, a fast-forward whenever one is possible).
    #[default]
    AsConfigured,
    /// Never: a merge commit even when the branch could just move forward (`--no-ff`), so the
    /// history keeps the branch merged as one.
    Never,
}

/// Merges `what` (a ref's full name, a branch's, or a commit's) into the current branch (`git
/// merge --no-edit`: git's message), fast-forwarding as `fast_forward` says.
///
/// Its `pre-merge-commit` and `commit-msg` hooks run as `checks` says.
///
/// git's message names what it was given as it was typed, so a ref's full name is shortened
/// first, as far as it stays unambiguous (`rev-parse --abbrev-ref=strict`): "Merge branch
/// 'topic'", or "'heads/topic'" beside a tag called `topic`, never "'refs/heads/topic'".
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub async fn merge(
    git: &Git,
    repository: &Repository,
    what: &BStr,
    fast_forward: FastForward,
    checks: Checks,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let short = if what.starts_with(b"refs/") {
        let named = git
            .read("rev-parse")
            // It starts with `refs/`: never an option.
            .args(["--verify", "--abbrev-ref=strict"])
            .arg(what.to_os_str_lossy())
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output()
            .await?;
        Some(named.stdout.trim_ascii().to_owned())
            .filter(|short| !short.is_empty() && !short.contains(&b'\n'))
    } else {
        None
    };
    let mut command = git
        .write("merge")
        .args(["--no-edit", "--no-stat", "--quiet"])
        .args(checks.flag());
    if fast_forward == FastForward::Never {
        command = command.arg("--no-ff");
    }
    let done = command
        .arg(short.as_deref().unwrap_or(what).to_os_str_lossy())
        .prompt_context("Merging")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    stopped(git, repository, done.await.map(drop), cancel).await
}

/// Rebases the current branch onto `onto` (`git rebase`): its commits not in `onto`,
/// replayed on it.
///
/// With `keep_merges`, its merges too (`--rebase-merges`), which a rebase otherwise flattens
/// (unless the configuration's `rebase.rebaseMerges` says otherwise). Its `pre-rebase` hook
/// runs as `checks` says (a commit it replays runs no hook that checks).
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub async fn rebase(
    git: &Git,
    repository: &Repository,
    onto: &BStr,
    keep_merges: bool,
    checks: Checks,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let mut command = git.write("rebase").arg("--quiet").args(checks.flag());
    if keep_merges {
        command = command.arg("--rebase-merges");
    }
    let done = command
        .arg("--end-of-options")
        .arg(onto.to_os_str_lossy())
        .prompt_context("Rebasing")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    stopped(git, repository, done.await.map(drop), cancel).await
}

/// Applies `commit`'s changes to the current branch as a new commit (`git cherry-pick`; a
/// merge's changes against its first parent).
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub async fn cherry_pick(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    pick(git, repository, "cherry-pick", &[], &[commit], cancel).await
}

/// Applies `commits`' changes to the current branch as new commits, in the order given (the
/// oldest first, as they were made).
///
/// One `git cherry-pick` goes through them all (merges' changes against their first parents);
/// those that change nothing here are skipped. Stopped at a conflict, the rest wait for
/// Continue.
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub async fn cherry_pick_all(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    pick(git, repository, "cherry-pick", &[], commits, cancel).await
}

/// Undoes `commit`'s changes in the current branch with a new commit (`git revert`, git's
/// message; a merge's against its first parent).
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub async fn revert(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    pick(git, repository, "revert", &["--no-edit"], &[commit], cancel).await
}

/// Undoes `commits`' changes in the current branch with new commits, in the order given (the
/// newest first, so each undoes what is there), as [`revert`] does each.
///
/// # Errors
///
/// When git refuses (local changes in the way, say) or is cancelled.
pub async fn revert_all(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    pick(git, repository, "revert", &["--no-edit"], commits, cancel).await
}

/// Cherry-picks or reverts `commits` (`subcommand`) in turn, skipping those that change
/// nothing ([`Integrated::Empty`] when every one did).
async fn pick(
    git: &Git,
    repository: &Repository,
    subcommand: &'static str,
    args: &[&str],
    commits: &[ObjectId],
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
    // A merge's changes are those against its first parent (git takes `--mainline` for the
    // other commits too, since 2.21).
    for commit in commits {
        if parents(git, repository, *commit, cancel).await? > 1 {
            command = command.args(["--mainline", "1"]);
            break;
        }
    }
    for commit in commits {
        command = command.arg(commit.to_string());
    }
    let mut done = command.output().await.map(drop);
    // A commit whose changes are here already stops git: skipped, the rest go on.
    let mut skipped = 0;
    while skipped < commits.len() && is_now_empty(&done) {
        skipped = skipped.saturating_add(1);
        done = git
            .write(subcommand)
            .arg("--skip")
            .current_dir(work_tree)
            .cancel_token(cancel)
            .output()
            .await
            .map(drop);
    }
    if skipped == commits.len() && done.is_ok() {
        return Ok(Integrated::Empty);
    }
    stopped(git, repository, done, cancel).await
}

/// Whether git stopped at a commit that would change nothing ("… is now empty").
fn is_now_empty(done: &Result<(), GitError>) -> bool {
    done.as_ref().is_err_and(|error| {
        error
            .failure()
            .is_some_and(|failure| failure.stderr.as_bytes().contains_str("is now empty"))
    })
}

/// How many parents `commit` has.
async fn parents(
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
        .output()
        .await?;
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
pub(crate) async fn stopped(
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
    let operations = repository
        .operations(git)
        .await
        .map_err(|source| GitError::Io {
            subcommand: "status",
            source,
        })?;
    let in_progress = operations.iter().any(|operation| {
        matches!(
            operation,
            Operation::Merge | Operation::Rebase | Operation::CherryPick | Operation::Revert
        )
    });
    if in_progress && has_conflicts(git, repository, cancel).await? {
        Ok(Integrated::Conflicts)
    } else {
        Err(error)
    }
}

/// How an operation carried on after a skip ended: done, stopped at the next conflicts, or (a
/// `git am`) stopped at a patch that doesn't apply.
pub(crate) async fn carried_on(
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
    let operations = repository
        .operations(git)
        .await
        .map_err(|source| GitError::Io {
            subcommand: "status",
            source,
        })?;
    if operations.is_empty() {
        Err(error)
    } else if has_conflicts(git, repository, cancel).await? {
        Ok(Integrated::Conflicts)
    } else if operations.contains(&Operation::ApplyMailbox) {
        Ok(Integrated::Stopped)
    } else {
        Err(error)
    }
}

/// Whether the index holds a conflict.
pub(crate) async fn has_conflicts(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<bool, GitError> {
    let listed = git
        .read("ls-files")
        .args(["-u", "-z"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
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
    /// The uncommitted changes, unless a file they touch differs between the two commits (then
    /// nothing moves): `--keep`, for taking an action back without losing work.
    Keep,
}

/// Moves the current branch (or `HEAD`) to `commit` (`git reset`), keeping what `mode` says.
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn reset(
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
        ResetMode::Keep => "--keep",
    };
    git.write("reset")
        .args(["--quiet", mode])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}
