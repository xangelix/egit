//! Commits as patches to mail or apply elsewhere (`git format-patch`): their messages and
//! changes, as `git am` takes them, one commit's, a range's, or any commits' (a series); any
//! commits' changes combined into one diff; and the uncommitted changes as a diff `git apply`
//! takes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bstr::ByteSlice as _;

use crate::cancel::CancelToken;
use crate::command::GitCommand;
use crate::error::GitError;
use crate::git::Git;
use crate::history::{ListedCommit, list_commits};
use crate::object_id::ObjectId;
use crate::rebase::MAX_PLANNED;
use crate::repo::Repository;

/// The longest name [`patch_file_name`] gives, as git's own.
const MAX_NAME: usize = 64;

/// How a diff is made into a patch any `git apply` takes: binary files too, standard `a/` and
/// `b/` prefixes, no external diff or text conversion, paths from the top.
const PATCH_DIFF: [&str; 6] = [
    "--binary",
    "--default-prefix",
    "--no-ext-diff",
    "--no-textconv",
    "--no-relative",
    "--no-color",
];

/// The date every patch `format-patch` makes starts with (after `From <name>`), marking where
/// each starts in a mailbox.
const MAILBOX_DATE: &[u8] = b" Mon Sep 17 00:00:00 2001";

/// The most commits a combined patch lists above its diff.
const LISTED: usize = 100;

/// Commits made into patches together: the patches' text, a mailbox as `git am` takes it, and
/// how many it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Series {
    /// The patches, the oldest first.
    pub text: Vec<u8>,
    /// How many: merges are left out, as git leaves them.
    pub patches: usize,
}

