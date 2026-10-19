//! Patches applied to a repository: what one is (commits by mail, `git format-patch`'s; or a
//! diff), what it holds and whether it applies (read by git without applying it), and
//! applying it, as commits (`git am --3way`) or as changes (`git apply`); and a `git am`
//! stopped part-way: how far it got, and the patch it stopped at.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::integrate::{Integrated, has_conflicts};
use crate::repo::{Operation, Repository};

/// The most commits of a mailbox read for its summary (each is a `git mailinfo`).
pub const MAX_SUMMARIZED: usize = 1_000;

/// What kind of patch something is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchKind {
    /// Commits by mail (`git format-patch`'s, an mbox, an email): `git am` makes them commits.
    Mailbox,
    /// Changes alone (`git diff`'s, `diff -u`'s): `git apply` makes them changes.
    Diff,
}

/// What `bytes` is: a mailbox when it starts as a mail does (an mbox's `From ` line, or headers
/// with a `Subject:`), else a diff; `None` when it holds no diff at all.
#[must_use]
pub fn patch_kind(bytes: &[u8]) -> Option<PatchKind> {
    let has_diff = bytes
        .lines()
        .any(|line| line.starts_with(b"diff --git ") || line.starts_with(b"+++ "));
    if !has_diff {
        return None;
    }
    let mut lines = bytes
        .lines()
        .skip_while(|line| line.trim_ascii().is_empty());
    let first = lines.next()?;
    if first.starts_with(b"From ") {
        return Some(PatchKind::Mailbox);
    }
    // Mail headers: `Name: value` lines up to a blank one, a `Subject:` among them.
    let header = |line: &[u8]| {
        line.find_byte(b':').is_some_and(|colon| {
            colon > 0
                && line.get(..colon).is_some_and(|name| {
                    name.iter()
                        .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })
        })
    };
    let mut subject = false;
    for line in std::iter::once(first).chain(lines) {
        if line.trim_ascii().is_empty() {
            break;
        }
        if line.starts_with(b" ") || line.starts_with(b"\t") {
            continue;
        }
        if !header(line) {
            return Some(PatchKind::Diff);
        }
        subject |= line.starts_with(b"Subject:");
    }
    Some(if subject {
        PatchKind::Mailbox
    } else {
        PatchKind::Diff
    })
}

/// A commit a mailbox holds, as `git mailinfo` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchCommit {
    /// Its subject (`[PATCH 1/3]` taken off).
    pub subject: String,
    /// Its author, `Name <email>`.
    pub author: String,
    /// When it was written, as the mail says.
    pub date: String,
}

/// How a patch changes a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchChange {
    /// Its lines (or mode) changed.
    Modified,
    /// It is new.
    Added,
    /// It goes.
    Deleted,
    /// Moved from another path.
    Renamed {
        /// Where it was.
        from: BString,
    },
}

/// A file a patch changes, over all its commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFile {
    /// Its path, from the top of the work tree.
    pub path: BString,
    /// What happens to it.
    pub change: PatchChange,
    /// Lines added and removed (`None`: a binary file).
    pub lines: Option<(u64, u64)>,
}

/// Whether a patch applies here, as `git apply --check` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applies {
    /// As it is.
    Cleanly,
    /// Not as it is, but merged in three ways (the blobs it was made from are here), with or
    /// without `conflicts`.
    Merging {
        /// Whether the merge leaves conflicts to resolve.
        conflicts: bool,
    },
    /// Not at all: git's reason.
    Not(String),
    /// Not checked: a series of commits, each built on the one before, applies only in turn.
    Unchecked,
}

/// What a patch holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchSummary {
    /// Commits by mail, or a diff.
    pub kind: PatchKind,
    /// A mailbox's commits (the first [`MAX_SUMMARIZED`]), in order.
    pub commits: Vec<PatchCommit>,
    /// How many commits it holds in all.
    pub commit_count: usize,
    /// The files it changes, in order.
    pub files: Vec<PatchFile>,
    /// Whether it applies here.
    pub applies: Applies,
}

