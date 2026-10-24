//! Staging and unstaging whole files: the index made to match the work tree at some paths, or
//! `HEAD` at them.
//!
//! Paths go to git on its standard input (`--pathspec-from-file`, NUL-separated) as literal
//! paths, so any number of them, named anything, are passed as they are.

use bstr::BStr;

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// Stages `paths` as the work tree has them: changed, new, and deleted files alike
/// (`git add --all`). No paths stage nothing.
///
/// # Errors
///
/// When git fails (an index another git holds the lock of, say) or is cancelled.
pub async fn stage(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    cancel: &CancelToken,
) -> Result<(), GitError> {
    with_paths(git, repository, "add", &["--all"], paths, cancel).await
}

/// Unstages `paths`, leaving the work tree alone (`git reset`).
///
/// The index takes what `HEAD` holds at them, or nothing where `HEAD` holds nothing (or there
/// is no commit yet). A staged rename is unstaged by naming both of its paths. No paths
/// unstage nothing.
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn unstage(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    cancel: &CancelToken,
) -> Result<(), GitError> {
    with_paths(git, repository, "reset", &["--quiet"], paths, cancel).await
}

/// Runs `subcommand` with `args` on `paths`, given on standard input (nothing for no paths).
pub(crate) async fn with_paths(
    git: &Git,
    repository: &Repository,
    subcommand: &'static str,
    args: &[&str],
    paths: &[&BStr],
    cancel: &CancelToken,
) -> Result<(), GitError> {
    // Without paths, `git reset` would unstage everything: there is nothing to do.
    if paths.is_empty() {
        return Ok(());
    }
    let mut stdin = Vec::with_capacity(paths.iter().map(|path| path.len().saturating_add(1)).sum());
    for path in paths {
        stdin.extend_from_slice(path);
        stdin.push(b'\0');
    }
    git.write(subcommand)
        .args(args)
        .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
        // Paths are paths, never patterns.
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .stdin(stdin)
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}
