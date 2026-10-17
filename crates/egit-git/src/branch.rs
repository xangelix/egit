//! Branches and tags: switching to a branch (or a commit), creating, renaming, and deleting
//! branches, and creating and deleting tags.
//!
//! Names given are short names (`topic`, `v1.2`), checked by [`check_ref_name`] as git checks
//! them; git refuses any it would not take anyway.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// Why a name can't be a branch's or a tag's, as `git check-ref-format --branch` tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadRefName {
    /// It is empty.
    Empty,
    /// It starts with `-` (git would read it as an option).
    LeadingDash,
    /// It holds a space, a control character, or one of `~ ^ : ? * [ \`.
    Forbidden(char),
    /// It holds `..`, `@{`, or `//`, or is `@`.
    Sequence(&'static str),
    /// It starts or ends with `/`, or ends with `.`.
    Edge,
    /// A part of it (between `/`) starts with `.` or ends with `.lock`.
    Part,
}

impl std::fmt::Display for BadRefName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("a name is needed"),
            Self::LeadingDash => f.write_str("it can't start with -"),
            Self::Forbidden(' ') => f.write_str("it can't hold spaces"),
            Self::Forbidden(c) if c.is_control() => f.write_str("it can't hold control characters"),
            Self::Forbidden(c) => write!(f, "it can't hold {c}"),
            Self::Sequence("@") => f.write_str("it can't be @"),
            Self::Sequence(sequence) => write!(f, "it can't hold {sequence}"),
            Self::Edge => f.write_str("it can't start or end with /, or end with ."),
            Self::Part => f.write_str("no part of it can start with . or end with .lock"),
        }
    }
}

/// Whether `name` can be a branch's or a tag's short name, by `git check-ref-format`'s rules.
///
/// # Errors
///
/// What is wrong with it.
pub fn check_ref_name(name: &str) -> Result<(), BadRefName> {
    if name.is_empty() {
        return Err(BadRefName::Empty);
    }
    if name.starts_with('-') {
        return Err(BadRefName::LeadingDash);
    }
    if let Some(c) = name
        .chars()
        .find(|&c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
    {
        return Err(BadRefName::Forbidden(c));
    }
    if name == "@" {
        return Err(BadRefName::Sequence("@"));
    }
    if let Some(sequence) = ["..", "@{", "//"]
        .into_iter()
        .find(|sequence| name.contains(sequence))
    {
        return Err(BadRefName::Sequence(sequence));
    }
    if name.starts_with('/') || name.ends_with('/') || name.ends_with('.') {
        return Err(BadRefName::Edge);
    }
    if name
        .split('/')
        // As git's: `.lock` in any other case is fine.
        .any(|part| part.starts_with('.') || part.as_bytes().ends_with(b".lock"))
    {
        return Err(BadRefName::Part);
    }
    Ok(())
}

/// Where to switch to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchTo {
    /// A local branch, by its short name.
    Branch(BString),
    /// A remote-tracking branch (`origin/topic`), through a new local branch (`topic`)
    /// tracking it.
    Track {
        /// The remote-tracking branch's short name.
        remote_branch: BString,
        /// The local branch made.
        local: BString,
    },
    /// A new branch, made at a commit.
    NewBranch {
        /// Its short name.
        name: BString,
        /// Where it starts.
        at: ObjectId,
    },
    /// A commit, without a branch.
    Detached(ObjectId),
}

/// Why switching failed.
#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    /// Switching would overwrite changes in the work tree: local changes to tracked files, or
    /// (`untracked`) files git doesn't track.
    #[error("switching would overwrite {} file(s) in the work tree", paths.len())]
    WouldOverwrite {
        /// The files, as git names them.
        paths: Vec<BString>,
        /// Whether they are files git doesn't track.
        untracked: bool,
    },
    /// Git failed otherwise, or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Switches the work tree to `to` (`git switch`).
