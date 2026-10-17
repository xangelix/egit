//! Commits as patches to mail or apply elsewhere (`git format-patch`): their messages and
//! changes, as `git am` takes them; and the uncommitted changes as a diff `git apply` takes.

use std::path::{Path, PathBuf};

use bstr::ByteSlice as _;

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::rebase::MAX_PLANNED;
use crate::repo::Repository;

/// The longest name [`patch_file_name`] gives, as git's own.
const MAX_NAME: usize = 64;

/// `commit` as a patch (`git format-patch -1 --stdout`, with git's standard `a/` and `b/`
/// prefixes whatever `diff.noprefix` says, so `git am` takes it as it is).
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn format_patch(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Vec<u8>, GitError> {
    git.read("format-patch")
        .args(["-1", "--stdout", "--default-prefix"])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(|done| done.stdout)
}

/// Writes the commits from `from` (included) up to `HEAD`, one patch each, into `folder`
/// (`git format-patch -o`, git naming them `0001-…`); merges are left out, as git leaves them.
/// Returns the files written.
///
/// # Errors
///
/// When `from` isn't in `HEAD`'s history, the range is past [`MAX_PLANNED`] commits, or git
/// fails or is cancelled.
pub fn format_patches(
    git: &Git,
    repository: &Repository,
    from: ObjectId,
    folder: &Path,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, GitError> {
    let in_branch = git
        .read("merge-base")
        .args(["--is-ancestor", &from.to_string(), "HEAD"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    if !in_branch.status.success() {
        return Err(GitError::unexpected(
            "format-patch",
            "the commit isn't in the current branch's history",
        ));
    }
    let parent = git
        .read("rev-parse")
        .args(["--verify", "--quiet", &format!("{from}^")])
        .current_dir(repository.work_tree())
        .output_unchecked()?;
    // From the first commit: everything `HEAD` reaches.
    let range: Vec<String> = if parent.status.success() {
        vec![format!("{from}^..HEAD")]
    } else {
        vec!["--root".to_owned(), "HEAD".to_owned()]
    };
    let counted = git
        .read("rev-list")
        .args(["--count", "--no-merges"])
        .args(range.iter().filter(|arg| *arg != "--root"))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    let count: usize = counted
        .stdout
        .trim_ascii()
        .to_str()
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    if count > MAX_PLANNED {
        return Err(GitError::unexpected(
            "format-patch",
            format!("more than {MAX_PLANNED} commits"),
        ));
    }
    let written = git
        .read("format-patch")
        .args(["--default-prefix", "-o"])
        .arg(folder)
        .args(&range)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    Ok(written
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(line.to_os_str_lossy().into_owned()))
        .collect())
}

/// Which uncommitted changes a patch holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changes {
    /// Those staged (`git diff --cached`).
    Staged,
    /// Every change to a tracked file, staged or not (`git diff HEAD`; untracked files aren't
    /// in it).
    All,
}

/// The uncommitted `changes` as a patch `git apply` takes: binary files too, standard `a/` and
/// `b/` prefixes, no external diff or text conversion, paths from the top.
///
/// # Errors
///
/// When git fails (no commit yet for [`Changes::All`]) or is cancelled.
pub fn changes_patch(
    git: &Git,
    repository: &Repository,
    changes: Changes,
    cancel: &CancelToken,
) -> Result<Vec<u8>, GitError> {
    git.read("diff")
        .args([
            "--binary",
            "--default-prefix",
            "--no-ext-diff",
            "--no-textconv",
            "--no-relative",
            "--no-color",
        ])
        .arg(match changes {
            Changes::Staged => "--cached",
            Changes::All => "HEAD",
        })
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(|done| done.stdout)
}

/// The file name git gives a commit's patch from its subject: `0001-` and the subject's
/// letters, digits, dots, and underscores, anything else becoming one `-`, cut to 64 bytes
/// with `.patch`.
#[must_use]
pub fn patch_file_name(subject: &str) -> String {
    const PREFIX: &str = "0001-";
    const SUFFIX: &str = ".patch";
    let room = MAX_NAME
        .saturating_sub(PREFIX.len())
        .saturating_sub(SUFFIX.len());
    let mut body = String::new();
    let mut dash = false;
    for c in subject.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            if dash && !body.is_empty() {
                body.push('-');
            }
            dash = false;
            body.push(c);
        } else {
            dash = true;
        }
        if body.len() >= room {
            break;
        }
    }
    body.truncate(room);
    let kept = body.trim_end_matches(['.', '-']).len();
    body.truncate(kept);
    if body.is_empty() {
        body.push_str("patch");
    }
    format!("{PREFIX}{body}{SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_is_named_as_git_names_it() {
        assert_eq!(
            patch_file_name("Fix a memory leak (v1.2)"),
            "0001-Fix-a-memory-leak-v1.2.patch"
        );
        assert_eq!(patch_file_name("  ...  "), "0001-patch.patch");
        assert_eq!(patch_file_name("Grüße, world!"), "0001-Gr-e-world.patch");
        // Cut (mid-word, as git does) to 64 bytes.
        let long = patch_file_name(&"word ".repeat(40));
        assert_eq!(long.len(), 64, "{long}");
        assert!(long.starts_with("0001-word-word-"));
        assert_eq!(&long[long.len() - 6..], ".patch");
    }
}
