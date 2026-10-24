//! Finding a repository, where its `HEAD` points, and what it is in the middle of.

use std::io;
use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice as _};

use crate::error::GitError;
use crate::git::Git;
use crate::object_id::{ObjectFormat, ObjectId};

/// A repository with a work tree, as git describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    work_tree: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    object_format: ObjectFormat,
}

/// Why a path couldn't be opened as a repository.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoverError {
    /// Neither the path nor any directory above it is in a repository.
    #[error("{} isn't in a git repository", path.display())]
    NotARepository {
        /// The path asked about.
        path: PathBuf,
    },

    /// Git refuses to open a repository another user owns until it is listed in
    /// `safe.directory` (see [`trust_directory`]).
    #[error("git won't open {} because another user owns it", path.display())]
    DubiousOwnership {
        /// The repository, as git names it (the value `safe.directory` needs).
        path: PathBuf,
    },

    /// The repository has no work tree.
    #[error("{} is a bare repository, which egit can't open yet", git_dir.display())]
    Bare {
        /// The repository.
        git_dir: PathBuf,
    },

    /// The path is inside a repository's git directory rather than its work tree.
    #[error("{} is inside a .git directory: open the folder that contains it", path.display())]
    InsideGitDir {
        /// The path asked about.
        path: PathBuf,
    },

    /// Running git failed otherwise.
    #[error(transparent)]
    Git(#[from] GitError),
}

impl Repository {
    /// The repository containing `path` (a directory, or a file in one).
    ///
    /// # Errors
    ///
    /// A [`DiscoverError`] saying why there is no repository egit can open there.
    pub fn discover(git: &Git, path: &Path) -> Result<Self, DiscoverError> {
        let start = if git.files().is_file(path) {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let probe = git
            .read("rev-parse")
            .args([
                "--path-format=absolute",
                "--git-dir",
                "--git-common-dir",
                "--is-bare-repository",
                "--is-inside-git-dir",
                "--show-object-format",
            ])
            .current_dir(start)
            .output()
            .map_err(|error| classify(error, start))?;
        let mut lines = probe.stdout.lines();
        let mut next = || {
            lines
                .next()
                .ok_or_else(|| GitError::unexpected("rev-parse", "too few lines"))
        };
        let git_dir = path_from(next()?);
        let common_dir = path_from(next()?);
        let bare = flag(next()?)?;
        let inside_git_dir = flag(next()?)?;
        let format = next()?;
        let object_format = ObjectFormat::from_name(format).ok_or_else(|| {
            GitError::unexpected(
                "rev-parse",
                format!("unknown object format {}", BStr::new(format)),
            )
        })?;
        if bare {
            return Err(DiscoverError::Bare { git_dir });
        }
        if inside_git_dir {
            return Err(DiscoverError::InsideGitDir {
                path: path.to_owned(),
            });
        }

        let top = git
            .read("rev-parse")
            .args(["--path-format=absolute", "--show-toplevel"])
            .current_dir(start)
            .output()?;
        let work_tree = top
            .stdout
            .lines()
            .next()
            .map(path_from)
            .ok_or_else(|| GitError::unexpected("rev-parse", "no top level"))?;
        Ok(Self {
            work_tree,
            git_dir,
            common_dir,
            object_format,
        })
    }

    /// The top directory of the work tree.
    #[must_use]
    pub fn work_tree(&self) -> &Path {
        &self.work_tree
    }

    /// This work tree's git directory (`.git`, or a linked worktree's own).
    #[must_use]
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// The directory every work tree of the repository shares (objects, refs, configuration).
    #[must_use]
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// The hash function the repository names objects with.
    #[must_use]
    pub const fn object_format(&self) -> ObjectFormat {
        self.object_format
    }

    /// Whether git keeps a commit-graph file for the repository, which lets a walk of its history
    /// in topological order start at once (see [`write_commit_graph`](crate::write_commit_graph));
    /// not when its files can't be looked at.
    #[must_use]
    pub fn has_commit_graph(&self, git: &Git) -> bool {
        let info = self.common_dir.join("objects/info");
        let (single, chain) = (
            info.join("commit-graph"),
            info.join("commit-graphs/commit-graph-chain"),
        );
        git.files()
            .exist(&[&single, &chain])
            .is_ok_and(|there| there.contains(&true))
    }

    /// Whether this is a linked work tree (`git worktree add`) rather than the main one.
    #[must_use]
    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }

