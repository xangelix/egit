//! The history's shape: every commit reachable from a ref, children before parents, and the refs
//! that point into it.

use std::ops::ControlFlow;

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::config::{ConfigPlace, ConfigSnapshot};
use crate::error::GitError;
use crate::git::Git;
use crate::lines::Lines;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// How a walk of the history ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walked {
    /// How many commits were handed over.
    pub commits: u64,
    /// Whether the consumer stopped the walk early.
    pub stopped: bool,
}

/// The most tips handed to one `git merge-base --independent` (their names stay far below the
/// system's limit on a command's arguments). More are walked as they are.
const MAX_REDUCED_TIPS: usize = 4096;

/// Walks every commit reachable from a ref or `HEAD` (any worktree's), children before parents.
///
/// Stashes are left out. The order is topological: every commit before its parents, and a
/// branch's commits together. `each` gets a commit and its parents' names (first parent first)
/// and may stop the walk.
///
/// With a commit-graph file ([`Repository::has_commit_graph`]) commits arrive at once; without
/// one, git must read the whole history before the first. Git's topological walk first reads
/// down to the oldest commit it starts from, so it starts only from the tips no other tip
/// reaches: an old tag on a long branch then costs nothing (Linux's 950 tips reduce to 4, and
/// its first commit arrives in 70 ms rather than 700).
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't a commit and its parents.
pub fn walk_history(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
    mut each: impl FnMut(ObjectId, &[ObjectId]) -> ControlFlow<()>,
) -> Result<Walked, GitError> {
    let mut lines = Lines::default();
    let mut parents = Vec::with_capacity(8);
    let mut commits = 0_u64;
    let mut malformed = None;
    let mut stopped = false;
    let mut handle = |line: &[u8]| -> ControlFlow<()> {
        let mut names = line.split_str(b" ").filter(|name| !name.is_empty());
        let Some(commit) = names.next().and_then(ObjectId::from_hex) else {
            malformed = Some(BString::from(line));
            return ControlFlow::Break(());
        };
        parents.clear();
        for name in names {
            let Some(parent) = ObjectId::from_hex(name) else {
                malformed = Some(BString::from(line));
                return ControlFlow::Break(());
            };
            parents.push(parent);
        }
        commits = commits.saturating_add(1);
        let flow = each(commit, &parents);
        if flow.is_break() {
            stopped = true;
        }
        flow
    };
    let walk = git
        .read("rev-list")
        .args(["--topo-order", "--parents"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    let walk = match independent_tips(git, repository, cancel)? {
        Some(tips) if tips.is_empty() => {
            return Ok(Walked {
                commits: 0,
                stopped: false,
            });
        }
        Some(tips) => walk.arg("--stdin").stdin(tips),
        None => walk.args(["--exclude=refs/stash", "--all"]),
    };
    walk.stream(|chunk| lines.feed(chunk, &mut handle))?;
    if let Some(line) = malformed {
        return Err(GitError::unexpected(
            "rev-list",
            format!("{line} isn't a commit and its parents"),
        ));
    }
    Ok(Walked { commits, stopped })
}

/// The commits `--exclude=refs/stash --all` starts from, less those another of them reaches, one
/// name a line; `None` when walking from `--all` costs no more (without a commit-graph, or with
/// more tips than [`MAX_REDUCED_TIPS`]).
fn independent_tips(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Option<Vec<u8>>, GitError> {
    if !repository.has_commit_graph() {
        return Ok(None);
    }
    let listed = git
        .read("rev-list")
        .args(["--no-walk=unsorted", "--exclude=refs/stash", "--all"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?
        .stdout;
    let tips: Vec<&[u8]> = listed.lines().filter(|line| !line.is_empty()).collect();
    if tips.len() > MAX_REDUCED_TIPS {
        return Ok(None);
    }
    if tips.len() < 2 {
        return Ok(Some(listed));
    }
    git.read("merge-base")
        .arg("--independent")
        .args(tips.iter().map(|tip| tip.to_os_str_lossy()))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(|reduced| Some(reduced.stdout))
}

/// What a ref names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    /// A local branch (`refs/heads/`).
    Branch,
    /// A remote-tracking branch (`refs/remotes/`).
    RemoteBranch,
    /// A tag (`refs/tags/`).
    Tag,
    /// Anything else (`refs/stash`, notes, …).
    Other,
}

/// A ref and the commit it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// Its full name (`refs/heads/main`).
    pub name: BString,
    /// What it names.
    pub kind: RefKind,
    /// The object it points at, through any annotated tags.
    pub target: ObjectId,
}

impl Reference {
    /// Its name without the namespace (`main`, `origin/main`, `v1.0`).
    #[must_use]
    pub fn short_name(&self) -> &BStr {
        let name = self.name.as_slice();
        let short = ["refs/heads/", "refs/remotes/", "refs/tags/"]
            .iter()
            .find_map(|prefix| name.strip_prefix(prefix.as_bytes()))
            .unwrap_or(name);
        BStr::new(short)
    }
}

/// Every ref but symbolic ones (such as `refs/remotes/origin/HEAD`), with annotated tags
/// followed through to what they finally tag (a tag of a tag too).
///
/// # Errors
///
/// When git fails or prints something unexpected.
pub fn list_references(git: &Git, repository: &Repository) -> Result<Vec<Reference>, GitError> {
    let listed = git
        .read("for-each-ref")
        .arg("--format=%(objectname)%00%(*objectname)%00%(*objecttype)%00%(symref)%00%(refname)")
        .current_dir(repository.work_tree())
        .output()?;
    let mut references = Vec::new();
    // Tags of tags: git 2.43 (the floor) peels only the first in `%(*objectname)`; later git,
    // since its commit 188782ec, all of them.
    let mut nested = Vec::new();
    for line in listed.stdout.lines() {
        let mut fields = line.split_str(b"\0");
        let (Some(object), Some(peeled), Some(peeled_type), Some(symref), Some(name)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(GitError::unexpected("for-each-ref", "too few fields"));
        };
        if !symref.is_empty() {
            continue;
        }
        let target = if peeled.is_empty() { object } else { peeled };
        let target = ObjectId::from_hex(target).ok_or_else(|| {
            GitError::unexpected(
                "for-each-ref",
                format!("{} isn't an object name", BStr::new(target)),
            )
        })?;
        if peeled_type == b"tag" {
            nested.push(references.len());
        }
        let kind = if name.starts_with(b"refs/heads/") {
            RefKind::Branch
        } else if name.starts_with(b"refs/remotes/") {
            RefKind::RemoteBranch
        } else if name.starts_with(b"refs/tags/") {
            RefKind::Tag
        } else {
            RefKind::Other
        };
        references.push(Reference {
            name: BString::from(name),
            kind,
            target,
        });
    }
    peel_nested(git, repository, &mut references, &nested)?;
    Ok(references)
}

/// Peels the references at `nested` (tags of tags) through every tag, as later git peels
/// `%(*objectname)` itself: one `rev-parse` of `<name>^{}` for all of them.
fn peel_nested(
    git: &Git,
    repository: &Repository,
    references: &mut [Reference],
    nested: &[usize],
) -> Result<(), GitError> {
    if nested.is_empty() {
        return Ok(());
    }
    let names: Vec<std::ffi::OsString> = nested
        .iter()
        .filter_map(|&at| references.get(at))
        .map(|reference| {
            let mut name = reference.name.to_os_str_lossy().into_owned();
            name.push("^{}");
            name
        })
        .collect();
    // Each name is a ref's, so `refs/…`: never an option (and git 2.43's `rev-parse` would
    // echo `--end-of-options` back).
    let peeled = git
        .read("rev-parse")
        .args(&names)
        .current_dir(repository.work_tree())
        .output()?;
    for (&at, line) in nested.iter().zip(peeled.stdout.lines()) {
        let target = ObjectId::from_hex(line)
            .ok_or_else(|| GitError::unexpected("rev-parse", "an object name"))?;
        if let Some(reference) = references.get_mut(at) {
            reference.target = target;
        }
    }
    Ok(())
}

/// How a commit changed a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// Added.
    Added,
    /// Copied from another file (with how similar the two are, in percent).
    Copied {
        /// The file it was copied from.
        from: BString,
        /// How similar the copy is, in percent.
        similarity: u8,
    },
    /// Deleted.
    Deleted,
    /// Changed.
    Modified,
    /// Renamed, possibly with changes.
    Renamed {
        /// Its name before.
        from: BString,
        /// How similar it stayed, in percent.
        similarity: u8,
    },
    /// Changed type (a file became a symlink, say).
    TypeChanged,
    /// Unmerged.
    Unmerged,
    /// Something git didn't name.
    Unknown,
}

/// A file a commit changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The file's path (after a rename, its new one).
    pub path: BString,
    /// How it changed.
    pub kind: ChangeKind,
    /// What the tree held before (`None` when the file was added).
    pub old: Option<TreeEntry>,
    /// What the tree holds after (`None` when the file was deleted).
    pub new: Option<TreeEntry>,
}

/// What a tree holds at a path: a mode and an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    /// The mode, as git records it (`0o100644` a file, `0o100755` an executable, `0o120000` a
    /// symbolic link, `0o160000` a submodule).
    pub mode: u32,
    /// The object: a blob, or for a submodule, its commit (not in this repository).
    pub id: ObjectId,
}

