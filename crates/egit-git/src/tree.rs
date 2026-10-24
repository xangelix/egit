//! A commit's tree: what it holds at a path, its folders, and its files with their sizes; and
//! which of some paths git ignores (for watching a work tree's folders).

use std::path::Path;

use bstr::{BStr, BString, ByteSlice as _, ByteVec as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::history::TreeEntry;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// What `commit`'s tree holds at `path` (from its top): `None` when nothing (`ls-tree` of that
/// one path, taken literally).
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't a tree's entry.
pub async fn tree_entry(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    path: &BStr,
    cancel: &CancelToken,
) -> Result<Option<TreeEntry>, GitError> {
    let listed = git
        .read("ls-tree")
        .args(["-z", "--full-tree"])
        .arg(commit.to_string())
        .arg("--")
        .arg(path.to_os_str_lossy())
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    parse_entry(&listed.stdout, path)
}

/// The entry for exactly `path` in `ls-tree -z` (`<mode> <type> <object>\t<path>`).
fn parse_entry(listed: &[u8], path: &BStr) -> Result<Option<TreeEntry>, GitError> {
    let unexpected = || GitError::unexpected("ls-tree", "a tree's entry");
    for record in listed.split_str(b"\0").filter(|record| !record.is_empty()) {
        let (fields, listed_path) = record.split_once_str(b"\t").ok_or_else(unexpected)?;
        if listed_path != path.as_bytes() {
            continue;
        }
        let mut fields = fields.split_str(b" ");
        let mode = fields
            .next()
            .and_then(|mode| u32::from_str_radix(mode.to_str().ok()?, 8).ok())
            .ok_or_else(unexpected)?;
        let id = fields
            .nth(1)
            .and_then(ObjectId::from_hex)
            .ok_or_else(unexpected)?;
        return Ok(Some(TreeEntry { mode, id }));
    }
    Ok(None)
}

/// Every folder in `commit`'s tree, at any depth, from the top of the work tree
/// (`ls-tree -r -d`).
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn tree_folders(
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
        .output()
        .await?;
    Ok(listed
        .stdout
        .split_str(b"\0")
        .filter(|path| !path.is_empty())
        .map(BString::from)
        .collect())
}

/// Every file `at` (a commit, or a name such as `HEAD`) holds, streamed to `each`.
///
/// Each with its path from the top and, when `sizes` is asked for, its size in bytes (`ls-tree
/// -r -z --full-tree`, `-l`). Submodules aren't files: they are left out.
///
/// Sizes need every blob's header: in a partial clone without them, git fails rather than
/// fetch them (`GIT_NO_LAZY_FETCH`, where git honours it), so leave sizes out there.
///
/// # Errors
///
/// When `at` names no commit, or git fails or is cancelled.
pub async fn tree_files(
    git: &Git,
    repository: &Repository,
    at: &str,
    sizes: bool,
    cancel: &CancelToken,
    mut each: impl FnMut(&BStr, Option<u64>),
) -> Result<(), GitError> {
    let mut command = git.read("ls-tree").args(["-r", "-z", "--full-tree"]);
    if sizes {
        command = command.arg("-l");
    }
    let mut records = crate::lines::Lines::ended_by(0);
    command
        .args(["--end-of-options", at])
        .env("GIT_NO_LAZY_FETCH", "1")
        .non_interactive()
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .stream(|chunk| {
            records.feed(chunk, &mut |record| {
                // `<mode> <type> <object>[ <size>]\t<path>`.
                if let Some((meta, path)) = record.split_once_str(b"\t") {
                    let mut fields = meta.fields_with(char::is_whitespace);
                    let kind = fields.nth(1);
                    let size = fields
                        .nth(1)
                        .and_then(|size| size.to_str().ok()?.parse().ok());
                    if kind == Some(b"blob".as_slice()) {
                        each(BStr::new(path), size);
                    }
                }
                std::ops::ControlFlow::Continue(())
            })
        })
        .await
        .map(drop)
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
pub async fn ignored_paths(
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
        .output_unchecked()
        .await?;
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

/// A path relative to a work tree as git writes one: its bytes, with `/` between folders (on
/// Windows, `\\` turned around).
#[must_use]
pub fn git_path(relative: &Path) -> BString {
    let mut bytes = <Vec<u8>>::from_path_lossy(relative).into_owned();
    if cfg!(windows) {
        for byte in &mut bytes {
            if *byte == b'\\' {
                *byte = b'/';
            }
        }
    }
    BString::from(bytes)
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_entry_at_exactly_the_path() {
        let id = "a".repeat(40);
        let listed = format!("100755 blob {id}\tbin/tool\0040000 tree {id}\tbin/tool/x\0");
        let entry = parse_entry(listed.as_bytes(), BStr::new("bin/tool"))
            .unwrap()
            .unwrap();
        assert_eq!(entry.mode, 0o100_755);
        assert_eq!(entry.id, ObjectId::from_hex(id.as_bytes()).unwrap());
        assert_eq!(
            parse_entry(listed.as_bytes(), BStr::new("bin")).unwrap(),
            None
        );
    }
}
