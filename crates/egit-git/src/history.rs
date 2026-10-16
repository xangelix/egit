//! The history's shape: every commit reachable from a ref, children before parents, and the refs
//! that point into it.

use std::ops::ControlFlow;

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
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

/// Walks every commit reachable from a ref or `HEAD`, children before parents.
///
/// Stashes are left out. The order is topological: every commit before its parents, and a
/// branch's commits together. `each` gets a commit and its parents' names (first parent first)
/// and may stop the walk.
///
/// With a commit-graph file ([`Repository::has_commit_graph`]) commits arrive at once; without
/// one, git must read the whole history before the first.
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
    git.read("rev-list")
        .args(["--topo-order", "--parents", "--exclude=refs/stash", "--all"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .stream(|chunk| lines.feed(chunk, &mut handle))?;
    if let Some(line) = malformed {
        return Err(GitError::unexpected(
            "rev-list",
            format!("{line} isn't a commit and its parents"),
        ));
    }
    Ok(Walked { commits, stopped })
}

/// Splits a stream into lines across chunk boundaries, copying only a line split between two
/// chunks.
#[derive(Debug, Default)]
struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    fn feed(
        &mut self,
        mut chunk: &[u8],
        handle: &mut impl FnMut(&[u8]) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        if !self.partial.is_empty() {
            let Some(end) = chunk.find_byte(b'\n') else {
                self.partial.extend_from_slice(chunk);
                return ControlFlow::Continue(());
            };
            self.partial
                .extend_from_slice(chunk.get(..end).unwrap_or_default());
            let flow = handle(&self.partial);
            self.partial.clear();
            flow?;
            chunk = chunk.get(end.saturating_add(1)..).unwrap_or_default();
        }
        while let Some(end) = chunk.find_byte(b'\n') {
            handle(chunk.get(..end).unwrap_or_default())?;
            chunk = chunk.get(end.saturating_add(1)..).unwrap_or_default();
        }
        self.partial.extend_from_slice(chunk);
        ControlFlow::Continue(())
    }
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
/// followed to what they tag.
///
/// # Errors
///
/// When git fails or prints something unexpected.
pub fn list_references(git: &Git, repository: &Repository) -> Result<Vec<Reference>, GitError> {
    let listed = git
        .read("for-each-ref")
        .arg("--format=%(objectname)%00%(*objectname)%00%(symref)%00%(refname)")
        .current_dir(repository.work_tree())
        .output()?;
    let mut references = Vec::new();
    for line in listed.stdout.lines() {
        let mut fields = line.split_str(b"\0");
        let (Some(object), Some(peeled), Some(symref), Some(name)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
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
    Ok(references)
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
            "--name-status",
            "-M",
            "--no-ext-diff",
        ])
        .current_dir(repository.work_tree());
    command = match first_parent {
        Some(parent) => command.arg(parent.to_string()).arg(commit.to_string()),
        None => command.arg("--root").arg(commit.to_string()),
    };
    let listed = command.output()?;
    parse_name_status(&listed.stdout)
}

/// Parses `--name-status -z`: a status, then one path (or two, for renames and copies), each
/// ended by a NUL.
fn parse_name_status(output: &[u8]) -> Result<Vec<FileChange>, GitError> {
    let malformed = || GitError::unexpected("diff-tree", "an incomplete change");
    let mut fields = output.split_str(b"\0").filter(|field| !field.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
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
        changes.push(FileChange { path, kind });
    }
    Ok(changes)
}

/// Writes (or rewrites) the repository's commit-graph file, with the changed-path filters that
/// make a file's history fast: `git commit-graph write --reachable --changed-paths`.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn write_commit_graph(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("commit-graph")
        .args(["write", "--reachable", "--changed-paths"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut lines = Lines::default();
        let mut found = Vec::new();
        for chunk in chunks {
            let _ = lines.feed(chunk, &mut |line| {
                found.push(String::from_utf8(line.to_vec()).unwrap());
                ControlFlow::Continue(())
            });
        }
        found
    }

    #[test]
    fn lines_survive_any_chunking() {
        assert_eq!(lines_of(&[b"a b\nc d\n"]), ["a b", "c d"]);
        assert_eq!(lines_of(&[b"a ", b"b\nc", b" d", b"\n"]), ["a b", "c d"]);
        assert_eq!(lines_of(&[b"", b"one\n", b"", b"two\n"]), ["one", "two"]);
        assert_eq!(lines_of(&[b"no end"]), Vec::<String>::new());
    }

    #[test]
    fn reads_name_status() {
        let output =
            b"M\0src/lib.rs\0A\0new\0D\0gone\0R087\0old name\0new name\0C100\0a\0b\0T\0link\0";
        let changes = parse_name_status(output).unwrap();
        assert_eq!(changes.len(), 6);
        assert_eq!(
            changes[0],
            FileChange {
                path: "src/lib.rs".into(),
                kind: ChangeKind::Modified
            }
        );
        assert_eq!(changes[1].kind, ChangeKind::Added);
        assert_eq!(changes[2].kind, ChangeKind::Deleted);
        assert_eq!(
            changes[3],
            FileChange {
                path: "new name".into(),
                kind: ChangeKind::Renamed {
                    from: "old name".into(),
                    similarity: 87
                }
            }
        );
        assert_eq!(
            changes[4].kind,
            ChangeKind::Copied {
                from: "a".into(),
                similarity: 100
            }
        );
        assert_eq!(changes[5].kind, ChangeKind::TypeChanged);
        assert!(parse_name_status(b"M\0").is_err());
        assert!(parse_name_status(b"R100\0only-one\0").is_err());
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
