//! A repository's Git LFS as egit shows and changes it: whether git-lfs is installed and set up
//! here, and the patterns it tracks (read with the tab's other lists); the files it holds and
//! the locks on its server (read when the LFS page shows them); and its tasks.
//!
//! What LFS does, for whoever meets it in a repository: git commits a small pointer for each
//! tracked file, and keeps its contents in a store of their own, on the remote's LFS server.
//! Checking out turns pointers into contents (through LFS's filters, which `git lfs install`
//! sets up); without them, or before the contents are downloaded, the work tree holds pointers.

use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_git::{
    CancelToken, ConfigPlace, ConfigSnapshot, Git, LfsFile, LfsLock, LfsPattern, LfsTransfer,
    LfsVersion, Progress, Repository, lfs_files, lfs_install, lfs_lock, lfs_locks, lfs_patterns,
    lfs_prune, lfs_set_up, lfs_track, lfs_transfer, lfs_untrack, lfs_version,
};

use crate::jobs;
use crate::problem::Problem;
use crate::tasks::Outcome;

/// What egit knows of a repository's LFS.
#[derive(Debug, Default)]
pub(crate) struct LfsState {
    /// git-lfs's version; `None` when it isn't installed.
    pub(crate) version: Option<LfsVersion>,
    /// Whether its filters are configured where this repository sees them.
    pub(crate) set_up: bool,
    /// The patterns it tracks or excludes (from the top `.gitattributes` alone when git-lfs
    /// isn't installed).
    pub(crate) patterns: Vec<LfsPattern>,
}

/// What keeps a repository's LFS files from being whole here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfsTrouble {
    /// The repository keeps files in LFS, but git-lfs isn't installed.
    NotInstalled,
    /// git-lfs is installed, but its filters aren't set up for this repository.
    NotSetUp,
}

impl LfsState {
    /// Whether the repository keeps files in LFS (it tracks a pattern).
    pub(crate) fn used(&self) -> bool {
        self.patterns.iter().any(|pattern| pattern.tracked)
    }

    /// What keeps its LFS files from being whole here, if anything.
    pub(crate) fn trouble(&self) -> Option<LfsTrouble> {
        if !self.used() {
            None
        } else if self.version.is_none() {
            Some(LfsTrouble::NotInstalled)
        } else if !self.set_up {
            Some(LfsTrouble::NotSetUp)
        } else {
            None
        }
    }
}

/// How much of a repository's LFS to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reading {
    /// Only where the top `.gitattributes` gives a pattern to LFS's filter: elsewhere nothing
    /// is run (the tab's lists are read often, and most repositories don't use LFS).
    WhereUsed,
    /// Everything (the LFS page, which sets LFS up where it isn't used yet).
    All,
}

/// Reads the repository's LFS: git-lfs's version, whether it is set up here, its patterns (on
/// a job's thread), as `reading` says.
///
/// # Errors
///
/// When git can't be run, or git-lfs fails listing its patterns.
pub(crate) async fn read_lfs(
    git: &Git,
    repository: &Repository,
    reading: Reading,
) -> Result<LfsState, Problem> {
    if reading == Reading::WhereUsed && attributes_patterns(git, repository).await.is_empty() {
        return Ok(LfsState::default());
    }
    let failed = |error| Problem::git("Git LFS couldn't be read", &error);
    let version = lfs_version(git).await.map_err(failed)?;
    let config = ConfigSnapshot::read(git, ConfigPlace::Repository(repository.work_tree()))
        .await
        .map_err(failed)?;
    let patterns = if version.is_some() {
        lfs_patterns(git, repository, &CancelToken::new())
            .await
            .map_err(failed)?
    } else {
        attributes_patterns(git, repository).await
    };
    Ok(LfsState {
        version,
        set_up: lfs_set_up(&config),
        patterns,
    })
}

/// The patterns the top `.gitattributes` gives LFS's filter (`*.psd filter=lfs …`): whether
/// LFS is used at all, and what it tracks where git-lfs isn't installed to say.
async fn attributes_patterns(git: &Git, repository: &Repository) -> Vec<LfsPattern> {
    let Ok(text) = git
        .files()
        .read(&repository.work_tree().join(".gitattributes"))
        .await
    else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut fields = line
                .split(u8::is_ascii_whitespace)
                .filter(|field| !field.is_empty());
            let pattern = fields.next()?;
            let mut lfs = false;
            let mut lockable = false;
            for field in fields {
                lfs |= field == b"filter=lfs";
                lockable |= field == b"lockable";
            }
            (lfs && !pattern.starts_with(b"#")).then(|| LfsPattern {
                pattern: pattern.into(),
                source: ".gitattributes".into(),
                lockable,
                tracked: true,
            })
        })
        .collect()
}

/// The whole of a repository's LFS, for its page.
pub(crate) async fn read_all(
    git: Git,
    repository: Arc<Repository>,
) -> Result<Arc<LfsState>, Problem> {
    jobs::blocking(async move {
        read_lfs(&git, &repository, Reading::All)
            .await
            .map(Arc::new)
    })
    .await
}

/// What the LFS page shows of the files and locks, read when it opens and after tasks.
pub(crate) async fn read_lfs_files(
    git: Git,
    repository: Arc<Repository>,
) -> Result<Arc<[LfsFile]>, Problem> {
    jobs::blocking(async move {
        lfs_files(&git, &repository, &CancelToken::new())
            .await
            .map(Into::into)
            .map_err(|error| Problem::git("The LFS files couldn't be listed", &error))
    })
    .await
}

