//! Git LFS, through its own git subcommand (`git lfs`): whether it is installed and set up, the
//! patterns it tracks and the files it holds (its `--json` output: git-lfs 3.4, Ubuntu 24.04's,
//! has it; patterns are read from its listing where `--json` isn't there), setting it up,
//! tracking and untracking, moving its files (fetch, check out, pull, push), pruning, and
//! locks. LFS keeps files' contents apart from git: a commit holds a small pointer, and the
//! contents come from the remote's LFS store when checked out.

use bstr::{BString, ByteSlice as _};
use serde::Deserialize;

use crate::cancel::CancelToken;
use crate::config::ConfigSnapshot;
use crate::error::GitError;
use crate::git::Git;
use crate::progress::Progress;
use crate::repo::Repository;

/// Git LFS's version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LfsVersion {
    /// Its major version.
    pub major: u32,
    /// Its minor version.
    pub minor: u32,
    /// Its patch.
    pub patch: u32,
}

impl std::fmt::Display for LfsVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl LfsVersion {
    /// Reads `git-lfs/3.8.0 (GitHub; linux amd64; go 1.27)`.
    fn parse(text: &[u8]) -> Option<Self> {
        let rest = text.trim_ascii().strip_prefix(b"git-lfs/")?;
        let end = rest
            .iter()
            .position(|&byte| byte != b'.' && !byte.is_ascii_digit())
            .unwrap_or(rest.len());
        let mut parts = rest.get(..end)?.split(|&byte| byte == b'.');
        let mut number = || -> Option<u32> { parts.next()?.to_str().ok()?.parse().ok() };
        Some(Self {
            major: number()?,
            minor: number().unwrap_or(0),
            patch: number().unwrap_or(0),
        })
    }
}

/// Git LFS's version, or `None` when it isn't installed (`git lfs version` fails).
///
/// # Errors
///
/// When git can't be run.
pub async fn lfs_version(git: &Git) -> Result<Option<LfsVersion>, GitError> {
    let done = git
        .read("lfs")
        .arg("version")
        .current_dir(crate::git::outside_repositories())
        .output_unchecked()
        .await?;
    Ok(done
        .status
        .success()
        .then(|| LfsVersion::parse(&done.stdout))
        .flatten())
}

/// Whether LFS's filters are configured where `config` (a repository's) sees them: without them
/// git checks LFS files out as their pointers.
#[must_use]
pub fn lfs_set_up(config: &ConfigSnapshot) -> bool {
    ["filter.lfs.process", "filter.lfs.smudge"]
        .iter()
        .any(|key| {
            config
                .get(key)
                .and_then(|entry| entry.value)
                .is_some_and(|value| !value.is_empty())
        })
}

/// A pattern of paths LFS tracks (or excludes), as a `.gitattributes` file names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsPattern {
    /// The pattern (`*.psd`, `assets/**`).
    pub pattern: BString,
    /// The `.gitattributes` file it is in.
    pub source: BString,
    /// Whether its files are locked before they are changed (`lockable`).
    pub lockable: bool,
    /// Whether it tracks the files (else it excludes them: `-filter`).
    pub tracked: bool,
}

/// `git lfs track --json`'s output.
#[derive(Deserialize)]
struct Patterns {
    patterns: Option<Vec<PatternEntry>>,
}

#[derive(Deserialize)]
struct PatternEntry {
    pattern: String,
    source: String,
    #[serde(default)]
    lockable: bool,
    #[serde(default = "tracked")]
    tracked: bool,
}

const fn tracked() -> bool {
    true
}