/// Reads what `patch` holds and whether it applies here, without applying it.
///
/// Its commits (`git mailsplit`, then `git mailinfo` each), the files it changes (`git apply
/// --numstat --summary`), and `git apply --check` (then `--3way`) for a single change.
///
/// # Errors
///
/// When it isn't a patch, or git fails or is cancelled.
pub fn summarize_patch(
    git: &Git,
    repository: &Repository,
    patch: &Arc<[u8]>,
    cancel: &CancelToken,
) -> Result<PatchSummary, GitError> {
    let kind =
        patch_kind(patch).ok_or_else(|| GitError::unexpected("apply", "nothing git can apply"))?;
    let (commits, commit_count) = match kind {
        PatchKind::Mailbox => mail_commits(git, repository, patch, cancel)?,
        PatchKind::Diff => (Vec::new(), 0),
    };
    let files = patch_files(git, repository, patch, cancel)?;
    let applies = if commit_count > 1 {
        Applies::Unchecked
    } else {
        check(git, repository, patch, cancel)?
    };
    Ok(PatchSummary {
        kind,
        commits,
        commit_count,
        files,
        applies,
    })
}

/// A mailbox's commits (the first [`MAX_SUMMARIZED`]) and how many it holds.
fn mail_commits(
    git: &Git,
    repository: &Repository,
    patch: &Arc<[u8]>,
    cancel: &CancelToken,
) -> Result<(Vec<PatchCommit>, usize), GitError> {
    let io = |source| GitError::Io {
        subcommand: "mailsplit",
        source,
    };
    let split = git.files().temp_dir("egit-mails-").map_err(io)?;
    let mut option = std::ffi::OsString::from("-o");
    option.push(split.path());
    let done = git
        .read("mailsplit")
        .arg(option)
        .stdin(Arc::clone(patch))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    let count: usize = done
        .stdout
        .trim_ascii()
        .to_str()
        .ok()
        .and_then(|count| count.parse().ok())
        .ok_or_else(|| GitError::unexpected("mailsplit", "a count of mails"))?;
    let mut commits = Vec::with_capacity(count.min(MAX_SUMMARIZED));
    let paths: Vec<PathBuf> = (1..=count.min(MAX_SUMMARIZED))
        .map(|index| split.path().join(format!("{index:04}")))
        .collect();
    let mails = git
        .files()
        .read_all(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>())
        .map_err(io)?;
    for mail in mails {
        let mail =
            mail.ok_or_else(|| GitError::unexpected("mailsplit", "a mail it said it made"))?;
        let info = git
            .read("mailinfo")
            .args(["/dev/null", "/dev/null"])
            .stdin(mail)
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output()?;
        commits.push(mail_info(&info.stdout));
    }
    Ok((commits, count))
}

/// A commit from what `git mailinfo` prints: `Author:`, `Email:`, `Subject:`, `Date:` lines.
fn mail_info(printed: &[u8]) -> PatchCommit {
    let field = |name: &[u8]| {
        printed
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|value| value.trim_ascii().to_str_lossy().into_owned())
            .unwrap_or_default()
    };
    let (name, email) = (field(b"Author:"), field(b"Email:"));
    PatchCommit {
        subject: field(b"Subject:"),
        author: if email.is_empty() {
            name
        } else {
            format!("{name} <{email}>")
        },
        date: field(b"Date:"),
    }
}

/// The files `patch` changes, each once (its lines summed over the commits).
fn patch_files(
    git: &Git,
    repository: &Repository,
    patch: &Arc<[u8]>,
    cancel: &CancelToken,
) -> Result<Vec<PatchFile>, GitError> {
    let apply = |args: &[&str]| {
        git.read("apply")
            .args(args)
            .stdin(Arc::clone(patch))
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output()
            .map(|done| done.stdout)
    };
    let numstat = apply(&["--numstat", "-z"])?;
    let summary = apply(&["--summary"])?;
    let mut files: Vec<PatchFile> = Vec::new();
    let mut fields = numstat.split_str(b"\0");
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn_str(3, b"\t");
        let (added, removed, named) = (parts.next(), parts.next(), parts.next());
        let (Some(added), Some(removed), Some(named)) = (added, removed, named) else {
            return Err(GitError::unexpected("apply", "a file's line counts"));
        };
        let lines = added
            .to_str()
            .ok()
            .and_then(|added| added.parse().ok())
            .zip(
                removed
                    .to_str()
                    .ok()
                    .and_then(|removed| removed.parse().ok()),
            );
        // A rename: an empty path, then the old and new ones.
        let (file, change) = if named.is_empty() {
            let from = fields.next().unwrap_or_default();
            let to = fields.next().unwrap_or_default();
            (
                BString::from(to),
                PatchChange::Renamed {
                    from: BString::from(from),
                },
            )
        } else {
            (BString::from(named), summary_change(&summary, named))
        };
        if let Some(known) = files.iter_mut().find(|known| known.path == file) {
            known.lines = known.lines.zip(lines).map(|((a, r), (more_a, more_r))| {
                (a.saturating_add(more_a), r.saturating_add(more_r))
            });
        } else {
            files.push(PatchFile {
                path: file,
                change,
                lines,
            });
        }
    }
    Ok(files)
}