    /// Where `HEAD` points: a branch (with its newest commit and upstream), or a commit.
    ///
    /// # Errors
    ///
    /// When git fails or prints something unexpected.
    pub fn head(&self, git: &Git) -> Result<Head, GitError> {
        let symbolic = git
            .read("symbolic-ref")
            .args(["--quiet", "HEAD"])
            .current_dir(&self.work_tree)
            .output_unchecked()?;
        match symbolic.status.code() {
            Some(0) => {
                let reference = BString::from(symbolic.stdout.trim_ascii_end());
                self.branch_head(git, reference).map(Head::Branch)
            }
            // Not a symbolic ref: `HEAD` is detached.
            Some(1) => self.detached_head(git).map(Head::Detached),
            _ => Err(GitError::failed(
                "symbolic-ref",
                symbolic.status,
                symbolic.stderr,
            )),
        }
    }

    /// Whether `HEAD`'s history holds `commit` (`merge-base --is-ancestor`): a commit of the
    /// branch checked out, which rewriting it can reach. `false` before the first commit.
    ///
    /// # Errors
    ///
    /// When git fails otherwise.
    pub fn head_reaches(&self, git: &Git, commit: ObjectId) -> Result<bool, GitError> {
        let reached = git
            .read("merge-base")
            .args(["--is-ancestor", &commit.to_string(), "HEAD"])
            .current_dir(&self.work_tree)
            .output_unchecked()?;
        match reached.status.code() {
            Some(0) => Ok(true),
            // Not an ancestor; or (128) no `HEAD` yet.
            Some(1 | 128) => Ok(false),
            _ => Err(GitError::failed(
                "merge-base",
                reached.status,
                reached.stderr,
            )),
        }
    }

    /// Where `HEAD` is, and nothing else [`Self::head`] works out (the upstream, how far
    /// ahead): two quick reads, for checking it before and after something moves it.
    ///
    /// # Errors
    ///
    /// When git fails or prints something unexpected.
    pub fn head_ref(&self, git: &Git) -> Result<HeadRef, GitError> {
        let symbolic = git
            .read("symbolic-ref")
            .args(["--quiet", "HEAD"])
            .current_dir(&self.work_tree)
            .output_unchecked()?;
        let reference = match symbolic.status.code() {
            Some(0) => Some(BString::from(symbolic.stdout.trim_ascii_end())),
            Some(1) => None,
            _ => {
                return Err(GitError::failed(
                    "symbolic-ref",
                    symbolic.status,
                    symbolic.stderr,
                ));
            }
        };
        let resolved = git
            .read("rev-parse")
            .args(["--verify", "--quiet", "HEAD"])
            .current_dir(&self.work_tree)
            .output_unchecked()?;
        let commit = if resolved.status.success() {
            Some(
                ObjectId::from_hex(resolved.stdout.trim_ascii())
                    .ok_or_else(|| GitError::unexpected("rev-parse", "a commit's name"))?,
            )
        } else {
            // A branch without commits yet.
            None
        };
        if reference.is_none() && commit.is_none() {
            return Err(GitError::unexpected("rev-parse", "HEAD's commit"));
        }
        Ok(HeadRef { reference, commit })
    }