impl TreeEntry {
    /// A submodule's mode: its object is a commit of another repository.
    pub const SUBMODULE: u32 = 0o160_000;

    /// A symbolic link's mode: its blob is the link's target.
    pub const SYMLINK: u32 = 0o120_000;
}

/// The files `commit` changed: against `first_parent`, or everything it holds when it has none
/// (a root commit). Renames are detected.
///
/// # Errors
///
/// When git fails or prints something unexpected.
pub fn commit_changes(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    first_parent: Option<ObjectId>,
) -> Result<Vec<FileChange>, GitError> {
    let mut command = git
        .read("diff-tree")
        .args([
            "-r",
            "-z",
            "--no-commit-id",
            "--raw",
            "--no-abbrev",
            "-M",
            "--no-ext-diff",
        ])
        .current_dir(repository.work_tree());
    command = match first_parent {
        Some(parent) => command.arg(parent.to_string()).arg(commit.to_string()),
        None => command.arg("--root").arg(commit.to_string()),
    };
    let listed = command.output()?;
    parse_raw(&listed.stdout)
}

/// Parses `--raw -z --no-abbrev`: `:old-mode new-mode old-id new-id status`, then one path (or
/// two, for renames and copies), each ended by a NUL. A side's mode is zero when it is absent.
fn parse_raw(output: &[u8]) -> Result<Vec<FileChange>, GitError> {
    let malformed = || GitError::unexpected("diff-tree", "an incomplete change");
    let mut fields = output.split_str(b"\0").filter(|field| !field.is_empty());
    let mut changes = Vec::new();
    while let Some(head) = fields.next() {
        let head = head.strip_prefix(b":").ok_or_else(malformed)?;
        let mut parts = head.split_str(b" ");
        let mut part = || parts.next().ok_or_else(malformed);
        let (old_mode, new_mode, old_id, new_id, status) =
            (part()?, part()?, part()?, part()?, part()?);
        let side = |mode: &[u8], id: &[u8]| -> Result<Option<TreeEntry>, GitError> {
            let mode = mode
                .to_str()
                .ok()
                .and_then(|mode| u32::from_str_radix(mode, 8).ok())
                .ok_or_else(malformed)?;
            let id = ObjectId::from_hex(id).ok_or_else(malformed)?;
            Ok((mode != 0).then_some(TreeEntry { mode, id }))
        };
        let (old, new) = (side(old_mode, old_id)?, side(new_mode, new_id)?);
        let (&letter, score) = status.split_first().ok_or_else(malformed)?;
        let similarity = || {
            score
                .to_str()
                .ok()
                .and_then(|score| score.parse().ok())
                .unwrap_or(0)
        };
        let path = BString::from(fields.next().ok_or_else(malformed)?);
        let (path, kind) = match letter {
            b'A' => (path, ChangeKind::Added),
            b'D' => (path, ChangeKind::Deleted),
            b'M' => (path, ChangeKind::Modified),
            b'T' => (path, ChangeKind::TypeChanged),
            b'U' => (path, ChangeKind::Unmerged),
            b'R' | b'C' => {
                let to = BString::from(fields.next().ok_or_else(malformed)?);
                let similarity = similarity();
                let kind = if letter == b'R' {
                    ChangeKind::Renamed {
                        from: path,
                        similarity,
                    }
                } else {
                    ChangeKind::Copied {
                        from: path,
                        similarity,
                    }
                };
                (to, kind)
            }
            _ => (path, ChangeKind::Unknown),
        };
        changes.push(FileChange {
            path,
            kind,
            old,
            new,
        });
    }
    Ok(changes)
}