/// The patterns LFS tracks or excludes here (`git lfs track --json`, or its listing).
///
/// # Errors
///
/// When git LFS fails or prints what isn't a listing.
pub async fn lfs_patterns(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Vec<LfsPattern>, GitError> {
    let done = git
        .read("lfs")
        .args(["track", "--json"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()
        .await?;
    if done.status.success() {
        let read: Patterns = serde_json::from_slice(&done.stdout)
            .map_err(|error| GitError::unexpected("lfs track", error.to_string()))?;
        return Ok(read
            .patterns
            .unwrap_or_default()
            .into_iter()
            .map(|entry| LfsPattern {
                pattern: entry.pattern.into(),
                source: entry.source.into(),
                lockable: entry.lockable,
                tracked: entry.tracked,
            })
            .collect());
    }
    // An LFS without `--json` for `track`: its listing.
    let listed = git
        .read("lfs")
        .arg("track")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    Ok(parse_listing(&listed.stdout))
}

/// Reads `git lfs track`'s listing: `    *.psd (.gitattributes)` under "Listing tracked
/// patterns", then excluded ones.
fn parse_listing(text: &[u8]) -> Vec<LfsPattern> {
    let mut tracked = true;
    let mut patterns = Vec::new();
    for line in text.lines() {
        if line.starts_with(b"Listing excluded") {
            tracked = false;
            continue;
        }
        let Some(entry) = line.strip_prefix(b"    ") else {
            continue;
        };
        let (pattern, source) = match entry.rfind(b" (") {
            Some(open) if entry.ends_with(b")") => (
                entry.get(..open).unwrap_or_default(),
                entry
                    .get(open.saturating_add(2)..entry.len().saturating_sub(1))
                    .unwrap_or_default(),
            ),
            _ => (entry, &b""[..]),
        };
        let (source, lockable) = source
            .strip_suffix(b" [lockable]")
            .map_or((source, false), |source| (source, true));
        patterns.push(LfsPattern {
            pattern: pattern.into(),
            source: source.into(),
            lockable,
            tracked,
        });
    }
    patterns
}

/// A file LFS holds at `HEAD`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsFile {
    /// Its path, from the top of the work tree.
    pub path: BString,
    /// Its contents' size, in bytes.
    pub size: u64,
    /// Whether the work tree holds its contents (else its pointer).
    pub checked_out: bool,
    /// Whether its contents are in this repository's LFS store.
    pub downloaded: bool,
    /// Its contents' object name (SHA-256).
    pub oid: BString,
}

/// `git lfs ls-files --json`'s output.
#[derive(Deserialize)]
struct Files {
    files: Option<Vec<FileEntry>>,
}

#[derive(Deserialize)]
struct FileEntry {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    checkout: bool,
    #[serde(default)]
    downloaded: bool,
    #[serde(default)]
    oid: String,
}

/// The files LFS holds at `HEAD` (`git lfs ls-files --json`): none before the first commit.
///
/// # Errors
///
/// When git LFS fails or prints what isn't a listing.
pub async fn lfs_files(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Vec<LfsFile>, GitError> {
    let done = git
        .read("lfs")
        .args(["ls-files", "--json"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()
        .await?;
    if !done.status.success() {
        if done.stderr.message().contains("HEAD") {
            return Ok(Vec::new());
        }
        return Err(GitError::failed("lfs ls-files", done.status, done.stderr));
    }
    let read: Files = serde_json::from_slice(&done.stdout)
        .map_err(|error| GitError::unexpected("lfs ls-files", error.to_string()))?;
    Ok(read
        .files
        .unwrap_or_default()
        .into_iter()
        .map(|entry| LfsFile {
            path: entry.name.into(),
            size: entry.size,
            checked_out: entry.checkout,
            downloaded: entry.downloaded,
            oid: entry.oid.into(),
        })
        .collect())
}

/// Sets LFS up in this repository (`git lfs install --local`): its filters in the repository's
/// configuration and its hooks (pushes then push LFS files too).
///
/// # Errors
///
/// When git LFS fails.
pub async fn lfs_install(git: &Git, repository: &Repository) -> Result<(), GitError> {
    git.write("lfs")
        .args(["install", "--local"])
        .current_dir(repository.work_tree())
        .output()
        .await
        .map(drop)
}

/// Tracks `pattern` with LFS (`git lfs track`; `--lockable`: files locked before they change).
///
/// It adds a line to the top `.gitattributes`, to commit. Files already committed stay in git
/// until they are added again (or the history is migrated).
///
/// # Errors
///
/// When git LFS fails (a pattern it refuses).
pub async fn lfs_track(
    git: &Git,
    repository: &Repository,
    pattern: &str,
    lockable: bool,
) -> Result<(), GitError> {
    let mut command = git
        .write("lfs")
        .arg("track")
        .current_dir(repository.work_tree());
    if lockable {
        command = command.arg("--lockable");
    }
    command.arg("--").arg(pattern).output().await.map(drop)
}

/// Stops tracking `pattern` with LFS (`git lfs untrack`): its line leaves `.gitattributes`.
/// Files committed through LFS stay pointers until added again.
///
/// # Errors
///
/// When git LFS fails.
pub async fn lfs_untrack(
    git: &Git,
    repository: &Repository,
    pattern: &str,
) -> Result<(), GitError> {
    git.write("lfs")
        .args(["untrack", "--"])
        .arg(pattern)
        .current_dir(repository.work_tree())
        .output()
        .await
        .map(drop)
}

/// How LFS files are moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LfsTransfer {
    /// Download the current branch's files' contents into the store (`fetch`; `all`: every
    /// ref's, every commit's).
    Fetch {
        /// Every ref's and commit's files, not only the current branch's.
        all: bool,
    },
    /// Replace pointers in the work tree with contents already downloaded (`checkout`).
    Checkout,
    /// Fetch, then check out (`pull`).
    Pull,
    /// Upload the files every local ref holds to `remote` (`push --all`).
    Push {
        /// The remote whose store takes them.
        remote: BString,
    },
}

/// Moves LFS files as `transfer` says, its progress into `progress`, asking for credentials
/// through askpass.
///
/// # Errors
///
/// When git LFS fails (the remote's store refuses, or lacks a file) or is cancelled.
pub async fn lfs_transfer(
    git: &Git,
    repository: &Repository,
    transfer: &LfsTransfer,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<String, GitError> {
    let mut command = git
        .write("lfs")
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel);
    command = match transfer {
        LfsTransfer::Fetch { all: false } => {
            command.arg("fetch").prompt_context("Fetching LFS files")
        }
        LfsTransfer::Fetch { all: true } => command
            .args(["fetch", "--all"])
            .prompt_context("Fetching every LFS file"),
        LfsTransfer::Checkout => command.arg("checkout"),
        LfsTransfer::Pull => command.arg("pull").prompt_context("Pulling LFS files"),
        LfsTransfer::Push { remote } => command
            .args(["push", "--all", "--"])
            .arg(remote.to_os_str_lossy())
            .prompt_context("Pushing LFS files"),
    };
    let done = command.output().await?;
    Ok(done.stderr.message())
}

/// Deletes downloaded LFS files nothing recent needs (`git lfs prune`: not the current
/// checkout's, recent refs', or unpushed commits'); returns what it said.
///
/// # Errors
///
/// When git LFS fails or is cancelled.
pub async fn lfs_prune(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<String, GitError> {
    let done = git
        .write("lfs")
        .arg("prune")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    let mut said = done.stdout.to_str_lossy().trim().to_owned();
    if said.is_empty() {
        said = done.stderr.message();
    }
    Ok(said)
}

/// A file locked on the remote's LFS server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsLock {
    /// The server's name for the lock.
    pub id: String,
    /// The file locked.
    pub path: BString,
    /// Who holds it.
    pub owner: String,
    /// When it was locked, as the server wrote it.
    pub locked_at: String,
}

/// `git lfs locks --json`'s output: a list.
#[derive(Deserialize)]
struct LockEntry {
    id: String,
    path: String,
    #[serde(default)]
    owner: Option<Owner>,
    #[serde(default)]
    locked_at: String,
}

#[derive(Deserialize)]
struct Owner {
    name: String,
}

/// The files locked on the remote's LFS server (`git lfs locks --json`).
///
/// # Errors
///
/// When git LFS fails (a server without locking, or none reachable) or is cancelled.
pub async fn lfs_locks(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Vec<LfsLock>, GitError> {
    let done = git
        .read("lfs")
        .args(["locks", "--json"])
        .prompt_context("Reading the LFS locks")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    parse_locks(&done.stdout)
}

fn parse_locks(json: &[u8]) -> Result<Vec<LfsLock>, GitError> {
    let read: Option<Vec<LockEntry>> = serde_json::from_slice(json)
        .map_err(|error| GitError::unexpected("lfs locks", error.to_string()))?;
    Ok(read
        .unwrap_or_default()
        .into_iter()
        .map(|entry| LfsLock {
            id: entry.id,
            path: entry.path.into(),
            owner: entry.owner.map_or_default(|owner| owner.name),
            locked_at: entry.locked_at,
        })
        .collect())
}

/// Locks `path` on the remote's LFS server (`git lfs lock`), or unlocks it (`unlock`; `force`:
/// someone else's lock too).
///
/// # Errors
///
/// When git LFS fails: a server without locking, a lock someone else holds.
pub async fn lfs_lock(
    git: &Git,
    repository: &Repository,
    path: &[u8],
    lock: Option<bool>,
) -> Result<(), GitError> {
    let mut command = git
        .write("lfs")
        .current_dir(repository.work_tree())
        .prompt_context("Locking an LFS file");
    command = match lock {
        None => command.arg("lock"),
        Some(false) => command.arg("unlock"),
        Some(true) => command.args(["unlock", "--force"]),
    };
    command
        .arg("--")
        .arg(path.to_os_str_lossy())
        .output()
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read() {
        let version = LfsVersion::parse(b"git-lfs/3.4.1 (GitHub; linux amd64; go 1.22.2)\n");
        assert_eq!(
            version,
            Some(LfsVersion {
                major: 3,
                minor: 4,
                patch: 1
            })
        );
        assert_eq!(
            version.map(|version| version.to_string()).as_deref(),
            Some("3.4.1")
        );
        assert_eq!(LfsVersion::parse(b"git version 2.43.0"), None);
    }

    #[test]
    fn a_listing_without_json_is_read() {
        let listing = b"Listing tracked patterns\n    *.psd (.gitattributes)\n    \
                        assets/** (assets/.gitattributes [lockable])\n\
                        Listing excluded patterns\n    *.txt (.gitattributes)\n";
        let patterns = parse_listing(listing);
        assert_eq!(patterns.len(), 3);
        assert_eq!(patterns[0].pattern, "*.psd");
        assert_eq!(patterns[1].source, "assets/.gitattributes");
        assert!(patterns[1].lockable);
        assert!(!patterns[2].tracked);
    }

    #[test]
    fn locks_are_read() {
        let json = br#"[{"id":"7","path":"art/hero.psd","owner":{"name":"Ada"},"locked_at":"2026-10-18T05:00:00Z"}]"#;
        let locks = parse_locks(json).unwrap();
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].owner, "Ada");
        assert_eq!(locks[0].path, "art/hero.psd");
        assert_eq!(parse_locks(b"null").unwrap(), []);
    }
}