    /// The current branch's upstream: its full name (`refs/remotes/origin/main`) and commit;
    /// none when detached, without an upstream, or when it is gone (`rev-parse
    /// --symbolic-full-name @{upstream}`, then its commit).
    ///
    /// # Errors
    ///
    /// When git can't be run, or prints something unexpected.
    pub fn upstream_tip(&self, git: &Git) -> Result<Option<(BString, ObjectId)>, GitError> {
        let named = git
            .read("rev-parse")
            .args(["--verify", "--quiet", "--symbolic-full-name", "@{upstream}"])
            .current_dir(&self.work_tree)
            .output_unchecked()?;
        let name = named.stdout.trim_ascii();
        if !named.status.success() || !name.starts_with(b"refs/") {
            return Ok(None);
        }
        let resolved = git
            .read("rev-parse")
            .args(["--verify", "--quiet", "--end-of-options"])
            .arg(format!("{}^{{commit}}", name.to_str_lossy()))
            .current_dir(&self.work_tree)
            .output_unchecked()?;
        if !resolved.status.success() {
            return Ok(None);
        }
        let commit = ObjectId::from_hex(resolved.stdout.trim_ascii())
            .ok_or_else(|| GitError::unexpected("rev-parse", "a commit's name"))?;
        Ok(Some((BString::from(name), commit)))
    }

    fn branch_head(&self, git: &Git, reference: BString) -> Result<BranchHead, GitError> {
        let name = BString::from(
            reference
                .strip_prefix(b"refs/heads/")
                .unwrap_or_else(|| reference.as_bytes()),
        );
        let listed = git
            .read("for-each-ref")
            .arg(
                "--format=%(refname)%00%(objectname)%00%(upstream:short)%00\
                 %(upstream:track,nobracket)%00%(contents:subject)",
            )
            .arg(reference.to_os_str_lossy())
            .current_dir(&self.work_tree)
            .output()?;
        // The pattern also matches refs below the branch's name; only an exact match counts.
        let record = listed.stdout.lines().find(|line| {
            line.split_str(b"\0")
                .next()
                .is_some_and(|name| name == reference.as_bytes())
        });
        let Some(record) = record else {
            // No commits on the branch yet.
            return Ok(BranchHead {
                name,
                reference,
                commit: None,
                upstream: None,
            });
        };
        let mut fields = record.splitn_str(5, b"\0").skip(1);
        let mut next = || {
            fields
                .next()
                .ok_or_else(|| GitError::unexpected("for-each-ref", "too few fields"))
        };
        let id = parse_id("for-each-ref", next()?)?;
        let upstream_name = next()?;
        let tracking = next()?;
        let subject = next()?;
        let upstream = if upstream_name.is_empty() {
            None
        } else {
            Some(Upstream {
                name: BString::from(upstream_name),
                tracking: Tracking::parse(tracking).ok_or_else(|| {
                    GitError::unexpected(
                        "for-each-ref",
                        format!("unknown tracking state {}", BStr::new(tracking)),
                    )
                })?,
            })
        };
        Ok(BranchHead {
            name,
            reference,
            commit: Some(CommitSummary {
                id,
                subject: BString::from(subject),
            }),
            upstream,
        })
    }

    fn detached_head(&self, git: &Git) -> Result<CommitSummary, GitError> {
        let shown = git
            .read("log")
            .config("log.showSignature", "false")
            .config("i18n.logOutputEncoding", "UTF-8")
            .args(["--max-count=1", "--format=%H%x00%s", "HEAD", "--"])
            .current_dir(&self.work_tree)
            .output()?;
        let line = shown.stdout.strip_suffix(b"\n").unwrap_or(&shown.stdout);
        let (id, subject) = line
            .split_once_str(b"\0")
            .ok_or_else(|| GitError::unexpected("log", "no commit"))?;
        Ok(CommitSummary {
            id: parse_id("log", id)?,
            subject: BString::from(subject),
        })
    }