/// How many lines a commit added to and removed from a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineCounts {
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// A file as [`line_counts`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountedFile {
    /// The file's path (after a rename, its new one).
    pub path: BString,
    /// Its lines added and removed; `None` for a binary file.
    pub lines: Option<LineCounts>,
}

/// How many lines `commit` added to and removed from each file it changed, against
/// `first_parent` (or nothing: a root commit), in the order [`commit_changes`] lists them.
///
/// Every file is diffed, so a large commit takes a while: this is meant to follow the list,
/// not to hold it up. Git never asks for credentials. In a partial clone
/// ([`ConfigSnapshot::is_partial_clone`]), a git that honours `GIT_NO_LAZY_FETCH` (2.44, and
/// 2.43.4's fix) fails rather than fetch the blobs it lacks, but an older one fetches them:
/// don't count lines there.
///
/// # Errors
///
/// When git fails, is cancelled, or prints something unexpected.
pub fn line_counts(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    first_parent: Option<ObjectId>,
    cancel: &CancelToken,
) -> Result<Vec<CountedFile>, GitError> {
    let mut command = git
        .read("diff-tree")
        .args([
            "-r",
            "-z",
            "--no-commit-id",
            "--numstat",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
        ])
        .env("GIT_NO_LAZY_FETCH", "1")
        .non_interactive()
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    command = match first_parent {
        Some(parent) => command.arg(parent.to_string()).arg(commit.to_string()),
        None => command.arg("--root").arg(commit.to_string()),
    };
    let listed = command.output()?;
    parse_numstat(&listed.stdout)
}

