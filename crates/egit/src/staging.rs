//! Staging and unstaging, as jobs: whole files (`git add`, `git reset`), or the lines picked in
//! a file's diff (the index's blob edited and written back, refused when the file changed since
//! its diff was read).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use bstr::{BStr, BString};
use egit_git::{
    CancelToken, Direction, FileDiff, Git, LinesError, ObjectKind, Repository, TreeEntry,
    apply_lines, stage, unstage, write_index_content,
};

use crate::history::Objects;
use crate::jobs::{self, WriteLock};
use crate::problem::Problem;

/// What to stage or unstage.
#[derive(Debug)]
pub(crate) enum StageRequest {
    /// Whole files: `paths` (a staged rename's both).
    Files {
        paths: Vec<BString>,
        direction: Direction,
    },
    /// Some lines of one file.
    Lines(Box<LinesRequest>),
}

/// Some lines of a file to stage or unstage.
#[derive(Debug)]
pub(crate) struct LinesRequest {
    pub(crate) path: BString,
    pub(crate) direction: Direction,
    /// The diff the lines were picked in: the index against the work tree to stage, `HEAD`
    /// against the index to unstage.
    pub(crate) diff: FileDiff,
    /// Which of the diff's lines were picked.
    pub(crate) picked: Vec<bool>,
    /// What the index held at the path when the diff was read (`None`: the file isn't
    /// tracked yet).
    pub(crate) entry: Option<TreeEntry>,
}

/// Stages or unstages as `request` says, once the repository's other writes are done.
pub(crate) async fn run(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    writes: WriteLock,
    request: StageRequest,
) -> Result<(), Problem> {
    jobs::blocking(move || {
        let _queued = writes.hold();
        let cancel = CancelToken::new();
        match request {
            StageRequest::Files { paths, direction } => {
                let paths: Vec<&BStr> = paths.iter().map(AsRef::as_ref).collect();
                let done = match direction {
                    Direction::Stage => stage(&git, &repository, &paths, &cancel),
                    Direction::Unstage => unstage(&git, &repository, &paths, &cancel),
                };
                done.map_err(|error| Problem::git(failed(direction), &error))
            }
            StageRequest::Lines(lines) => stage_lines(&git, &repository, &objects, *lines, &cancel),
        }
    })
    .await
}

/// What failed, for people.
const fn failed(direction: Direction) -> &'static str {
    match direction {
        Direction::Stage => "The changes couldn't be staged",
        Direction::Unstage => "The changes couldn't be unstaged",
    }
}

/// Moves the picked lines across: the index's content (as `request.entry` says it was) edited
/// in memory, then written over the entry if it is still that one.
fn stage_lines(
    git: &Git,
    repository: &Repository,
    objects: &Objects,
    request: LinesRequest,
    cancel: &CancelToken,
) -> Result<(), Problem> {
    let LinesRequest {
        path,
        direction,
        diff,
        picked,
        entry,
    } = request;
    let title = failed(direction);
    let base = match entry {
        Some(entry) => {
            let mut blob = None;
            objects
                .read(git, repository.work_tree(), &[entry.id], |_, object| {
                    blob = object
                        .filter(|object| object.kind == ObjectKind::Blob)
                        .map(|object| object.data.to_vec());
                })
                .map_err(|error| Problem::git(title, &error))?;
            blob.ok_or_else(|| Problem::new(title, "The index's copy of the file is missing."))?
        }
        None => Vec::new(),
    };
    let stale = || {
        Problem::new(
            title,
            "The file changed since its diff was read. Look at it again, then pick the lines \
             anew.",
        )
    };
    let content = apply_lines(&base, &diff, direction, |index| {
        picked.get(index).copied().unwrap_or(false)
    })
    .map_err(|_| stale())?;
    let mode = entry.map_or_else(|| work_tree_mode(repository, &path), |entry| entry.mode);
    match write_index_content(git, repository, path.as_ref(), entry, mode, content, cancel) {
        Ok(_) => Ok(()),
        Err(LinesError::Stale) => Err(stale()),
        Err(LinesError::Git(error)) => Err(Problem::git(title, &error)),
    }
}

/// The mode a file not yet tracked gets: executable or not, as the work tree has it.
fn work_tree_mode(repository: &Repository, path: &[u8]) -> u32 {
    let file = repository.work_tree().join(OsStr::from_bytes(path));
    let executable =
        std::fs::metadata(file).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
    if executable { 0o100_755 } else { 0o100_644 }
}