    /// The operations in progress in this work tree (a merge stopped on conflicts, a rebase…),
    /// read from the state files git documents for them.
    ///
    /// # Errors
    ///
    /// When the git directory can't be read.
    pub fn operations(&self, git: &Git) -> io::Result<Vec<Operation>> {
        let dir = &self.git_dir;
        let names = [
            "rebase-merge",
            "rebase-apply",
            "rebase-apply/applying",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
        ];
        let paths = names.map(|name| dir.join(name));
        let there = git
            .files()
            .exist(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
        let has = |name: &str| {
            names
                .iter()
                .position(|known| *known == name)
                .and_then(|index| there.get(index))
                .copied()
                .unwrap_or(false)
        };
        let mut operations = Vec::new();
        if has("rebase-merge") {
            operations.push(Operation::Rebase);
        }
        if has("rebase-apply") {
            operations.push(if has("rebase-apply/applying") {
                Operation::ApplyMailbox
            } else {
                Operation::Rebase
            });
        }
        for (file, operation) in [
            ("MERGE_HEAD", Operation::Merge),
            ("CHERRY_PICK_HEAD", Operation::CherryPick),
            ("REVERT_HEAD", Operation::Revert),
            ("BISECT_LOG", Operation::Bisect),
        ] {
            if has(file) {
                operations.push(operation);
            }
        }
        Ok(operations)
    }
}

/// Adds `path` to the user's `safe.directory` list, so git opens it although another user
/// owns it. Pass the path [`DiscoverError::DubiousOwnership`] names.
///
/// # Errors
///
/// When git can't write the user's configuration.
pub fn trust_directory(git: &Git, path: &Path) -> Result<(), GitError> {
    git.write("config")
        .args(["--global", "--add", "safe.directory"])
        .arg(path)
        .current_dir(crate::git::outside_repositories())
        .output()
        .map(drop)
}

/// Makes `folder` (one that exists, on the computer `git` runs on) a new repository with no
/// commits (`git init`), its branch the one `init.defaultBranch` names. Pass the path
/// [`DiscoverError::NotARepository`] names.
///
/// # Errors
///
/// When `folder` isn't a folder, or git can't write `.git` in it.
pub fn init_repository(git: &Git, folder: &Path) -> Result<(), GitError> {
    git.write("init")
        .arg("--quiet")
        .current_dir(folder)
        .output()
        .map(drop)
}

/// Where `HEAD` points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// On a branch.
    Branch(BranchHead),
    /// Detached, at a commit.
    Detached(CommitSummary),
}

/// Where `HEAD` is, briefly ([`Repository::head_ref`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadRef {
    /// The branch's full name (`refs/heads/main`); `None` when detached.
    pub reference: Option<BString>,
    /// Its commit; `None` until the branch's first.
    pub commit: Option<ObjectId>,
}

impl HeadRef {
    /// The branch's short name (`main`), when on one.
    #[must_use]
    pub fn branch(&self) -> Option<&BStr> {
        self.reference.as_ref().map(|reference| {
            reference
                .strip_prefix(b"refs/heads/")
                .unwrap_or(reference.as_slice())
                .as_bstr()
        })
    }
}

/// The branch `HEAD` is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchHead {
    /// The branch's name (`main`).
    pub name: BString,
    /// Its full reference (`refs/heads/main`).
    pub reference: BString,
    /// Its newest commit; `None` until the first commit.
    pub commit: Option<CommitSummary>,
    /// The branch it tracks, if any.
    pub upstream: Option<Upstream>,
}

/// A commit, briefly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSummary {
    /// Its name.
    pub id: ObjectId,
    /// The first line of its message.
    pub subject: BString,
}

/// The branch a local branch tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// Its short name (`origin/main`).
    pub name: BString,
    /// How the two compare.
    pub tracking: Tracking,
}

/// How a branch compares with its upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracking {
    /// The upstream is configured but no longer exists (deleted on the remote, then pruned).
    Gone,
    /// Commits on each side the other doesn't have.
    Diverged {
        /// Commits on the branch that aren't upstream.
        ahead: u32,
        /// Commits upstream that aren't on the branch.
        behind: u32,
    },
}