/// Parses `--numstat -z`: `added TAB removed TAB path NUL`, or for a rename or copy,
/// `added TAB removed TAB NUL old NUL new NUL`; a binary file's counts are `-`.
fn parse_numstat(output: &[u8]) -> Result<Vec<CountedFile>, GitError> {
    let malformed = || GitError::unexpected("diff-tree", "an incomplete line count");
    let mut fields = output.split_str(b"\0");
    let mut files = Vec::new();
    while let Some(record) = fields.next().filter(|record| !record.is_empty()) {
        let mut parts = record.splitn_str(3, b"\t");
        let (added, removed, path) = (
            parts.next().ok_or_else(malformed)?,
            parts.next().ok_or_else(malformed)?,
            parts.next().ok_or_else(malformed)?,
        );
        let path = if path.is_empty() {
            // A rename or copy: its old path, then its new one (neither ever empty).
            let mut next = || fields.next().filter(|path| !path.is_empty());
            next().and_then(|_| next()).ok_or_else(malformed)?
        } else {
            path
        };
        let count = |field: &[u8]| field.to_str().ok().and_then(|field| field.parse().ok());
        let lines = match (added, removed) {
            (b"-", b"-") => None,
            _ => Some(LineCounts {
                added: count(added).ok_or_else(malformed)?,
                removed: count(removed).ok_or_else(malformed)?,
            }),
        };
        files.push(CountedFile {
            path: BString::from(path),
            lines,
        });
    }
    Ok(files)
}

