//! Where refs have been: a ref's reflog (`HEAD`'s, a branch's), each move with what made it;
//! where a ref is now; and refs made or removed only where they were expected (`update-ref`
//! with the old value, so a ref someone else moved meanwhile is left alone).

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// One move of a ref, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflogEntry {
    /// Its place in the reflog: `HEAD@{n}`'s `n`.
    pub index: usize,
    /// Where the ref went.
    pub commit: ObjectId,
    /// When (Unix time).
    pub time: i64,
    /// What moved it, as git wrote it ("commit: Fix the parser", "reset: moving to HEAD~1",
    /// "checkout: moving from main to topic").
    pub action: BString,
    /// The commit's subject.
    pub subject: BString,
}

/// The first `limit` moves of `reference` (`HEAD`, or a ref's full name), newest first (`git log
/// --walk-reflogs`); none when it has no reflog.
///
/// # Errors
///
/// When git fails otherwise, prints what isn't a reflog entry, or is cancelled.
pub fn reflog(
    git: &Git,
    repository: &Repository,
    reference: &BStr,
    limit: usize,
    cancel: &CancelToken,
) -> Result<Vec<ReflogEntry>, GitError> {
    if !is_ref_name(reference) {
        return Ok(Vec::new());
    }
    let done = git
        .read("log")
        .args([
            "--walk-reflogs",
            "-z",
            "--date=unix",
            "--format=%H%x1f%gd%x1f%gs%x1f%s",
        ])
        .arg(format!("--max-count={limit}"))
        .arg(reference.to_os_str_lossy())
        .arg("--")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    if !done.status.success() {
        // No reflog (no commit yet, no such ref, or none kept): nothing to show.
        let said = done.stderr.message();
        return if [
            "does not have any commits",
            "unknown revision",
            "bad revision",
        ]
        .iter()
        .any(|reason| said.contains(reason))
        {
            Ok(Vec::new())
        } else {
            Err(GitError::failed("log", done.status, done.stderr))
        };
    }
    let malformed = || GitError::unexpected("log", "a reflog entry");
    done.stdout
        .split_str(b"\0")
        .filter(|record| !record.is_empty())
        .enumerate()
        .map(|(index, record)| {
            let mut fields = record.splitn_str(4, b"\x1f");
            let (Some(id), Some(selector), Some(action), Some(subject)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(malformed());
            };
            // `HEAD@{1700000000}`, by `--date=unix`.
            let time = selector
                .rsplit_once_str(b"@{")
                .and_then(|(_, rest)| rest.strip_suffix(b"}"))
                .and_then(|time| time.to_str().ok())
                .and_then(|time| time.parse().ok())
                .ok_or_else(malformed)?;
            Ok(ReflogEntry {
                index,
                commit: ObjectId::from_hex(id.trim_ascii()).ok_or_else(malformed)?,
                time,
                action: BString::from(action),
                subject: BString::from(subject),
            })
        })
        .collect()
}

/// Whether `name` is `HEAD` or a full ref name (never something git could take for an option).
fn is_ref_name(name: &BStr) -> bool {
    name == "HEAD" || name.starts_with(b"refs/")
}

/// The object the ref `name` (`HEAD`, or a full name) points to (an annotated tag's own, not
/// its commit), or `None` when there is no such ref (`rev-parse --verify --quiet`).
///
/// # Errors
///
/// When git can't be run.
pub fn ref_target(
    git: &Git,
    repository: &Repository,
    name: &BStr,
) -> Result<Option<ObjectId>, GitError> {
    if !is_ref_name(name) {
        return Ok(None);
    }
    let done = git
        .read("rev-parse")
        .args(["--verify", "--quiet"])
        .arg(name.to_os_str_lossy())
        .current_dir(repository.work_tree())
        .output_unchecked()?;
    Ok(done
        .status
        .success()
        .then(|| ObjectId::from_hex(done.stdout.trim_ascii()))
        .flatten())
}

/// Makes the ref `name` (a full name) at `at`, unless it exists (`update-ref <name> <at>
/// <zero>`).
///
/// # Errors
///
/// When it exists already, or git fails or is cancelled.
pub fn create_ref(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    at: ObjectId,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("update-ref")
        .arg("--")
        .arg(name.to_os_str_lossy())
        .arg(at.to_string())
        .arg(at.format().zero().to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

/// Removes the ref `name` (a full name) if it is still at `was` (`update-ref -d <name> <was>`).
///
/// # Errors
///
/// When it moved, or git fails or is cancelled.
pub fn delete_ref(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    was: ObjectId,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("update-ref")
        .args(["-d", "--"])
        .arg(name.to_os_str_lossy())
        .arg(was.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}
