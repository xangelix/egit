//! Staging and unstaging, as jobs: whole files (`git add`, `git reset`), the lines picked in a
//! file's diff (the index's blob edited and written back, refused when the file changed since
//! its diff was read), or a conflict's resolution (refused when its sides changed since they
//! were read).

use std::sync::Arc;

use bstr::ByteSlice as _;

use bstr::{BStr, BString};
use egit_git::{
    CancelToken, Direction, FileDiff, Git, LinesError, ObjectKind, Pick, Repository, Resolution,
    ResolveError, TreeEntry, apply_lines, conflict_sides, resolve_conflict, stage, unstage,
    write_index_content,
};

use crate::history::Objects;
use crate::jobs::{self, WriteLock};
use crate::merge::{Choice, ConflictFile};
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
    /// A conflict resolved.
    Resolve(ResolveRequest),
    /// Files in conflict resolved whole as one side has them (as a side that deleted a file
    /// would: deleted), as they are in the index now.
    TakeSide {
        paths: Vec<BString>,
        side: WholeSide,
    },
}

/// A side of conflicts taken whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WholeSide {
    /// `HEAD`'s.
    Current,
    /// What is brought in.
    Incoming,
}

/// A conflict to resolve, and how.
#[derive(Debug)]
pub(crate) struct ResolveRequest {
    /// The conflict, as it was read (its sides must still be the index's).
    pub(crate) file: Arc<ConflictFile>,
    pub(crate) how: Resolve,
}

/// How a conflict is resolved.
#[derive(Debug)]
pub(crate) enum Resolve {
    /// Each of its conflicts as chosen (none open).
    Merged(Arc<[Choice]>),
    /// The current side's file as it is.
    Current,
    /// The incoming side's.
    Incoming,
    /// The file deleted.
    Delete,
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
            StageRequest::Resolve(request) => resolve(&git, &repository, &request, &cancel),
            StageRequest::TakeSide { paths, side } => {
                take_side(&git, &repository, (&paths, side), &cancel)
            }
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
    let mode = entry.map_or_else(
        || work_tree_mode(git, repository, &path),
        |entry| entry.mode,
    );
    match write_index_content(git, repository, path.as_ref(), entry, mode, content, cancel) {
        Ok(_) => Ok(()),
        Err(LinesError::Stale) => Err(stale()),
        Err(LinesError::Git(error)) => Err(Problem::git(title, &error)),
    }
}

/// Resolves a conflict as `request` says, if its sides are still the index's.
fn resolve(
    git: &Git,
    repository: &Repository,
    request: &ResolveRequest,
    cancel: &CancelToken,
) -> Result<(), Problem> {
    const TITLE: &str = "The conflict couldn't be resolved";
    let ResolveRequest { file, how } = request;
    let missing = |side| Problem::new(TITLE, format!("The {side} side deleted the file."));
    let resolution = match how {
        Resolve::Merged(choices) => {
            let text = file
                .text()
                .ok_or_else(|| Problem::new(TITLE, "The file's sides aren't two texts."))?;
            let mut open = false;
            let content = text.resolve(|index| {
                choices
                    .get(index)
                    .and_then(Choice::pick)
                    .unwrap_or_else(|| {
                        open = true;
                        Pick::Current
                    })
            });
            if open {
                return Err(Problem::new(
                    TITLE,
                    "Some of its conflicts aren't resolved yet.",
                ));
            }
            let mode = file.sides.merged_mode().unwrap_or(0o100_644);
            Resolution::Content { content, mode }
        }
        Resolve::Current => Resolution::Side(file.sides.current.ok_or_else(|| missing("current"))?),
        Resolve::Incoming => {
            Resolution::Side(file.sides.incoming.ok_or_else(|| missing("incoming"))?)
        }
        Resolve::Delete => Resolution::Delete,
    };
    resolve_conflict(
        git,
        repository,
        file.path.as_ref(),
        &file.sides,
        resolution,
        cancel,
    )
    .map_err(|error| match error {
        ResolveError::Stale => Problem::new(
            TITLE,
            "Its sides changed since egit read them (another program resolved it, say). Look \
             at it again.",
        ),
        ResolveError::Git(error) => Problem::git(TITLE, &error),
    })
}

/// Resolves each file at `paths` as `side` has it, the sides read now (one that is no longer
/// in conflict, resolved meanwhile, is left as it is).
fn take_side(
    git: &Git,
    repository: &Repository,
    (paths, side): (&[BString], WholeSide),
    cancel: &CancelToken,
) -> Result<(), Problem> {
    const TITLE: &str = "The conflicts couldn't be resolved";
    for path in paths {
        let path = path.as_ref();
        let Some(sides) = conflict_sides(git, repository, path, cancel)
            .map_err(|error| Problem::git(TITLE, &error))?
        else {
            continue;
        };
        let taken = match side {
            WholeSide::Current => sides.current,
            WholeSide::Incoming => sides.incoming,
        };
        let resolution = taken.map_or(Resolution::Delete, Resolution::Side);
        resolve_conflict(git, repository, path, &sides, resolution, cancel).map_err(|error| {
            match error {
                ResolveError::Stale => Problem::new(
                    TITLE,
                    "A file's sides changed while it was resolved. Look at it again.",
                ),
                ResolveError::Git(error) => Problem::git(TITLE, &error),
            }
        })?;
    }
    Ok(())
}

/// The mode a file not yet tracked gets: executable or not, as the work tree has it.
fn work_tree_mode(git: &Git, repository: &Repository, path: &[u8]) -> u32 {
    let file = repository.work_tree().join(path.to_os_str_lossy());
    let found = git
        .files()
        .look(&[&file])
        .ok()
        .and_then(|mut found| found.pop());
    if matches!(
        found,
        Some(egit_git::Entry::File {
            executable: true,
            ..
        })
    ) {
        0o100_755
    } else {
        0o100_644
    }
}