/// Writes (or rewrites) the repository's commit-graph file (`git commit-graph write --reachable`).
///
/// It includes the changed-path filters that make a file's history fast (`--changed-paths`)
/// unless the repository is a partial clone, where computing them would fetch every tree and
/// blob it left out.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn write_commit_graph(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let config = ConfigSnapshot::read(git, ConfigPlace::Repository(repository.work_tree()))?;
    let mut command = git
        .write("commit-graph")
        .args(["write", "--reachable"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if !config.is_partial_clone() {
        command = command.arg("--changed-paths");
    }
    command.output().map(drop)
}

/// The commits `tip` has that `base` hasn't (both full ref names, or commit names), newest
/// first, at most `limit` of them: what a branch would bring into another.
///
/// # Errors
///
/// When git fails (a ref that doesn't exist) or is cancelled.
pub fn commits_between(
    git: &Git,
    repository: &Repository,
    base: &BStr,
    tip: &BStr,
    limit: usize,
    cancel: &CancelToken,
) -> Result<Vec<ObjectId>, GitError> {
    let mut not_base = BString::from("^");
    not_base.extend_from_slice(base);
    let listed = git
        .read("rev-list")
        .arg(format!("--max-count={limit}"))
        .arg("--end-of-options")
        .arg(tip.to_os_str_lossy())
        .arg(not_base.to_os_str_lossy())
        .arg("--")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    listed
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            ObjectId::from_hex(line)
                .ok_or_else(|| GitError::unexpected("rev-list", "a commit name"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
    const B: &str = "2cf8d83d9ee29543b34a87727421fdecb7e3f3a1";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn entry(mode: u32, hex: &str) -> TreeEntry {
        TreeEntry {
            mode,
            id: ObjectId::from_hex(hex.as_bytes()).unwrap(),
        }
    }

    #[test]
    fn reads_raw_changes() {
        let output = format!(
            ":100644 100644 {A} {B} M\0src/lib.rs\0\
             :000000 100755 {ZERO} {B} A\0new\0\
             :100644 000000 {A} {ZERO} D\0gone\0\
             :100644 100644 {A} {B} R087\0old name\0new name\0\
             :100644 100644 {A} {A} C100\0a\0b\0\
             :100644 120000 {A} {B} T\0link\0"
        );
        let changes = parse_raw(output.as_bytes()).unwrap();
        assert_eq!(changes.len(), 6);
        assert_eq!(
            changes[0],
            FileChange {
                path: "src/lib.rs".into(),
                kind: ChangeKind::Modified,
                old: Some(entry(0o100_644, A)),
                new: Some(entry(0o100_644, B)),
            }
        );
        assert_eq!(changes[1].kind, ChangeKind::Added);
        assert_eq!(
            (changes[1].old, changes[1].new),
            (None, Some(entry(0o100_755, B)))
        );
        assert_eq!(changes[2].kind, ChangeKind::Deleted);
        assert_eq!(changes[2].new, None);
        assert_eq!(
            (changes[3].path.to_string(), &changes[3].kind),
            (
                "new name".to_owned(),
                &ChangeKind::Renamed {
                    from: "old name".into(),
                    similarity: 87
                }
            )
        );
        assert_eq!(
            changes[4].kind,
            ChangeKind::Copied {
                from: "a".into(),
                similarity: 100
            }
        );
        assert_eq!(changes[5].kind, ChangeKind::TypeChanged);
        assert_eq!(changes[5].new.unwrap().mode, TreeEntry::SYMLINK);
    }

    #[test]
    fn refuses_what_isnt_a_raw_change() {
        assert!(parse_raw(format!(":100644 100644 {A} {B} M\0").as_bytes()).is_err());
        assert!(parse_raw(format!(":100644 100644 {A} {B} R100\0only-one\0").as_bytes()).is_err());
        assert!(parse_raw(b"M\0path\0").is_err());
        assert!(parse_raw(format!(":10064x 100644 {A} {B} M\0p\0").as_bytes()).is_err());
        assert!(parse_raw(format!(":100644 100644 {A} short M\0p\0").as_bytes()).is_err());
    }

    #[test]
    fn reads_line_counts() {
        let output = b"3\t1\tsrc/lib.rs\0\
                       -\t-\tlogo.png\0\
                       0\t0\t\0old name\0new name\0\
                       2\t0\ta\ttab\0";
        let counted = |path: &str, lines: Option<(u32, u32)>| CountedFile {
            path: path.into(),
            lines: lines.map(|(added, removed)| LineCounts { added, removed }),
        };
        assert_eq!(
            parse_numstat(output).unwrap(),
            [
                counted("src/lib.rs", Some((3, 1))),
                counted("logo.png", None),
                counted("new name", Some((0, 0))),
                counted("a\ttab", Some((2, 0))),
            ]
        );
        assert_eq!(parse_numstat(b"").unwrap(), []);
    }

    #[test]
    fn refuses_what_isnt_a_line_count() {
        assert!(parse_numstat(b"3\tlib.rs\0").is_err());
        assert!(parse_numstat(b"x\t1\tlib.rs\0").is_err());
        assert!(parse_numstat(b"1\t1\t\0only-one\0").is_err());
    }

    #[test]
    fn short_names() {
        let reference = |name: &str| Reference {
            name: name.into(),
            kind: RefKind::Other,
            target: ObjectId::from_hex(b"ce013625030ba8dba906f756967f9e9ca394464a").unwrap(),
        };
        assert_eq!(reference("refs/heads/main").short_name(), "main");
        assert_eq!(
            reference("refs/remotes/origin/main").short_name(),
            "origin/main"
        );
        assert_eq!(reference("refs/tags/v1.0").short_name(), "v1.0");
        assert_eq!(reference("refs/stash").short_name(), "refs/stash");
    }
}
