//! Stashes: the work tree's changes put aside (`git stash push`), listed, and applied, popped,
//! or dropped.
//!
//! A stash is named by its place in the list (`stash@{2}`), which shifts as stashes come and
//! go; each one acted on is checked against the commit it was listed with first
//! ([`StashError::Stale`] otherwise), so another stash is never applied or dropped by mistake.

use bstr::{BStr, ByteSlice as _};

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
pub async fn stashes(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Vec<Stash>, GitError> {
    let listed = git
        .read("stash")
        .args(["list", "-z", "--format=%gd%x1f%H%x1f%ct%x1f%gs"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
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

/// A stash as the history shows it: a commit of its own on the one it was made on.
///
/// Git keeps a stash as a merge of the commit `HEAD` was at, a commit of what was staged, and
/// (with untracked files) one of those; the history shows the stash alone, on the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StashNode {
    /// Its commit (what `stash@{index}` names).
    pub commit: ObjectId,
    /// The commit it was made on: its first parent.
    pub base: ObjectId,
    /// When it was made (seconds since the Unix epoch).
    pub time: i64,
    /// Its place in the list, the latest first.
    pub index: u32,
}

/// The stashes as the history shows them (`git stash list`, with each one's first parent),
/// the latest first; none without any.
///
/// # Errors
///
/// When git fails, is cancelled, or lists what isn't a stash.
pub async fn stash_nodes(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Vec<StashNode>, GitError> {
    let listed = git
        .read("stash")
        .args(["list", "-z", "--format=%H%x1f%P%x1f%ct"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    let unexpected = || GitError::unexpected("stash", "a stash and its parents");
    listed
        .stdout
        .split_str(b"\0")
        .map(|record| record.trim_with(|c| c == '\n'))
        .filter(|record| !record.is_empty())
        .enumerate()
        .map(|(index, record)| {
            let mut fields = record.splitn_str(3, b"\x1f");
            let commit = fields
                .next()
                .and_then(ObjectId::from_hex)
                .ok_or_else(unexpected)?;
            let base = fields
                .next()
                .and_then(|parents| parents.split_str(b" ").next())
                .and_then(ObjectId::from_hex)
                .ok_or_else(unexpected)?;
            let time = fields
                .next()
                .and_then(|time| time.to_str().ok()?.trim().parse().ok())
                .ok_or_else(unexpected)?;
            Ok(StashNode {
                commit,
                base,
                time,
                index: u32::try_from(index).unwrap_or(u32::MAX),
            })
        })
        .collect()
}

/// Puts the work tree's changes aside (`git stash push`), with `message` if it isn't empty,
/// and files git doesn't track too with `untracked`. Returns whether there was anything to
/// stash.
///
/// Only the changes at `paths` go, when any are given (literal paths on standard input;
/// untracked ones among them always go too: git refuses an untracked path otherwise).
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn stash_push(
    git: &Git,
    repository: &Repository,
    (message, untracked): (&str, bool),
    paths: &[&BStr],
    cancel: &CancelToken,
) -> Result<bool, GitError> {
    let before = latest(git, repository, cancel).await?;
    let mut command = git
        .write("stash")
        .args(["push", "--quiet"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if untracked || !paths.is_empty() {
        command = command.arg("--include-untracked");
    }
    let message = message.trim();
    if !message.is_empty() {
        command = command.arg("--message").arg(message);
    }
    if !paths.is_empty() {
        let mut stdin =
            Vec::with_capacity(paths.iter().map(|path| path.len().saturating_add(1)).sum());
        for path in paths {
            stdin.extend_from_slice(path);
            stdin.push(b'\0');
        }
        command = command
            .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
            // Paths are paths, never patterns.
            .env("GIT_LITERAL_PATHSPECS", "1")
            .stdin(stdin);
    }
    command.output().await?;
    // Nothing to stash makes no stash (and says so only without `--quiet`).
    Ok(latest(git, repository, cancel).await? != before)
}

/// The latest stash's commit, if there is one.
pub(crate) async fn latest(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Option<ObjectId>, GitError> {
    let found = git
        .read("rev-parse")
        .args(["--verify", "--quiet", "refs/stash"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()
        .await?;
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
pub async fn stash_apply(
    git: &Git,
    repository: &Repository,
    stash: &Stash,
    pop: bool,
    cancel: &CancelToken,
) -> Result<Integrated, StashError> {
    still(git, repository, stash, cancel).await?;
    let done = git
        .write("stash")
        .args([if pop { "pop" } else { "apply" }, "--quiet"])
        .arg(stash.name())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    match done.await {
        Ok(_) => Ok(Integrated::Done),
        Err(error)
            if error.failure().is_some() && has_conflicts(git, repository, cancel).await? =>
        {
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
pub async fn stash_drop(
    git: &Git,
    repository: &Repository,
    stash: &Stash,
    cancel: &CancelToken,
) -> Result<(), StashError> {
    still(git, repository, stash, cancel).await?;
    git.write("stash")
        .args(["drop", "--quiet"])
        .arg(stash.name())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    Ok(())
}

/// Puts the stash `commit` back on the stash list, as the latest, with `message` (`git stash
/// store`): a dropped stash brought back (its commit is still there until git prunes it).
///
/// # Errors
///
/// When `commit` isn't a stash git has, or git fails or is cancelled.
pub async fn stash_store(
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
        .await
        .map(drop)
}

/// Whether `stash` is still where it was listed.
async fn still(
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
        .output_unchecked()
        .await?;
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