/// Combining commits into one patch failed.
#[derive(Debug, thiserror::Error)]
pub enum CombineError {
    /// No commits were given.
    #[error("no commits to combine")]
    Nothing,
    /// The commit doesn't apply onto those before it: it needs a commit left out between them.
    #[error("{0} conflicts with the commits before it")]
    Conflict(ObjectId),
    /// Running git failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// `commit` as a patch (`git format-patch -1 --stdout`, with git's standard `a/` and `b/`
/// prefixes whatever `diff.noprefix` says, so `git am` takes it as it is).
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn format_patch(
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
        .await
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
pub async fn format_patches(
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
        .output_unchecked()
        .await?;
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
        .output_unchecked()
        .await?;
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
        .output()
        .await?;
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
    let command = git
        .read("format-patch")
        .arg("--default-prefix")
        .args(&range)
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    written_into(git, command, folder).await
}

/// Runs `command`, a `format-patch`, writing its patches into `folder` on this computer: with
/// git here, `-o` it; with git on another, into a folder there, each then read back and written
/// here by git's own name. Returns the files written.
async fn written_into(
    git: &Git,
    command: GitCommand,
    folder: &Path,
) -> Result<Vec<PathBuf>, GitError> {
    let listed = |written: &[u8]| -> Vec<PathBuf> {
        written
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| PathBuf::from(line.to_os_str_lossy().into_owned()))
            .collect()
    };
    if git.host().is_local() {
        let written = command.arg("-o").arg(folder).output().await?;
        return Ok(listed(&written.stdout));
    }
    let io = |source| GitError::Io {
        subcommand: "format-patch",
        source,
    };
    let files = git.files();
    let there = files.temp_dir("egit-patches-").await.map_err(io)?;
    let written = command.arg("-o").arg(there.path()).output().await?;
    let made = listed(&written.stdout);
    let read = files
        .read_all(&made.iter().map(PathBuf::as_path).collect::<Vec<_>>())
        .await
        .map_err(io)?;
    std::fs::create_dir_all(folder).map_err(io)?;
    made.iter()
        .zip(read)
        .map(|(path, bytes)| {
            let name = path.file_name().unwrap_or(path.as_os_str());
            let here = folder.join(name);
            let bytes = bytes
                .ok_or_else(|| GitError::unexpected("format-patch", "a patch it said it wrote"))?;
            std::fs::write(&here, bytes).map_err(io)?;
            Ok(here)
        })
        .collect()
}

/// `commits` as patches, one each, in one text: a mailbox `git am` takes.
///
/// `commits` are any, the newest first as a history lists them; the patches are the oldest
/// first (`format-patch --stdout --no-walk=unsorted`, their names on standard input, so any
/// number of them). Merges are left out, as git leaves them.
///
/// # Errors
///
/// When one isn't a commit here, or git fails or is cancelled.
pub async fn format_series(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    cancel: &CancelToken,
) -> Result<Series, GitError> {
    let text = series(git, repository, commits, cancel)
        .arg("--stdout")
        .output()
        .await?
        .stdout;
    let patches = text
        .lines()
        .filter(|line| line.starts_with(b"From ") && line.ends_with(MAILBOX_DATE))
        .count();
    Ok(Series { text, patches })
}

/// `commits` (any, the newest first) as patches, one each, the oldest first, into `folder`
/// (`format-patch -o`, git naming them `0001-…`); merges are left out. Returns the files
/// written.
///
/// # Errors
///
/// When one isn't a commit here, or git fails or is cancelled.
pub async fn format_series_into(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    folder: &Path,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, GitError> {
    written_into(git, series(git, repository, commits, cancel), folder).await
}

/// `format-patch` of exactly `commits`, given on standard input: git lists them the other way
/// round, so the newest first become the oldest first.
fn series(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    cancel: &CancelToken,
) -> GitCommand {
    git.read("format-patch")
        .args(["--default-prefix", "--no-walk=unsorted", "--stdin"])
        .current_dir(repository.work_tree())
        .stdin(names(commits))
        .cancel_token(cancel)
}

/// Commits' names, a line each.
fn names(commits: &[ObjectId]) -> Vec<u8> {
    let mut names = Vec::with_capacity(commits.len().saturating_mul(65));
    for commit in commits {
        names.extend_from_slice(commit.to_string().as_bytes());
        names.push(b'\n');
    }
    names
}

/// `commits`' changes (any, the newest first) as one patch `git apply` takes, under a list of
/// the commits (which `git apply` passes over).
///
/// The diff runs from the oldest one's parent (or nothing, for a root commit) to the newest,
/// when each is the parent of the next; otherwise each commit's changes (against its first
/// parent) are merged in turn onto those before it, the oldest first (`merge-tree
/// --write-tree`, in the object store alone), as cherry-picking them one by one would.
///
/// # Errors
///
/// [`CombineError::Conflict`] when a commit doesn't apply onto those before it,
/// [`CombineError::Nothing`] without commits, and otherwise when git fails or is cancelled.
pub async fn combined_patch(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    cancel: &CancelToken,
) -> Result<Vec<u8>, CombineError> {
    let listed = list_commits(git, repository, commits, cancel).await?;
    let (Some(newest), Some(oldest)) = (listed.first(), listed.last()) else {
        return Err(CombineError::Nothing);
    };
    let base = match oldest.parents.first() {
        Some(parent) => *parent,
        None => empty_tree(git, repository, cancel).await?,
    };
    let follows = listed
        .iter()
        .zip(listed.iter().skip(1))
        .all(|(newer, older)| newer.parents.first() == Some(&older.id));
    let result = if follows {
        newest.id
    } else {
        replay(
            git,
            repository,
            &listed,
            oldest.parents.first().copied(),
            cancel,
        )
        .await?
    };
    let diff = git
        .read("diff")
        .args(PATCH_DIFF)
        .arg(base.to_string())
        .arg(result.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?
        .stdout;
    let mut patch = heading(&listed);
    patch.extend_from_slice(&diff);
    Ok(patch)
}

/// Merges each of `listed` (the newest first), the oldest first, onto `start` (the oldest's
/// parent; `None`: it is a root commit) and what came before it; returns a commit holding them
/// all.
async fn replay(
    git: &Git,
    repository: &Repository,
    listed: &[ListedCommit],
    start: Option<ObjectId>,
    cancel: &CancelToken,
) -> Result<ObjectId, CombineError> {
    let mut onto = start;
    for commit in listed.iter().rev() {
        let (parent, at) = match (commit.parents.first(), onto) {
            (Some(parent), Some(at)) if *parent != at => (parent, at),
            // Following what came before (a root commit: nothing), it is what they make.
            (Some(_), Some(_)) | (None, None) => {
                onto = Some(commit.id);
                continue;
            }
            // A root commit after others has nothing to be replayed against.
            _ => return Err(CombineError::Conflict(commit.id)),
        };
        let merged = git
            .write("merge-tree")
            .args(["--write-tree", &format!("--merge-base={parent}")])
            .arg(at.to_string())
            .arg(commit.id.to_string())
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output_unchecked()
            .await?;
        match merged.status.code() {
            Some(0) => {}
            Some(1) => return Err(CombineError::Conflict(commit.id)),
            _ => return Err(GitError::failed("merge-tree", merged.status, merged.stderr).into()),
        }
        let tree = merged
            .stdout
            .lines()
            .next()
            .and_then(ObjectId::from_hex)
            .ok_or_else(|| GitError::unexpected("merge-tree", "a tree"))?;
        onto = Some(commit_of(git, repository, tree, cancel).await?);
    }
    onto.ok_or(CombineError::Nothing)
}

/// A commit of `tree` alone, for `merge-tree` to merge onto: never referred to, by no one in
/// particular, unsigned.
pub(crate) async fn commit_of(
    git: &Git,
    repository: &Repository,
    tree: ObjectId,
    cancel: &CancelToken,
) -> Result<ObjectId, GitError> {
    let made = git
        .write("commit-tree")
        .args([
            "--no-gpg-sign",
            "-m",
            "egit: worked out, referred to by nothing",
        ])
        .arg(tree.to_string())
        .env("GIT_AUTHOR_NAME", "egit")
        .env("GIT_AUTHOR_EMAIL", "egit@localhost")
        .env("GIT_COMMITTER_NAME", "egit")
        .env("GIT_COMMITTER_EMAIL", "egit@localhost")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    ObjectId::from_hex(made.stdout.trim_ascii())
        .ok_or_else(|| GitError::unexpected("commit-tree", "a commit"))
}

/// The empty tree, in the repository's object format.
async fn empty_tree(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<ObjectId, GitError> {
    let hashed = git
        .write("hash-object")
        .args(["-w", "-t", "tree", "--stdin"])
        .current_dir(repository.work_tree())
        .stdin(Vec::new())
        .cancel_token(cancel)
        .output()
        .await?;
    ObjectId::from_hex(hashed.stdout.trim_ascii())
        .ok_or_else(|| GitError::unexpected("hash-object", "a tree"))
}

/// What a combined patch says above its diff: the commits in it, the oldest first.
fn heading(listed: &[ListedCommit]) -> Vec<u8> {
    let mut text = if listed.len() == 1 {
        "The changes of 1 commit:\n".to_owned()
    } else {
        format!(
            "The changes of {} commits together, the oldest first:\n",
            listed.len()
        )
    };
    for commit in listed.iter().rev().take(LISTED) {
        // Writing into a `String` can't fail.
        let _ = writeln!(text, "  {} {}", commit.id.abbreviated(12), commit.subject);
    }
    if listed.len() > LISTED {
        let _ = writeln!(text, "  and {} more", listed.len().saturating_sub(LISTED));
    }
    text.push('\n');
    text.into_bytes()
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
pub async fn changes_patch(
    git: &Git,
    repository: &Repository,
    changes: Changes,
    cancel: &CancelToken,
) -> Result<Vec<u8>, GitError> {
    git.read("diff")
        .args(PATCH_DIFF)
        .arg(match changes {
            Changes::Staged => "--cached",
            Changes::All => "HEAD",
        })
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await
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
