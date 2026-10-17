//! Another work tree of a repository (`git worktree add`): a branch, a new branch, or a commit
//! checked out in a folder of its own, sharing the repository's history.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use bstr::BStr;

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
            command = command
                .arg("-b")
                .arg(OsStr::from_bytes(name))
                .arg(if track { "--track" } else { "--no-track" });
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
        .arg(OsStr::from_bytes(&start))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}