/// The locks on the remote's LFS server.
pub(crate) async fn read_lfs_locks(
    git: Git,
    repository: Arc<Repository>,
) -> Result<Arc<[LfsLock]>, Problem> {
    jobs::blocking(async move {
        lfs_locks(&git, &repository, &CancelToken::new())
            .await
            .map(Into::into)
            .map_err(|error| Problem::git("The LFS locks couldn't be read", &error))
    })
    .await
}

/// What the user asks of LFS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LfsTask {
    /// Set it up here (`install --local`).
    Install,
    /// Track a pattern (lockable or not).
    Track { pattern: String, lockable: bool },
    /// Stop tracking a pattern.
    Untrack(BString),
    /// Move its files.
    Transfer(LfsTransfer),
    /// Delete downloaded files nothing recent needs.
    Prune,
    /// Lock a file on the server.
    Lock(BString),
    /// Unlock a file (`force`: someone else's lock).
    Unlock { path: BString, force: bool },
}

impl LfsTask {
    /// What it is doing, for people.
    pub(crate) const fn doing(&self) -> &'static str {
        match self {
            Self::Install => "Setting up Git LFS",
            Self::Track { .. } => "Tracking with Git LFS",
            Self::Untrack(_) => "Untracking",
            Self::Transfer(LfsTransfer::Fetch { .. }) => "Fetching LFS files",
            Self::Transfer(LfsTransfer::Checkout) => "Checking out LFS files",
            Self::Transfer(LfsTransfer::Pull) => "Pulling LFS files",
            Self::Transfer(LfsTransfer::Push { .. }) => "Pushing LFS files",
            Self::Prune => "Pruning LFS files",
            Self::Lock(_) => "Locking",
            Self::Unlock { .. } => "Unlocking",
        }
    }

    /// How it ended, in words.
    pub(crate) fn done(&self) -> String {
        match self {
            Self::Install => {
                "Git LFS is set up here: its files check out whole, and pushes push them."
                    .to_owned()
            }
            Self::Track { pattern, .. } => format!(
                "Tracking {pattern} with Git LFS: commit .gitattributes to share it. Files \
                 committed before stay in git until they are added again."
            ),
            Self::Untrack(pattern) => format!(
                "Stopped tracking {} with Git LFS: commit .gitattributes to share it.",
                pattern.to_str_lossy()
            ),
            Self::Transfer(LfsTransfer::Fetch { all: false }) => {
                "Fetched the current branch's LFS files: check them out to replace their pointers."
                    .to_owned()
            }
            Self::Transfer(LfsTransfer::Fetch { all: true }) => {
                "Fetched every LFS file every ref holds.".to_owned()
            }
            Self::Transfer(LfsTransfer::Checkout) => {
                "Checked out the LFS files downloaded: the work tree holds their contents."
                    .to_owned()
            }
            Self::Transfer(LfsTransfer::Pull) => "Pulled the LFS files.".to_owned(),
            Self::Transfer(LfsTransfer::Push { remote }) => {
                format!("Pushed the LFS files to {}.", remote.to_str_lossy())
            }
            Self::Prune => "Pruned the LFS files nothing recent needs.".to_owned(),
            Self::Lock(path) => format!("Locked {}.", path.to_str_lossy()),
            Self::Unlock { path, .. } => format!("Unlocked {}.", path.to_str_lossy()),
        }
    }
}

/// Runs an LFS task.
pub(crate) async fn run(
    git: &Git,
    repository: &Repository,
    task: &LfsTask,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let result = match task {
        LfsTask::Install => lfs_install(git, repository).await,
        LfsTask::Track { pattern, lockable } => {
            lfs_track(git, repository, pattern, *lockable).await
        }
        LfsTask::Untrack(pattern) => lfs_untrack(git, repository, &pattern.to_str_lossy()).await,
        LfsTask::Transfer(transfer) => lfs_transfer(git, repository, transfer, progress, cancel)
            .await
            .map(drop),
        LfsTask::Prune => lfs_prune(git, repository, cancel).await.map(drop),
        LfsTask::Lock(path) => lfs_lock(git, repository, path, None).await,
        LfsTask::Unlock { path, force } => lfs_lock(git, repository, path, Some(*force)).await,
    };
    result.map(|()| Outcome::Done).map_err(|error| {
        let title = match task {
            LfsTask::Install => "Git LFS wasn't set up",
            LfsTask::Track { .. } => "The pattern wasn't tracked",
            LfsTask::Untrack(_) => "The pattern wasn't untracked",
            LfsTask::Transfer(LfsTransfer::Push { .. }) => "The LFS files weren't pushed",
            LfsTask::Transfer(_) => "The LFS files weren't fetched",
            LfsTask::Prune => "The LFS files weren't pruned",
            LfsTask::Lock(_) => "The file wasn't locked",
            LfsTask::Unlock { .. } => "The file wasn't unlocked",
        };
        Problem::git(title, &error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trouble_is_said_only_where_lfs_is_used() {
        let pattern = LfsPattern {
            pattern: "*.psd".into(),
            source: ".gitattributes".into(),
            lockable: false,
            tracked: true,
        };
        let mut state = LfsState::default();
        assert_eq!(state.trouble(), None, "no pattern: no LFS here");
        state.patterns.push(pattern);
        assert_eq!(state.trouble(), Some(LfsTrouble::NotInstalled));
        state.version = Some(LfsVersion {
            major: 3,
            minor: 4,
            patch: 1,
        });
        assert_eq!(state.trouble(), Some(LfsTrouble::NotSetUp));
        state.set_up = true;
        assert_eq!(state.trouble(), None);
    }
}
