//! Moving a branch forward to commits that include it, never merging: the current branch by
//! `merge --ff-only`, any other by `fetch` from a remote or from the repository itself, which
//! moves a branch only by a fast-forward (and never the one checked out: git refuses).

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::progress::Progress;
use crate::repo::Repository;

/// Where a branch that isn't checked out moves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardFrom<'a> {
    /// A remote's branch (its short name there), fetched: pulling without merging.
    Remote {
        /// The remote's name.
        remote: &'a BStr,
        /// The branch's name on the remote.
        branch: &'a BStr,
    },
    /// A ref here, by its full name (`refs/remotes/origin/main`, `refs/heads/topic`).
    Here(&'a BStr),
}

/// How a fast-forward went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forwarded {
    /// The branch moved forward.
    Moved,
    /// It was there already.
    AlreadyThere,
    /// Nothing moved: the branch has commits the other hasn't (it would take a merge).
    NotFastForward,
}

/// Fast-forwards the current branch to `to` (a full ref name or a commit's), as `git merge
/// --ff-only` does.
///
/// # Errors
///
/// When git fails otherwise (changes in the way, an operation in progress) or is cancelled.
pub fn fast_forward_current(
    git: &Git,
    repository: &Repository,
    to: &BStr,
    cancel: &CancelToken,
) -> Result<Forwarded, GitError> {
    let done = git
        .write("merge")
        .args(["--ff-only", "--no-stat"])
        .arg(to.to_os_str_lossy())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    if done.status.success() {
        let there = done.stdout.find(b"Already up to date").is_some();
        return Ok(if there {
            Forwarded::AlreadyThere
        } else {
            Forwarded::Moved
        });
    }
    if done
        .stderr
        .message()
        .contains("Not possible to fast-forward")
    {
        return Ok(Forwarded::NotFastForward);
    }
    Err(GitError::failed("merge", done.status, done.stderr))
}

/// Fast-forwards `branch` (a local branch's short name, not the one checked out) to `from`:
/// `fetch --porcelain` with a refspec without `+`, so git moves it only forward.
///
/// # Errors
///
/// When git fails (the remote hasn't the branch, the branch is checked out somewhere) or is
/// cancelled.
pub fn fast_forward_branch(
    git: &Git,
    repository: &Repository,
    branch: &BStr,
    from: ForwardFrom<'_>,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Forwarded, GitError> {
    let mut target = b"refs/heads/".to_vec();
    target.extend_from_slice(branch);
    let (remote, mut refspec) = match from {
        ForwardFrom::Remote { remote, branch } => {
            let mut source = b"refs/heads/".to_vec();
            source.extend_from_slice(branch);
            (remote, source)
        }
        ForwardFrom::Here(source) => (BStr::new("."), source.to_vec()),
    };
    refspec.push(b':');
    refspec.extend_from_slice(&target);
    let done = git
        .write("fetch")
        .args(["--porcelain", "--progress", "--no-tags", "--no-prune", "--"])
        .arg(remote.to_os_str_lossy())
        .arg(refspec.to_os_str_lossy())
        .prompt_context(format!("Pulling {branch}"))
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel)
        .output_unchecked()?;
    // `<flag> <old> <new> <ref>` for each ref it touched; nothing for one already there.
    let flag = done.stdout.lines().find_map(|line| {
        let (flag, rest) = (line.first()?, line.get(2..)?);
        let name = rest.rsplit_str(b" ").next()?;
        (name == target.as_slice()).then_some(*flag)
    });
    match flag {
        Some(b'!') => Ok(Forwarded::NotFastForward),
        Some(_) if done.status.success() => Ok(Forwarded::Moved),
        None if done.status.success() => Ok(Forwarded::AlreadyThere),
        _ => Err(GitError::failed("fetch", done.status, done.stderr)),
    }
}