/// Whether `--summary` says `path` is created or deleted.
fn summary_change(summary: &[u8], path: &[u8]) -> PatchChange {
    for line in summary.lines() {
        let line = line.trim_ascii_start();
        let rest = |prefix: &[u8]| {
            line.strip_prefix(prefix)
                .and_then(|rest| rest.split_once_str(b" "))
                .is_some_and(|(_, named)| named == path)
        };
        if rest(b"create mode ") {
            return PatchChange::Added;
        }
        if rest(b"delete mode ") {
            return PatchChange::Deleted;
        }
    }
    PatchChange::Modified
}

/// `git apply --check`, then with `--3way` where that fails.
fn check(
    git: &Git,
    repository: &Repository,
    patch: &Arc<[u8]>,
    cancel: &CancelToken,
) -> Result<Applies, GitError> {
    let check = |three_way: bool| {
        git.read("apply")
            .arg("--check")
            .args(three_way.then_some("--3way"))
            .stdin(Arc::clone(patch))
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output_unchecked()
    };
    if check(false)?.status.success() {
        return Ok(Applies::Cleanly);
    }
    let merged = check(true)?;
    let said = merged.stderr.message();
    Ok(if merged.status.success() {
        Applies::Merging {
            conflicts: said.contains("with conflicts"),
        }
    } else {
        Applies::Not(said)
    })
}

/// How a patch becomes part of the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyAs {
    /// A mailbox's commits, made one by one (`git am --3way`), with a `Signed-off-by` line when
    /// `sign_off`.
    Commits {
        /// Whether each gains a `Signed-off-by` line.
        sign_off: bool,
    },
    /// Changes in the work tree: `staged` (`git apply --3way`, which stages them and merges
    /// where they don't apply as they are), or not (`git apply`, all of it or nothing).
    Changes {
        /// Whether they are staged (and merged where they don't apply as they are).
        staged: bool,
    },
}

/// Applies `patch` to the current branch, as `how` says.
///
/// # Errors
///
/// When it doesn't apply (and nothing is left to resolve), or git fails or is cancelled.
pub fn apply_patch(
    git: &Git,
    repository: &Repository,
    patch: &Arc<[u8]>,
    how: ApplyAs,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let command = match how {
        ApplyAs::Commits { sign_off } => git
            .write("am")
            .args(["--3way", "--quiet"])
            .args(sign_off.then_some("--signoff"))
            .prompt_context("Signing the commit"),
        ApplyAs::Changes { staged } => git.write("apply").args(staged.then_some("--3way")),
    };
    let done = command
        .stdin(Arc::clone(patch))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    if done.status.success() {
        return Ok(Integrated::Done);
    }
    let applying = repository
        .operations(git)
        .map_err(|source| GitError::Io {
            subcommand: "am",
            source,
        })?
        .contains(&Operation::ApplyMailbox);
    let subcommand = match how {
        ApplyAs::Commits { .. } => "am",
        ApplyAs::Changes { .. } => "apply",
    };
    if has_conflicts(git, repository, cancel)? {
        Ok(Integrated::Conflicts)
    } else if applying {
        Ok(Integrated::Stopped)
    } else {
        Err(GitError::failed(subcommand, done.status, done.stderr))
    }
}

