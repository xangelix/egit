//! A repository's work trees: another one added (`git worktree add`: a branch, a new branch,
//! or a commit checked out in a folder of its own, sharing the repository's history), each one
//! listed, removed, locked or unlocked, and those whose folders are gone pruned.

use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// What a new work tree checks out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeFrom<'a> {
    /// A local branch (its short name), checked out nowhere else.
    Branch(&'a BStr),
    /// A new branch `name` made at `at` (a full ref name or a commit's); `track`: `at` is a
    /// remote's branch the new one follows as its upstream.
    NewBranch {
        /// The new branch's name.
        name: &'a BStr,
        /// Where it starts.
        at: &'a BStr,
        /// Whether `at` becomes its upstream.
        track: bool,
    },
    /// A commit, without a branch.
    Detached(ObjectId),
}

/// Adds a work tree at `path` (a folder that doesn't exist yet, or is empty) checking out
/// `from`.
///
/// # Errors
///
/// When git refuses (the folder isn't empty, the branch is checked out elsewhere or exists
/// already) or is cancelled.
pub fn add_worktree(
    git: &Git,
    repository: &Repository,
    path: &Path,
    from: WorktreeFrom<'_>,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git.write("worktree").args(["add", "--quiet"]);
    let start = match from {
        WorktreeFrom::Branch(branch) => branch.to_vec(),
        WorktreeFrom::NewBranch { name, at, track } => {
            command = command.arg("-b").arg(name.to_os_str_lossy()).arg(if track {
                "--track"
            } else {
                "--no-track"
            });
            at.to_vec()
        }
        WorktreeFrom::Detached(commit) => {
            command = command.arg("--detach");
            commit.to_string().into_bytes()
        }
    };
    command
        .arg("--")
        .arg(path)
        .arg(start.to_os_str_lossy())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// One of a repository's work trees (`git worktree list`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// Its folder.
    pub path: PathBuf,
    /// The commit checked out; `None` in a bare repository, or on a branch without commits.
    pub head: Option<ObjectId>,
    /// The branch checked out (its full name); `None` when detached or bare.
    pub branch: Option<BString>,
    /// Whether it is the repository's main one (listed first).
    pub main: bool,
    /// Whether it is a bare repository's (no files checked out).
    pub bare: bool,
    /// Why it is locked (empty when no reason was given); `None` when it isn't.
    pub locked: Option<BString>,
    /// Why git would prune it (its folder is gone, say); `None` when it wouldn't.
    pub prunable: Option<BString>,
}

/// The repository's work trees, the main one first (`worktree list --porcelain -z`).
///
/// # Errors
///
/// When git fails or prints what isn't a work tree.
pub fn worktrees(git: &Git, repository: &Repository) -> Result<Vec<Worktree>, GitError> {
    let listed = git
        .read("worktree")
        .args(["list", "--porcelain", "-z"])
        .current_dir(repository.work_tree())
        .output()?;
    parse_worktrees(&listed.stdout)
}

/// `worktree list --porcelain -z`'s records: fields ended by NUL, a record by an empty field.
fn parse_worktrees(listed: &[u8]) -> Result<Vec<Worktree>, GitError> {
    let malformed = || GitError::unexpected("worktree list", "a work tree");
    let mut found = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in listed.split_str(b"\0") {
        if field.is_empty() {
            found.extend(current.take());
            continue;
        }
        let (key, value) = field
            .split_once_str(b" ")
            .map_or((field, None), |(key, value)| (key, Some(value)));
        if key == b"worktree" {
            found.extend(current.take());
            current = Some(Worktree {
                path: value.ok_or_else(malformed)?.to_path_lossy().into_owned(),
                head: None,
                branch: None,
                main: found.is_empty(),
                bare: false,
                locked: None,
                prunable: None,
            });
            continue;
        }
        let worktree = current.as_mut().ok_or_else(malformed)?;
        match key {
            b"HEAD" => {
                let id = ObjectId::from_hex(value.ok_or_else(malformed)?).ok_or_else(malformed)?;
                // A branch without commits yet lists no object.
                worktree.head = (id != id.format().zero()).then_some(id);
            }
            b"branch" => worktree.branch = Some(BString::from(value.ok_or_else(malformed)?)),
            b"bare" => worktree.bare = true,
            b"locked" => worktree.locked = Some(BString::from(value.unwrap_or_default())),
            b"prunable" => worktree.prunable = Some(BString::from(value.unwrap_or_default())),
            // `detached`, and whatever later versions add.
            _ => {}
        }
    }
    found.extend(current);
    Ok(found)
}