impl Tracking {
    /// Reads `%(upstream:track,nobracket)`: empty, `gone`, `ahead 1`, `behind 2`, or
    /// `ahead 1, behind 2`.
    fn parse(text: &[u8]) -> Option<Self> {
        if text == b"gone" {
            return Some(Self::Gone);
        }
        let (mut ahead, mut behind) = (0, 0);
        for part in text.split_str(b", ").filter(|part| !part.is_empty()) {
            let (word, count) = part.split_once_str(b" ")?;
            let count: u32 = count.to_str().ok()?.parse().ok()?;
            match word {
                b"ahead" => ahead = count,
                b"behind" => behind = count,
                _ => return None,
            }
        }
        Some(Self::Diverged { ahead, behind })
    }
}

/// An operation git stopped in the middle of, waiting for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// `git merge` (or a pull that merges).
    Merge,
    /// `git rebase` (or a pull that rebases).
    Rebase,
    /// `git am`.
    ApplyMailbox,
    /// `git cherry-pick`.
    CherryPick,
    /// `git revert`.
    Revert,
    /// `git bisect`.
    Bisect,
}

/// Turns a failed discovery into the reason a person can act on.
fn classify(error: GitError, start: &Path) -> DiscoverError {
    const DUBIOUS: &[u8] = b"detected dubious ownership in repository at '";
    let Some(failure) = error.failure() else {
        return DiscoverError::Git(error);
    };
    let stderr = failure.stderr.as_bytes();
    if let Some(at) = stderr.find(DUBIOUS) {
        let rest = stderr
            .get(at.saturating_add(DUBIOUS.len())..)
            .unwrap_or_default();
        let line = rest.lines().next().unwrap_or_default();
        let path = line.strip_suffix(b"'").unwrap_or(line);
        return DiscoverError::DubiousOwnership {
            path: path_from(path),
        };
    }
    if stderr.find(b"not a git repository").is_some() {
        return DiscoverError::NotARepository {
            path: start.to_owned(),
        };
    }
    DiscoverError::Git(error)
}

fn path_from(bytes: &[u8]) -> PathBuf {
    bytes.to_path_lossy().into_owned()
}

fn flag(line: &[u8]) -> Result<bool, GitError> {
    match line {
        b"true" => Ok(true),
        b"false" => Ok(false),
        _ => Err(GitError::unexpected(
            "rev-parse",
            format!("{} isn't true or false", BStr::new(line)),
        )),
    }
}

fn parse_id(subcommand: &'static str, hex: &[u8]) -> Result<ObjectId, GitError> {
    ObjectId::from_hex(hex).ok_or_else(|| {
        GitError::unexpected(
            subcommand,
            format!("{} isn't an object name", BStr::new(hex)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tracking_states() {
        assert_eq!(
            Tracking::parse(b""),
            Some(Tracking::Diverged {
                ahead: 0,
                behind: 0
            })
        );
        assert_eq!(Tracking::parse(b"gone"), Some(Tracking::Gone));
        assert_eq!(
            Tracking::parse(b"ahead 3"),
            Some(Tracking::Diverged {
                ahead: 3,
                behind: 0
            })
        );
        assert_eq!(
            Tracking::parse(b"behind 2"),
            Some(Tracking::Diverged {
                ahead: 0,
                behind: 2
            })
        );
        assert_eq!(
            Tracking::parse(b"ahead 1, behind 20"),
            Some(Tracking::Diverged {
                ahead: 1,
                behind: 20
            })
        );
    }

    #[test]
    fn rejects_unknown_tracking_states() {
        assert_eq!(Tracking::parse(b"sideways 1"), None);
        assert_eq!(Tracking::parse(b"ahead lots"), None);
        assert_eq!(Tracking::parse(b"ahead"), None);
    }
}