/// How far a `git am` stopped part-way got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxProgress {
    /// The patch it stopped at (from 1).
    pub next: u32,
    /// How many patches it applies.
    pub last: u32,
    /// The stopped patch's subject.
    pub subject: String,
}

impl Repository {
    /// How far a `git am` stopped part-way got (its `rebase-apply/next`, `last`, and `info`),
    /// or `None` when none is.
    ///
    /// # Errors
    ///
    /// When the files can't be read.
    pub fn mailbox_progress(&self, git: &Git) -> io::Result<Option<MailboxProgress>> {
        let dir = self.git_dir().join("rebase-apply");
        let paths = ["applying", "next", "last", "info"].map(|name| dir.join(name));
        let files = git.files();
        if !files.exists(&paths[0])? {
            return Ok(None);
        }
        let read = files.read_all(&[&paths[1], &paths[2], &paths[3]])?;
        let [next, last, info] = <[Option<Vec<u8>>; 3]>::try_from(read)
            .map_err(|_| io::Error::other("a stopped `git am`'s files came back garbled"))?;
        let number = |read: Option<Vec<u8>>| -> u32 {
            read.unwrap_or_default()
                .trim_ascii()
                .to_str()
                .ok()
                .and_then(|number| number.parse().ok())
                .unwrap_or(0)
        };
        let info = info.unwrap_or_default();
        let subject = info
            .lines()
            .find_map(|line| line.strip_prefix(b"Subject:"))
            .map(|subject| subject.trim_ascii().to_str_lossy().into_owned())
            .unwrap_or_default();
        Ok(Some(MailboxProgress {
            next: number(next),
            last: number(last),
            subject,
        }))
    }
}

/// The patch a stopped `git am` is at, its diff (`git am --show-current-patch=diff`).
///
/// # Errors
///
/// When no `git am` is stopped, or git fails.
pub fn current_patch(git: &Git, repository: &Repository) -> Result<Vec<u8>, GitError> {
    git.read("am")
        .arg("--show-current-patch=diff")
        .current_dir(repository.work_tree())
        .output()
        .map(|done| done.stdout)
}

/// `name`, if it holds bytes that are a patch, as a person names one: `.patch`, `.diff`,
/// `.mbox`, `.eml`.
#[must_use]
pub fn looks_like_patch(name: &BStr) -> bool {
    let lower = name.to_ascii_lowercase();
    [".patch", ".diff", ".mbox", ".eml"]
        .iter()
        .any(|extension| lower.ends_with(extension.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_is_told_from_a_mailbox() {
        let diff = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(patch_kind(diff), Some(PatchKind::Diff));
        let mut mbox =
            b"From 1234 Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\nSubject: [PATCH] x\n\n---\n"
                .to_vec();
        mbox.extend_from_slice(diff);
        assert_eq!(patch_kind(&mbox), Some(PatchKind::Mailbox));
        let mut mail = b"From: A <a@b>\nDate: today\nSubject: x\n\nWhy.\n---\n".to_vec();
        mail.extend_from_slice(diff);
        assert_eq!(patch_kind(&mail), Some(PatchKind::Mailbox));
        let mut shown = b"commit 1234\nAuthor: A <a@b>\n\n    x\n\n".to_vec();
        shown.extend_from_slice(diff);
        assert_eq!(patch_kind(&shown), Some(PatchKind::Diff), "`git show`'s");
        assert_eq!(patch_kind(b"Hello\nworld\n"), None);
        assert_eq!(patch_kind(b"From: A\nSubject: no diff\n\nhi\n"), None);
    }

    #[test]
    fn mailinfo_s_fields_make_a_commit() {
        let commit = mail_info(b"Author: Ada\nEmail: ada@x\nSubject: Fix it\nDate: Mon\n\n");
        assert_eq!(commit.author, "Ada <ada@x>");
        assert_eq!(commit.subject, "Fix it");
        assert_eq!(commit.date, "Mon");
    }

    #[test]
    fn patch_files_are_named_by_their_extension() {
        for name in ["fix.patch", "x.DIFF", "series.mbox", "mail.eml"] {
            assert!(looks_like_patch(BStr::new(name)), "{name}");
        }
        assert!(!looks_like_patch(BStr::new("notes.txt")));
    }
}