/// Why a work tree wasn't removed.
#[derive(Debug, thiserror::Error)]
pub enum RemoveWorktreeError {
    /// It has changes not committed, or untracked files, which removing it would lose.
    #[error("it has changes not committed, or files git doesn't track")]
    Changes,
    /// It is locked.
    #[error("it is locked")]
    Locked,
    /// Git failed otherwise.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Removes the work tree at `path` and its folder (`worktree remove`); `force`: even with
/// changes not committed (never a locked one, which must be unlocked first).
///
/// # Errors
///
/// When it has changes (unless forced) or is locked, or git fails or is cancelled.
pub fn remove_worktree(
    git: &Git,
    repository: &Repository,
    path: &Path,
    force: bool,
    cancel: &CancelToken,
) -> Result<(), RemoveWorktreeError> {
    let mut command = git.write("worktree").arg("remove");
    if force {
        command = command.arg("--force");
    }
    let done = command
        .arg("--")
        .arg(path)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    if done.status.success() {
        return Ok(());
    }
    let said = done.stderr.message();
    Err(if said.contains("contains modified or untracked files") {
        RemoveWorktreeError::Changes
    } else if said.contains("cannot remove a locked working tree") {
        RemoveWorktreeError::Locked
    } else {
        RemoveWorktreeError::Git(GitError::failed("worktree", done.status, done.stderr))
    })
}

/// Locks the work tree at `path` (`worktree lock`), so it is neither pruned, moved, nor
/// removed: on a drive not always there, say; `reason`, when given, is shown with it.
///
/// # Errors
///
/// When it is locked already, or git fails or is cancelled.
pub fn lock_worktree(
    git: &Git,
    repository: &Repository,
    path: &Path,
    reason: &str,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git.write("worktree").arg("lock");
    if !reason.is_empty() {
        command = command.args(["--reason", reason]);
    }
    command
        .arg("--")
        .arg(path)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Unlocks the work tree at `path` (`worktree unlock`).
///
/// # Errors
///
/// When it isn't locked, or git fails or is cancelled.
pub fn unlock_worktree(
    git: &Git,
    repository: &Repository,
    path: &Path,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("worktree")
        .args(["unlock", "--"])
        .arg(path)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Forgets the work trees whose folders are gone (`worktree prune`), but locked ones.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn prune_worktrees(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("worktree")
        .arg("prune")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktrees_are_read_from_their_records() {
        let zero = "0".repeat(40);
        let one = "1".repeat(40);
        let listed = format!(
            "worktree /repo\0HEAD {one}\0branch refs/heads/main\0\0\
             worktree /side by side\0HEAD {one}\0detached\0locked\0\0\
             worktree /gone\0HEAD {zero}\0branch refs/heads/new\0locked on a drive\0\
             prunable gitdir file points to non-existent location\0\0"
        );
        let found = parse_worktrees(listed.as_bytes()).unwrap();
        assert_eq!(found.len(), 3);
        let [main, side, gone] = found.as_slice() else {
            unreachable!()
        };
        assert!(main.main && !side.main && !gone.main);
        assert_eq!(main.path, Path::new("/repo"));
        assert_eq!(main.branch, Some(BString::from("refs/heads/main")));
        assert_eq!(main.head, ObjectId::from_hex(one.as_bytes()));
        assert_eq!(side.path, Path::new("/side by side"));
        assert_eq!(side.branch, None);
        assert_eq!(side.locked, Some(BString::from("")));
        assert_eq!(gone.head, None, "no commit yet");
        assert_eq!(gone.locked, Some(BString::from("on a drive")));
        assert_eq!(
            gone.prunable,
            Some(BString::from("gitdir file points to non-existent location"))
        );
        assert!(
            parse_worktrees(b"HEAD 1234\0\0").is_err(),
            "no worktree line first"
        );
        assert_eq!(parse_worktrees(b"").unwrap(), Vec::new());
    }
}
