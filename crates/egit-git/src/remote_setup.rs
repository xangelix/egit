//! A repository's remotes set up: added, renamed (their remote-tracking branches and the
//! branches following them with them), pointed at another address, and removed (their
//! remote-tracking branches with them), by `git remote`.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use bstr::BStr;

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// Adds the remote `name` at `url` (`remote add`), fetching nothing yet.
///
/// # Errors
///
/// When it exists already, its name isn't one git takes, or git fails or is cancelled.
pub fn add_remote(
    git: &Git,
    repository: &Repository,
    name: &str,
    url: &str,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("remote")
        .args(["add", "--", name, url])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Renames the remote `old` to `new` (`remote rename`): its remote-tracking branches, its
/// settings, and the branches following it go along.
///
/// # Errors
///
/// When `new` exists already or isn't a name git takes, or git fails or is cancelled.
pub fn rename_remote(
    git: &Git,
    repository: &Repository,
    old: &BStr,
    new: &str,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("remote")
        .args(["rename", "--"])
        .arg(OsStr::from_bytes(old))
        .arg(new)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Removes the remote `name` (`remote remove`), its remote-tracking branches, and its
/// settings; branches following it follow nothing after.
///
/// # Errors
///
/// When there's no such remote, or git fails or is cancelled.
pub fn remove_remote(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("remote")
        .args(["remove", "--"])
        .arg(OsStr::from_bytes(name))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Points the remote `name` at `url` (`remote set-url`): where it fetches from, or (`push`)
/// where it pushes to, which is otherwise where it fetches from.
///
/// # Errors
///
/// When there's no such remote, or git fails or is cancelled.
pub fn set_remote_url(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    url: &str,
    push: bool,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git.write("remote").arg("set-url");
    if push {
        command = command.arg("--push");
    }
    command
        .arg("--")
        .arg(OsStr::from_bytes(name))
        .arg(url)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}
