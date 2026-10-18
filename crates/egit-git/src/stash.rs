//! Stashes: the work tree's changes put aside (`git stash push`), listed, and applied, popped,
//! or dropped.
//!
//! A stash is named by its place in the list (`stash@{2}`), which shifts as stashes come and
//! go; each one acted on is checked against the commit it was listed with first
//! ([`StashError::Stale`] otherwise), so another stash is never applied or dropped by mistake.

use bstr::ByteSlice as _;

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::integrate::{Integrated, has_conflicts};
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// A stash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    /// Its place in the list, the latest first (`stash@{index}`).
    pub index: u32,
    /// Its commit.
    pub id: ObjectId,
    /// When it was made (seconds since the Unix epoch).
    pub time: i64,
    /// What git says of it (`On main: message`, or `WIP on main: <the commit's subject>`).
    pub message: String,
}

impl Stash {
    /// Its name as git knows it now (`stash@{2}`).
    fn name(&self) -> String {
        format!("stash@{{{}}}", self.index)
    }
}

/// The stashes, the latest first.
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't a stash.
pub fn stashes(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Vec<Stash>, GitError> {
    let listed = git
        .read("stash")
        .args(["list", "-z", "--format=%gd%x1f%H%x1f%ct%x1f%gs"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    let unexpected = || GitError::unexpected("stash", "a stash");
    listed
        .stdout
        .split_str(b"\0")
        .map(|record| record.trim_with(|c| c == '\n'))
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut fields = record.splitn_str(4, b"\x1f");
            let index = fields
                .next()
                .and_then(|name| name.strip_prefix(b"stash@{")?.strip_suffix(b"}"))
                .and_then(|index| index.to_str().ok()?.parse().ok())
                .ok_or_else(unexpected)?;
            let id = fields
                .next()
                .and_then(ObjectId::from_hex)
                .ok_or_else(unexpected)?;
            let time = fields
                .next()
                .and_then(|time| time.to_str().ok()?.parse().ok())
                .ok_or_else(unexpected)?;
            let message = fields
                .next()
                .unwrap_or_default()
                .to_str_lossy()
                .into_owned();
            Ok(Stash {
                index,
                id,
                time,
                message,
            })
        })
        .collect()
}

/// Puts the work tree's changes aside (`git stash push`), with `message` if it isn't empty,
/// and files git doesn't track too with `untracked`. Returns whether there was anything to
/// stash.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn stash_push(
    git: &Git,
    repository: &Repository,
    message: &str,
    untracked: bool,
    cancel: &CancelToken,
) -> Result<bool, GitError> {
    let before = latest(git, repository, cancel)?;
    let mut command = git
        .write("stash")
        .args(["push", "--quiet"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if untracked {
        command = command.arg("--include-untracked");
    }
    let message = message.trim();
    if !message.is_empty() {
        command = command.arg("--message").arg(message);
    }
    command.output()?;
    // Nothing to stash makes no stash (and says so only without `--quiet`).
    Ok(latest(git, repository, cancel)? != before)
}

/// The latest stash's commit, if there is one.
pub(crate) fn latest(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Option<ObjectId>, GitError> {
    let found = git
        .read("rev-parse")
        .args(["--verify", "--quiet", "refs/stash"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    Ok(found
        .status
        .success()
        .then(|| ObjectId::from_hex(found.stdout.trim_ascii()))
        .flatten())
}

/// Why acting on a stash failed.
#[derive(Debug, thiserror::Error)]
pub enum StashError {
    /// The stash isn't where it was listed any more (another was made or dropped since).
    #[error("the stash list changed since it was read")]
    Stale,
    /// Git failed (local changes in the way, say) or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Applies `stash`'s changes to the work tree (`git stash apply`); with `pop`, drops it once
/// applied (a stash whose changes conflict is kept).
///
/// # Errors
///
/// [`StashError::Stale`] when the list changed, and [`StashError::Git`] when git fails.
pub fn stash_apply(
    git: &Git,
    repository: &Repository,
    stash: &Stash,
    pop: bool,
    cancel: &CancelToken,
) -> Result<Integrated, StashError> {
    still(git, repository, stash, cancel)?;
    let done = git
        .write("stash")
        .args([if pop { "pop" } else { "apply" }, "--quiet"])
        .arg(stash.name())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    match done {
        Ok(_) => Ok(Integrated::Done),
        Err(error) if error.failure().is_some() && has_conflicts(git, repository, cancel)? => {
            Ok(Integrated::Conflicts)
        }
        Err(error) => Err(error.into()),
    }
}

/// Drops `stash` (`git stash drop`): its changes are gone.
///
/// # Errors
///
/// [`StashError::Stale`] when the list changed, and [`StashError::Git`] when git fails.
pub fn stash_drop(
    git: &Git,
    repository: &Repository,
    stash: &Stash,
    cancel: &CancelToken,
) -> Result<(), StashError> {
    still(git, repository, stash, cancel)?;
    git.write("stash")
        .args(["drop", "--quiet"])
        .arg(stash.name())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    Ok(())
}

/// Puts the stash `commit` back on the stash list, as the latest, with `message` (`git stash
/// store`): a dropped stash brought back (its commit is still there until git prunes it).
///
/// # Errors
///
/// When `commit` isn't a stash git has, or git fails or is cancelled.
pub fn stash_store(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    message: &str,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("stash")
        .args(["store", "--quiet", "--message", message])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Whether `stash` is still where it was listed.
fn still(
    git: &Git,
    repository: &Repository,
    stash: &Stash,
    cancel: &CancelToken,
) -> Result<(), StashError> {
    let found = git
        .read("rev-parse")
        .args(["--verify", "--quiet"])
        .arg(stash.name())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    let id = found
        .status
        .success()
        .then(|| ObjectId::from_hex(found.stdout.trim_ascii()))
        .flatten();
    if id == Some(stash.id) {
        Ok(())
    } else {
        Err(StashError::Stale)
    }
}
