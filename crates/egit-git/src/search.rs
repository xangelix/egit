//! Finding commits as git finds them: those whose message or author holds some text, or that
//! changed a path, among every commit a history shows (`--exclude=refs/stash --all`); and the
//! commit a name names.

use std::ops::ControlFlow;

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::lines::Lines;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// What a commit must have to be found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitFilter<'a> {
    /// Its message holds `text` (`--grep`, a line at a time).
    Message {
        /// The text, as it is (not a pattern).
        text: &'a str,
        /// Whether letters match in either case.
        fold_case: bool,
    },
    /// Its author's `Name <email>` holds `text` (`--author`).
    Author {
        /// The text, as it is (not a pattern).
        text: &'a str,
        /// Whether letters match in either case.
        fold_case: bool,
    },
    /// It changed `path` (a file, or anything in a folder; literally, from the top of the work
    /// tree), as `git log -- <path>` lists them: at a merge, the history followed is the side
    /// the path came from, and the merge is listed when it combined both sides' changes (git's
    /// default simplification; `--full-history` would list every merge differing from either
    /// side: twenty times as many in git's own history, five times as slow).
    Path(&'a BStr),
}

/// Hands each commit `filter` finds to `each`, as git finds them (not in the history's
/// order); `each` may stop the search.
///
/// # Errors
///
/// When git fails, prints something that isn't a commit's name, or is cancelled.
pub async fn find_commits(
    git: &Git,
    repository: &Repository,
    filter: CommitFilter<'_>,
    cancel: &CancelToken,
    mut each: impl FnMut(ObjectId) -> ControlFlow<()>,
) -> Result<(), GitError> {
    let mut command = git
        .read("rev-list")
        .args(["--exclude=refs/stash", "--all"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    command = match filter {
        CommitFilter::Message { text, fold_case } => command
            .arg("--fixed-strings")
            .args(fold_case.then_some("--regexp-ignore-case"))
            .arg(format!("--grep={text}")),
        CommitFilter::Author { text, fold_case } => command
            .arg("--fixed-strings")
            .args(fold_case.then_some("--regexp-ignore-case"))
            .arg(format!("--author={text}")),
        // Trees only: a partial clone that has them all fetches nothing (git that honours
        // `GIT_NO_LAZY_FETCH` would refuse to, should one be missing).
        CommitFilter::Path(path) => command
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .arg("--")
            .arg(path.to_os_str_lossy()),
    };
    let mut lines = Lines::default();
    let mut malformed = false;
    let mut handle = |line: &[u8]| {
        ObjectId::from_hex(line).map_or_else(
            || {
                malformed = true;
                ControlFlow::Break(())
            },
            &mut each,
        )
    };
    command
        .stream(|chunk| lines.feed(chunk, &mut handle))
        .await?;
    if malformed {
        return Err(GitError::unexpected(
            "rev-list",
            "a line that isn't a commit's name",
        ));
    }
    Ok(())
}

/// The commit `name` names (a branch, a tag, an abbreviated name, `HEAD~3`, …), or `None`
/// when it names none (or would read as an option).
///
/// # Errors
///
/// When git can't be run.
pub async fn resolve_commit(
    git: &Git,
    repository: &Repository,
    name: &str,
) -> Result<Option<ObjectId>, GitError> {
    if name.is_empty() || name.starts_with('-') {
        return Ok(None);
    }
    let resolved = git
        .read("rev-parse")
        .args(["--verify", "--quiet"])
        .arg(format!("{name}^{{commit}}"))
        .current_dir(repository.work_tree())
        .output_unchecked()
        .await?;
    if !resolved.status.success() {
        return Ok(None);
    }
    Ok(ObjectId::from_hex(resolved.stdout.trim_ascii()))
}