///
/// # Errors
///
/// [`SwitchError::WouldOverwrite`] when changes in the work tree are in the way (nothing
/// changed), and [`SwitchError::Git`] when git fails otherwise or is cancelled.
pub fn switch(
    git: &Git,
    repository: &Repository,
    to: &SwitchTo,
    cancel: &CancelToken,
) -> Result<(), SwitchError> {
    let mut command = git
        .write("switch")
        .args(["--quiet", "--no-guess"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    command = match to {
        SwitchTo::Branch(name) => command.arg(OsStr::from_bytes(name)),
        SwitchTo::Track {
            remote_branch,
            local,
        } => command
            .arg("--create")
            .arg(OsStr::from_bytes(local))
            .arg("--track")
            .arg(OsStr::from_bytes(remote_branch)),
        SwitchTo::NewBranch { name, at } => command
            .arg("--create")
            .arg(OsStr::from_bytes(name))
            .arg(at.to_string()),
        SwitchTo::Detached(at) => command.arg("--detach").arg(at.to_string()),
    };
    match command.output() {
        Ok(_) => Ok(()),
        Err(error) => Err(would_overwrite(&error).unwrap_or(SwitchError::Git(error))),
    }
}

/// The files git said a switch would overwrite, if that is why it failed: the indented lines
/// after `… would be overwritten by checkout:`.
fn would_overwrite(error: &GitError) -> Option<SwitchError> {
    let said = error.failure()?.stderr.as_bytes();
    let at = said.find("would be overwritten by checkout")?;
    let before = said.get(..at)?;
    let untracked = before.rfind("untracked").is_some_and(|untracked| {
        before
            .get(untracked..)
            .is_some_and(|rest| !rest.contains(&b'\n'))
    });
    let paths = said
        .get(at..)?
        .lines()
        .skip(1)
        .take_while(|line| line.starts_with(b"\t"))
        .map(|line| BString::from(line.trim_start_with(|c| c == '\t')))
        .collect();
    Some(SwitchError::WouldOverwrite { paths, untracked })
}

/// Makes a branch `name` at `at` (`git branch`), without switching to it.
///
/// # Errors
///
/// When git fails (a branch of that name exists, say) or is cancelled.
pub fn create_branch(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    at: ObjectId,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("branch")
        .arg(OsStr::from_bytes(name))
        .arg(at.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Renames the branch `old` to `new` (`git branch --move`; its upstream and reflog go along).
///
/// # Errors
///
/// When git fails (a branch called `new` exists, say) or is cancelled.
pub fn rename_branch(
    git: &Git,
    repository: &Repository,
    old: &BStr,
    new: &BStr,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("branch")
        .arg("--move")
        .arg(OsStr::from_bytes(old))
        .arg(OsStr::from_bytes(new))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Sets `branch`'s upstream, where it pulls from and pushes to by default.
///
/// `upstream` is a remote-tracking branch's short name, such as `origin/main` (`git branch
/// --set-upstream-to`); `None` forgets it (`--unset-upstream`).
///
/// # Errors
///
/// When git fails (`upstream` isn't a branch it has, say) or is cancelled.
pub fn set_upstream(
    git: &Git,
    repository: &Repository,
    branch: &BStr,
    upstream: Option<&BStr>,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git.write("branch").arg("--quiet");
    command = match upstream {
        Some(upstream) => {
            let mut option = b"--set-upstream-to=".to_vec();
            option.extend_from_slice(upstream);
            command.arg(OsStr::from_bytes(&option))
        }
        None => command.arg("--unset-upstream"),
    };
    command
        .arg(OsStr::from_bytes(branch))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Why deleting a branch failed.
#[derive(Debug, thiserror::Error)]
pub enum DeleteBranchError {
    /// Its commits aren't all in its upstream or `HEAD`: deleting it could lose them.
    #[error("the branch isn't merged")]
    NotMerged,
    /// Git failed otherwise, or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Deletes the local branch `name` (`git branch --delete`); `force` deletes it even when its
/// commits aren't merged anywhere (they stay in the reflog for a while).
///
/// # Errors
///
/// [`DeleteBranchError::NotMerged`] without `force` when it isn't merged, and
/// [`DeleteBranchError::Git`] when git fails otherwise (it is checked out, say).
pub fn delete_branch(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    force: bool,
    cancel: &CancelToken,
) -> Result<(), DeleteBranchError> {
    let mut command = git.write("branch").arg("--delete");
    if force {
        command = command.arg("--force");
    }
    let done = command
        .arg(OsStr::from_bytes(name))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output();
    match done {
        Ok(_) => Ok(()),
        Err(error)
            if error.failure().is_some_and(|failure| {
                failure.stderr.as_bytes().contains_str("not fully merged")
            }) =>
        {
            Err(DeleteBranchError::NotMerged)
        }
        Err(error) => Err(error.into()),
    }
}

/// Makes a tag `name` at `at`: annotated with `message` (whitespace cleaned up, as a commit's
/// is), or lightweight without one (never signed: `tag.gpgSign` would ask for a message).
///
/// # Errors
///
/// When git fails (a tag of that name exists, say) or is cancelled.
pub fn create_tag(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    at: ObjectId,
    message: Option<&str>,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git
        .write("tag")
        .prompt_context("Signing the tag")
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    command = match message {
        Some(message) => command
            .args(["--annotate", "--cleanup=whitespace", "--file=-"])
            .stdin(message.as_bytes().to_vec()),
        None => command.arg("--no-sign"),
    };
    command
        .arg(OsStr::from_bytes(name))
        .arg(at.to_string())
        .output()
        .map(drop)
}

/// Deletes the tag `name` (`git tag --delete`), here only: a remote keeps its own.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn delete_tag(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("tag")
        .arg("--delete")
        .arg(OsStr::from_bytes(name))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_as_git_checks_them() {
        for good in ["topic", "feature/login", "v1.2.3", "fix-#12", "日本"] {
            assert_eq!(check_ref_name(good), Ok(()), "{good}");
        }
        for (bad, why) in [
            ("", BadRefName::Empty),
            ("-x", BadRefName::LeadingDash),
            ("a b", BadRefName::Forbidden(' ')),
            ("a~1", BadRefName::Forbidden('~')),
            ("a:b", BadRefName::Forbidden(':')),
            ("a\\b", BadRefName::Forbidden('\\')),
            ("a\tb", BadRefName::Forbidden('\t')),
            ("@", BadRefName::Sequence("@")),
            ("a..b", BadRefName::Sequence("..")),
            ("a@{1}", BadRefName::Sequence("@{")),
            ("a//b", BadRefName::Sequence("//")),
            ("/a", BadRefName::Edge),
            ("a/", BadRefName::Edge),
            ("a.", BadRefName::Edge),
            ("a/.b", BadRefName::Part),
            (".a", BadRefName::Part),
            ("a.lock", BadRefName::Part),
            ("a.lock/b", BadRefName::Part),
        ] {
            assert_eq!(check_ref_name(bad), Err(why), "{bad:?}");
        }
    }
}
