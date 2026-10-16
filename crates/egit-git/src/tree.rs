//! The folders a work tree's files are in, for watching them: a commit's folders, and which of
//! some paths git ignores.

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// Every folder in `commit`'s tree, at any depth, from the top of the work tree
/// (`ls-tree -r -d`).
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn tree_folders(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Vec<BString>, GitError> {
    let listed = git
        .read("ls-tree")
        .args(["-r", "-d", "-z", "--name-only"])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    Ok(listed
        .stdout
        .split_str(b"\0")
        .filter(|path| !path.is_empty())
        .map(BString::from)
        .collect())
}

/// Which of `paths` (from the top of the work tree) git ignores: those `.gitignore` and the
/// like match, unless they are tracked (`check-ignore`).
///
/// A folder's path should end with `/`, for patterns that only match folders (`build/`); it
/// is returned so.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn ignored_paths(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    cancel: &CancelToken,
) -> Result<Vec<BString>, GitError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path);
        stdin.push(b'\0');
    }
    let checked = git
        .read("check-ignore")
        .args(["-z", "--stdin"])
        .current_dir(repository.work_tree())
        .stdin(stdin)
        .cancel_token(cancel)
        .output_unchecked()?;
    // 1: none of them is ignored.
    match checked.status.code() {
        Some(0) => Ok(checked
            .stdout
            .split_str(b"\0")
            .filter(|path| !path.is_empty())
            .map(BString::from)
            .collect()),
        Some(1) => Ok(Vec::new()),
        _ => Err(GitError::failed(
            "check-ignore",
            checked.status,
            checked.stderr,
        )),
    }
}
