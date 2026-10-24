//! Taking back what egit did, and doing it again: each action egit carries out notes how (the
//! branch and commit before and after it, a ref it made or removed, a stash), and an undo checks
//! the repository is still as the action left it before reversing it: a soft reset for a
//! commit (its changes staged again), a reset keeping uncommitted changes for anything else
//! that moved the branch, a switch back, a branch or tag made again or removed, a stash popped
//! or stored again. Actions with the world outside (a push, a fetch) aren't taken back.
//!
//! Undone actions can be done again (redo), until a new action is taken; both lists last as long
//! as the tab.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, DiscardError, DiscardSide, DiscardedFile, Forwarded, Git, GitError, IgnoreError,
    Ignored, Integrated, ObjectId, Pulled, Repository, ResetMode, Stash, SwitchError, SwitchTo,
    create_branch, create_ref, delete_branch, delete_ref, ignore_again, move_branch, put_back,
    ref_target, rename_branch, reset, stash_apply, stash_drop, stash_push, stash_store, stashes,
    switch, unignore,
};
use parking_lot::Mutex;

use crate::problem::Problem;
use crate::tasks::{Outcome, Task};

/// The most actions kept to take back.
const KEPT: usize = 100;

/// The digits of a commit's name an action's words show.
const ID_DIGITS: usize = 8;

/// Where `HEAD` is: on a branch (at a commit, unless it has none yet), or detached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Branch { name: BString, at: Option<ObjectId> },
    Detached(ObjectId),
}

impl Place {
    /// Where `HEAD` is now, if that can be read.
    pub(crate) async fn read(git: &Git, repository: &Repository) -> Option<Self> {
        here(git, repository).await.ok()
    }

    /// Its commit.
    pub(crate) const fn commit(&self) -> Option<ObjectId> {
        match self {
            Self::Branch { at, .. } => *at,
            Self::Detached(commit) => Some(*commit),
        }
    }

    /// Its branch's name, when on one.
    fn branch(&self) -> Option<&BStr> {
        match self {
            Self::Branch { name, .. } => Some(name.as_bstr()),
            Self::Detached(_) => None,
        }
    }

    /// Where a switch goes to come here.
    fn switch_to(&self) -> SwitchTo {
        match self {
            Self::Branch { name, .. } => SwitchTo::Branch(name.clone()),
            Self::Detached(commit) => SwitchTo::Detached(*commit),
        }
    }

    /// The same place, for an undo's check: the same branch, or the same detached commit.
    fn same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Branch { name, .. }, Self::Branch { name: other, .. }) => name == other,
            (Self::Detached(commit), Self::Detached(other)) => commit == other,
            _ => false,
        }
    }
}

/// One change an action made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// `HEAD` (on `branch`, or detached) moved from `before` to `after`; moved back (and
    /// forward again) by a reset in `mode`: soft for a commit (its changes staged again), a
    /// soft or mixed reset's own, otherwise keeping uncommitted changes.
    Moved {
        branch: Option<BString>,
        before: ObjectId,
        after: ObjectId,
        mode: ResetMode,
    },
    /// `HEAD` went from one place to another.
    Switched { from: Place, to: Place },
    /// A branch (its short name) made at `at`.
    BranchMade { name: BString, at: ObjectId },
    /// A branch removed from `was`.
    BranchRemoved { name: BString, was: ObjectId },
    /// A branch renamed.
    BranchRenamed { from: BString, to: BString },
    /// A branch not checked out moved from `from` to `to`.
    BranchMoved {
        name: BString,
        from: ObjectId,
        to: ObjectId,
    },
    /// A tag (its full name) made, at `at` (a commit, or an annotated tag's object).
    TagMade { name: BString, at: ObjectId },
    /// A tag removed from `was`.
    TagRemoved { name: BString, was: ObjectId },
    /// The changes stashed as `id` (those at `paths`, when any are named).
    Stashed {
        id: ObjectId,
        message: String,
        untracked: bool,
        paths: Vec<BString>,
    },
    /// The stash `id` dropped.
    StashDropped { id: ObjectId, message: String },
    /// Changes not staged discarded: each file as it was, and as the discard left it.
    Discarded(Vec<DiscardedFile>),
    /// A pattern added to an ignore file: what was added at its end.
    Ignored(Ignored),
}

/// An action that can be taken back: what it was, in words, and its changes, in the order made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Undo {
    pub(crate) what: String,
    pub(crate) changes: Vec<Change>,
}

/// What a job reports to the journal.
#[derive(Debug)]
pub(crate) enum Landed {
    /// An action taken.
    Done(Undo),
    /// An action taken back.
    Undone(Undo),
    /// An action done again (its changes as they were done this time).
    Redone(Undo),
    /// An undo (or, `redo`, a redo) that stopped before changing anything, for trying again.
    Kept { undo: Undo, redo: bool },
}

/// The actions to take back, and those taken back, to do again.
#[derive(Debug, Default)]
pub(crate) struct Journal {
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    /// What jobs report.
    inbox: Arc<Mutex<Vec<Landed>>>,
}

impl Journal {
    /// Where jobs report what they did.
    pub(crate) fn inbox(&self) -> Arc<Mutex<Vec<Landed>>> {
        Arc::clone(&self.inbox)
    }

    /// Takes in what jobs reported: an action taken (which forgets what was undone), undone,
    /// or done again.
    pub(crate) fn poll(&mut self) {
        let landed = std::mem::take(&mut *self.inbox.lock());
        for landed in landed {
            match landed {
                Landed::Done(undo) => {
                    self.redo.clear();
                    self.keep(undo);
                }
                Landed::Undone(undo) | Landed::Kept { undo, redo: true } => self.redo.push(undo),
                Landed::Redone(undo) | Landed::Kept { undo, redo: false } => self.keep(undo),
            }
        }
    }

    fn keep(&mut self, undo: Undo) {
        self.undo.push(undo);
        if self.undo.len() > KEPT {
            self.undo.remove(0);
        }
    }

    /// The action an undo would take back.
    pub(crate) fn next_undo(&self) -> Option<&Undo> {
        self.undo.last()
    }

    /// The action a redo would do again.
    pub(crate) fn next_redo(&self) -> Option<&Undo> {
        self.redo.last()
    }

    /// Takes the next action to undo (it comes back if the undo stops before changing anything).
    pub(crate) fn take_undo(&mut self) -> Option<Undo> {
        self.undo.pop()
    }

    /// Takes the next action to redo.
    pub(crate) fn take_redo(&mut self) -> Option<Undo> {
        self.redo.pop()
    }
}

/// Why a change couldn't be taken back or done again.
#[derive(Debug)]
struct Halt {
    /// The repository isn't as the action left it: trying again won't help.
    moved_on: bool,
    /// What, in words.
    detail: String,
}

/// The repository isn't as the action left it: `detail` says how.
fn moved_on(detail: impl Into<String>) -> Halt {
    Halt {
        moved_on: true,
        detail: detail.into(),
    }
}

/// Git failed or refused (changes in the way, say): its own words.
fn failed(error: &GitError) -> Halt {
    Halt {
        moved_on: false,
        detail: Problem::git("", error).detail,
    }
}

/// An undo or redo that stopped: why, and whether it can be tried again (nothing was changed,
/// and the repository is still as the action left it).
#[derive(Debug)]
pub(crate) struct Stopped {
    pub(crate) problem: Problem,
    pub(crate) again: bool,
}

impl Halt {
    /// What the user reads, `done` changes into an undo (`redo`: a redo).
    fn stopped(self, redo: bool, done: usize) -> Stopped {
        let title = match (redo, self.moved_on) {
            (false, true) => "It can't be taken back: things changed since",
            (false, false) => "It couldn't be taken back",
            (true, true) => "It can't be done again: things changed since",
            (true, false) => "It couldn't be done again",
        };
        let mut detail = self.detail;
        if done > 0 {
            detail.push_str(if redo {
                " Part of it was done again already."
            } else {
                " Part of it was taken back already."
            });
        }
        if self.moved_on || done > 0 {
            detail.push_str(" The reflog lists where HEAD and each branch have been.");
        }
        Stopped {
            problem: Problem::new(title, detail),
            again: !self.moved_on && done == 0,
        }
    }
}

/// The object `name` (a full ref name) points to, if it exists.
async fn at(git: &Git, repository: &Repository, name: &str) -> Result<Option<ObjectId>, Halt> {
    ref_target(git, repository, BStr::new(name))
        .await
        .map_err(|error| failed(&error))
}

/// Where `HEAD` is now.
async fn here(git: &Git, repository: &Repository) -> Result<Place, Halt> {
    let head = repository
        .head_ref(git)
        .await
        .map_err(|error| failed(&error))?;
    Ok(match (head.branch(), head.commit) {
        (Some(name), at) => Place::Branch {
            name: name.to_owned(),
            at,
        },
        (None, Some(commit)) => Place::Detached(commit),
        (None, None) => return Err(moved_on("HEAD points nowhere.")),
    })
}

/// Takes `undo` back: its changes, last first, each checked first.
pub(crate) async fn take_back(
    git: &Git,
    repository: &Repository,
    undo: &Undo,
    cancel: &CancelToken,
) -> Result<Undo, Stopped> {
    for (done, change) in undo.changes.iter().rev().enumerate() {
        reverse(git, repository, change, cancel)
            .await
            .map_err(|halt| halt.stopped(false, done))?;
    }
    Ok(undo.clone())
}

/// Does `undo` again: its changes, first first, each checked first; returns them as done this
/// time (a stash made again is a new one).
pub(crate) async fn do_again(
    git: &Git,
    repository: &Repository,
    undo: &Undo,
    cancel: &CancelToken,
) -> Result<Undo, Stopped> {
    let mut changes = Vec::with_capacity(undo.changes.len());
    for change in &undo.changes {
        let done = changes.len();
        changes.push(
            redo(git, repository, change, cancel)
                .await
                .map_err(|halt| halt.stopped(true, done))?,
        );
    }
    Ok(Undo {
        what: undo.what.clone(),
        changes,
    })
}

/// Takes one change back.
async fn reverse(
    git: &Git,
    repository: &Repository,
    change: &Change,
    cancel: &CancelToken,
) -> Result<(), Halt> {
    match change {
        Change::Moved {
            branch,
            before,
            after,
            mode,
        } => {
            let now = here(git, repository).await?;
            if now.branch() != branch.as_ref().map(|branch| branch.as_bstr())
                || now.commit() != Some(*after)
            {
                return Err(moved_on("HEAD isn't where the action left it."));
            }
            reset(git, repository, *before, *mode, cancel)
                .await
                .map_err(|error| failed(&error))
        }
        Change::Switched { from, to } => switch_back(git, repository, to, from, cancel).await,
        Change::BranchMade { name, at: made } => {
            if at(git, repository, &format!("refs/heads/{name}")).await? != Some(*made) {
                return Err(moved_on(format!("{name} moved since it was made.")));
            }
            delete_branch(git, repository, name.as_bstr(), true, cancel)
                .await
                .map_err(|error| Halt {
                    moved_on: false,
                    detail: error.to_string(),
                })
        }
        Change::BranchRemoved { name, was } => {
            create_branch(git, repository, name.as_bstr(), *was, cancel)
                .await
                .map_err(|error| failed(&error))
        }
        Change::BranchRenamed { from, to } => {
            rename_branch(git, repository, to.as_bstr(), from.as_bstr(), cancel)
                .await
                .map_err(|error| failed(&error))
        }
        Change::BranchMoved { name, from, to } => {
            move_back(git, repository, name.as_bstr(), *to, *from, cancel).await
        }
        Change::TagMade { name, at } => delete_ref(git, repository, name.as_bstr(), *at, cancel)
            .await
            .map_err(|_| moved_on(format!("{name} isn't where it was made."))),
        Change::TagRemoved { name, was } => {
            create_ref(git, repository, name.as_bstr(), *was, cancel)
                .await
                .map_err(|_| moved_on(format!("{name} exists again.")))
        }
        Change::Stashed { id, .. } => {
            let latest = latest_stash(git, repository, cancel).await?;
            if latest.as_ref().map(|stash| stash.id) != Some(*id) {
                return Err(moved_on("The stash isn't the latest any more."));
            }
            let Some(stash) = latest else {
                return Ok(());
            };
            stash_apply(git, repository, &stash, true, cancel)
                .await
                .map(drop)
                .map_err(|error| Halt {
                    moved_on: false,
                    detail: error.to_string(),
                })
        }
        Change::StashDropped { id, message } => stash_store(git, repository, *id, message, cancel)
            .await
            .map_err(|error| failed(&error)),
        Change::Discarded(files) => put_back(git, repository, files, DiscardSide::Before, cancel)
            .await
            .map_err(discard_halt),
        Change::Ignored(ignored) => unignore(git, ignored).await.map_err(ignore_halt),
    }
}

/// Why patterns couldn't be taken back (or added again).
fn ignore_halt(error: IgnoreError) -> Halt {
    match error {
        IgnoreError::Changed(file) => moved_on(format!("{} changed since.", file.display())),
        error => Halt {
            moved_on: false,
            detail: format!("{error}."),
        },
    }
}

/// Why discarded files couldn't be put back (or discarded again).
fn discard_halt(error: DiscardError) -> Halt {
    match error {
        DiscardError::Stale(path) => moved_on(format!("{} changed since.", path.to_str_lossy())),
        DiscardError::Git(error) => failed(&error),
        error => Halt {
            moved_on: false,
            detail: format!("{error}."),
        },
    }
}

/// Does one change again; returns it as done this time.
async fn redo(
    git: &Git,
    repository: &Repository,
    change: &Change,
    cancel: &CancelToken,
) -> Result<Change, Halt> {
    match change {
        Change::Moved {
            branch,
            before,
            after,
            mode,
        } => {
            let now = here(git, repository).await?;
            if now.branch() != branch.as_ref().map(|branch| branch.as_bstr())
                || now.commit() != Some(*before)
            {
                return Err(moved_on("HEAD isn't where the undo left it."));
            }
            reset(git, repository, *after, *mode, cancel)
                .await
                .map_err(|error| failed(&error))?;
        }
        Change::Switched { from, to } => switch_back(git, repository, from, to, cancel).await?,
        Change::BranchMade { name, at } => {
            create_branch(git, repository, name.as_bstr(), *at, cancel)
                .await
                .map_err(|error| failed(&error))?;
        }
        Change::BranchRemoved { name, was } => {
            if at(git, repository, &format!("refs/heads/{name}")).await? != Some(*was) {
                return Err(moved_on(format!("{name} moved since.")));
            }
            delete_branch(git, repository, name.as_bstr(), true, cancel)
                .await
                .map_err(|error| Halt {
                    moved_on: false,
                    detail: error.to_string(),
                })?;
        }
        Change::BranchRenamed { from, to } => {
            rename_branch(git, repository, from.as_bstr(), to.as_bstr(), cancel)
                .await
                .map_err(|error| failed(&error))?;
        }
        Change::BranchMoved { name, from, to } => {
            move_back(git, repository, name.as_bstr(), *from, *to, cancel).await?;
        }
        Change::TagMade { name, at } => {
            create_ref(git, repository, name.as_bstr(), *at, cancel)
                .await
                .map_err(|_| moved_on(format!("{name} exists again.")))?;
        }
        Change::TagRemoved { name, was } => {
            delete_ref(git, repository, name.as_bstr(), *was, cancel)
                .await
                .map_err(|_| moved_on(format!("{name} moved since.")))?;
        }
        Change::Stashed {
            message,
            untracked,
            paths,
            ..
        } => {
            let named: Vec<&BStr> = paths.iter().map(|path| path.as_bstr()).collect();
            stash_push(git, repository, (message, *untracked), &named, cancel)
                .await
                .map_err(|error| failed(&error))?;
            let made = latest_stash(git, repository, cancel)
                .await?
                .ok_or_else(|| moved_on("There was nothing to stash."))?;
            return Ok(Change::Stashed {
                id: made.id,
                message: message.clone(),
                untracked: *untracked,
                paths: paths.clone(),
            });
        }
        Change::StashDropped { id, .. } => {
            let latest = latest_stash(git, repository, cancel).await?;
            match latest {
                Some(stash) if stash.id == *id => stash_drop(git, repository, &stash, cancel)
                    .await
                    .map_err(|error| Halt {
                        moved_on: false,
                        detail: error.to_string(),
                    })?,
                _ => return Err(moved_on("The stash isn't the latest any more.")),
            }
        }
        Change::Discarded(files) => {
            put_back(git, repository, files, DiscardSide::After, cancel)
                .await
                .map_err(discard_halt)?;
        }
        Change::Ignored(ignored) => {
            return ignore_again(git, ignored)
                .await
                .map(Change::Ignored)
                .map_err(ignore_halt);
        }
    }
    Ok(change.clone())
}

/// Switches from `now` (where `HEAD` must be) to `to`.
async fn switch_back(
    git: &Git,
    repository: &Repository,
    now: &Place,
    to: &Place,
    cancel: &CancelToken,
) -> Result<(), Halt> {
    if !here(git, repository).await?.same_as(now) {
        return Err(moved_on("HEAD isn't where the switch left it."));
    }
    match switch(git, repository, &to.switch_to(), cancel).await {
        Ok(()) => Ok(()),
        Err(SwitchError::WouldOverwrite { paths, .. }) => Err(Halt {
            moved_on: false,
            detail: format!(
                "Changes are in the way of switching: {}. Commit or stash them first.",
                paths
                    .iter()
                    .map(|path| path.to_str_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
        Err(SwitchError::Git(error)) => Err(failed(&error)),
    }
}

/// Moves the branch `name` from `now` (where it must be) to `to`.
async fn move_back(
    git: &Git,
    repository: &Repository,
    name: &BStr,
    now: ObjectId,
    to: ObjectId,
    cancel: &CancelToken,
) -> Result<(), Halt> {
    if at(git, repository, &format!("refs/heads/{name}")).await? != Some(now) {
        return Err(moved_on(format!("{name} moved since.")));
    }
    move_branch(git, repository, name, to, cancel)
        .await
        .map_err(|error| failed(&error))
}

/// The latest stash, if any.
async fn latest_stash(
    git: &Git,
    repository: &Repository,
    cancel: &CancelToken,
) -> Result<Option<Stash>, Halt> {
    stashes(git, repository, cancel)
        .await
        .map(|stashes| stashes.into_iter().next())
        .map_err(|error| failed(&error))
}

/// What an action's undo needs from before it: where `HEAD` was, and the branch or tag it
/// removes or moves (only for an action that can be taken back).
#[derive(Debug, Default)]
pub(crate) struct Before {
    head: Option<Place>,
    branch: Option<ObjectId>,
    tag: Option<ObjectId>,
    /// The latest stash, before an action that may stash (or find nothing to).
    stash: Option<ObjectId>,
}

/// Reads what `task`'s undo will need, before it runs (nothing for one that can't be undone).
pub(crate) async fn before(git: &Git, repository: &Repository, task: &Task) -> Before {
    let head = || Place::read(git, repository);
    let named = async |name: String| at(git, repository, &name).await.ok().flatten();
    match task {
        Task::DeleteBranch { name, .. } | Task::DeleteBranchEverywhere { name, .. } => Before {
            branch: named(format!("refs/heads/{name}")).await,
            ..Before::default()
        },
        Task::FastForwardBranch { branch, .. } => Before {
            branch: named(format!("refs/heads/{branch}")).await,
            ..Before::default()
        },
        Task::DeleteTag(name) => Before {
            tag: named(format!("refs/tags/{name}")).await,
            ..Before::default()
        },
        Task::StashAndSwitch(_)
        | Task::PullStashing {
            untracked: true, ..
        } => Before {
            head: head().await,
            stash: latest_stash(git, repository, &CancelToken::new())
                .await
                .ok()
                .flatten()
                .map(|stash| stash.id),
            ..Before::default()
        },
        task if moves_head(task) => Before {
            head: head().await,
            ..Before::default()
        },
        _ => Before::default(),
    }
}

/// Whether `task` moves `HEAD` (or the branch it is on).
const fn moves_head(task: &Task) -> bool {
    matches!(
        task,
        Task::Merge { .. }
            | Task::Rebase { .. }
            | Task::CherryPick(_)
            | Task::CherryPickAll(_)
            | Task::Revert { .. }
            | Task::RevertAll(_)
            | Task::Reset(..)
            | Task::InteractiveRebase { .. }
            | Task::DropCommit(_)
            | Task::SquashIntoParent(_)
            | Task::Reword { .. }
            | Task::SignCommits { .. }
            | Task::FastForward { .. }
            | Task::ApplyPatch { .. }
            | Task::Pull(_)
            | Task::PullStashing { .. }
            | Task::PullAndRemember { .. }
            | Task::Switch(_)
            | Task::StashAndSwitch(_)
            | Task::CheckOutPullRequest { .. }
    )
}

/// The stash (of every change, untracked ones too) an action made: the latest, where it isn't
/// the one there was before (`since`).
async fn stash_made(
    git: &Git,
    repository: &Repository,
    since: Option<ObjectId>,
    message: &str,
) -> Option<Change> {
    let latest = latest_stash(git, repository, &CancelToken::new())
        .await
        .ok()
        .flatten()?;
    (Some(latest.id) != since).then(|| Change::Stashed {
        id: latest.id,
        message: message.to_owned(),
        untracked: true,
        paths: Vec::new(),
    })
}

/// How to take back what `task` did, now that it ended as `outcome` (`None` when it can't be,
/// or changed nothing).
pub(crate) async fn noted(
    git: &Git,
    repository: &Repository,
    task: &Task,
    before: Before,
    outcome: &Outcome,
) -> Option<Undo> {
    if !went_through(outcome) {
        return None;
    }
    let short = |name: &BString| name.to_str_lossy().into_owned();
    let head = || Place::read(git, repository);
    let what: String;
    let mut changes = Vec::new();
    match task {
        Task::Switch(to) | Task::StashAndSwitch(to) | Task::CheckOutPullRequest { to, .. } => {
            let (from, to_place) = (before.head?, head().await?);
            // Only a stash this made (with nothing to stash, git makes none).
            if let Task::StashAndSwitch(_) = task
                && let Some(made) =
                    stash_made(git, repository, before.stash, "Before switching").await
            {
                changes.push(made);
            }
            match to {
                SwitchTo::NewBranch { name, at } => changes.push(Change::BranchMade {
                    name: name.clone(),
                    at: *at,
                }),
                SwitchTo::Track { local, .. } => changes.push(Change::BranchMade {
                    name: local.clone(),
                    at: to_place.commit()?,
                }),
                SwitchTo::Branch(_) | SwitchTo::Detached(_) => {}
            }
            what = match &to_place {
                Place::Branch { name, .. } => format!("the switch to {}", short(name)),
                Place::Detached(commit) => {
                    format!("the switch to {}", commit.abbreviated(ID_DIGITS))
                }
            };
            changes.push(Change::Switched { from, to: to_place });
        }
        task if moves_head(task) => {
            let stashed_before = before.stash;
            let (from, to) = (before.head?, head().await?);
            let (before, after) = (from.commit()?, to.commit()?);
            if before == after || !from.same_as(&to) {
                return None;
            }
            what = moved_words(task);
            // Only a stash this made, put back after the pull is taken back.
            if let Task::PullStashing {
                untracked: true, ..
            } = task
                && let Some(made) =
                    stash_made(git, repository, stashed_before, "Before pulling").await
            {
                changes.push(made);
            }
            // A soft or mixed reset is taken back the same way (its changes are uncommitted
            // ones, in the way of keeping them); anything else keeps what is uncommitted.
            let mode = match task {
                Task::Reset(_, mode @ (ResetMode::Soft | ResetMode::Mixed)) => *mode,
                _ => ResetMode::Keep,
            };
            changes.push(Change::Moved {
                branch: to.branch().map(BStr::to_owned),
                before,
                after,
                mode,
            });
        }
        Task::Ignore(ignore) => {
            let Outcome::Ignored { ignored, .. } = outcome else {
                return None;
            };
            if ignored.added.is_empty() {
                return None;
            }
            what = format!("ignoring {}", ignore.words());
            changes.push(Change::Ignored(ignored.clone()));
        }
        Task::Discard(discard) => {
            let Outcome::Discarded(Some(files)) = outcome else {
                return None;
            };
            what = format!("discarding {}", discard.words());
            changes.push(Change::Discarded(files.clone()));
        }
        task => {
            let (words, change) = noted_ref(git, repository, task, &before).await?;
            what = words;
            changes.push(change);
        }
    }
    Some(Undo { what, changes })
}

/// How to take back what `task` did to a branch, tag, or stash (not the current branch).
async fn noted_ref(
    git: &Git,
    repository: &Repository,
    task: &Task,
    before: &Before,
) -> Option<(String, Change)> {
    let short = |name: &BString| name.to_str_lossy().into_owned();
    let named = async |name: String| at(git, repository, &name).await.ok().flatten();
    let latest = async || {
        latest_stash(git, repository, &CancelToken::new())
            .await
            .ok()
            .flatten()
    };
    Some(match task {
        Task::CreateBranch { name, at } => (
            format!("making the branch {}", short(name)),
            Change::BranchMade {
                name: name.clone(),
                at: *at,
            },
        ),
        Task::RenameBranch { old, new } => (
            format!("renaming {} to {}", short(old), short(new)),
            Change::BranchRenamed {
                from: old.clone(),
                to: new.clone(),
            },
        ),
        Task::DeleteBranch { name, .. } | Task::DeleteBranchEverywhere { name, .. } => (
            if let Task::DeleteBranchEverywhere { remote, .. } = task {
                format!(
                    "deleting {} here (on {} it stays deleted)",
                    short(name),
                    short(remote)
                )
            } else {
                format!("deleting {}", short(name))
            },
            Change::BranchRemoved {
                name: name.clone(),
                was: before.branch?,
            },
        ),
        Task::FastForwardBranch { branch, shown, .. } => (
            format!("fast-forwarding {} to {shown}", short(branch)),
            Change::BranchMoved {
                name: branch.clone(),
                from: before.branch?,
                to: named(format!("refs/heads/{branch}")).await?,
            },
        ),
        Task::CreateTag { name, .. } => {
            let full = format!("refs/tags/{name}");
            (
                format!("making the tag {}", short(name)),
                Change::TagMade {
                    at: named(full.clone()).await?,
                    name: full.into(),
                },
            )
        }
        Task::DeleteTag(name) => (
            format!("deleting the tag {}", short(name)),
            Change::TagRemoved {
                name: format!("refs/tags/{name}").into(),
                was: before.tag?,
            },
        ),
        Task::Stash {
            message,
            untracked,
            paths,
        } => (
            "stashing the changes".to_owned(),
            Change::Stashed {
                id: latest().await?.id,
                message: message.clone(),
                untracked: *untracked,
                paths: paths.clone(),
            },
        ),
        Task::DropStash(stash) => (
            format!("dropping stash@{{{}}}", stash.index),
            Change::StashDropped {
                id: stash.id,
                message: stash.message.clone(),
            },
        ),
        _ => return None,
    })
}

/// Whether a task went all the way (an operation stopped part-way is aborted, not undone).
const fn went_through(outcome: &Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Done
            | Outcome::Integrated(Integrated::Done | Integrated::Empty)
            | Outcome::Pulled(Pulled::Updated)
            | Outcome::Forwarded(Forwarded::Moved)
            | Outcome::Stashed(true)
            | Outcome::Signed { .. }
            | Outcome::Discarded(Some(_))
            | Outcome::Ignored { .. }
    )
}

/// What a task that moved `HEAD` was, in words.
fn moved_words(task: &Task) -> String {
    let short = |commit: &ObjectId| commit.abbreviated(ID_DIGITS).to_string();
    match task {
        Task::Merge { shown, .. } => format!("the merge of {shown}"),
        Task::Rebase { shown, .. } => format!("the rebase onto {shown}"),
        Task::CherryPick(commit) => format!("the cherry-pick of {}", short(commit)),
        Task::CherryPickAll(commits) => format!("the cherry-pick of {} commits", commits.len()),
        Task::Revert { commit, .. } => format!("the revert of {}", short(commit)),
        Task::RevertAll(commits) => format!("the revert of {} commits", commits.len()),
        Task::Reset(commit, ResetMode::Hard) => format!(
            "the reset to {} (the changes it discarded can't come back)",
            short(commit)
        ),
        Task::Reset(commit, _) => format!("the reset to {}", short(commit)),
        Task::InteractiveRebase { .. } => "the interactive rebase".to_owned(),
        Task::DropCommit(commit) => format!("dropping {}", short(commit)),
        Task::SquashIntoParent(commit) => format!("squashing {} into its parent", short(commit)),
        Task::Reword { commit, .. } => format!("rewording {}", short(commit)),
        Task::SignCommits { commits } if commits.len() == 1 => "signing a commit".to_owned(),
        Task::SignCommits { commits } => format!("signing {} commits", commits.len()),
        Task::FastForward { shown, .. } => format!("the fast-forward to {shown}"),
        Task::ApplyPatch { commits: 1, .. } => "applying the patch".to_owned(),
        Task::ApplyPatch { commits, .. } => format!("applying {commits} patches"),
        _ => "the pull".to_owned(),
    }
}

/// How to take back a commit (an amend too): the branch from where it was to the new commit,
/// by a soft reset (its changes staged again).
pub(crate) fn committed(
    before: Option<Place>,
    after: Option<Place>,
    subject: &str,
    amend: bool,
) -> Option<Undo> {
    let (from, to) = (before?, after?);
    let (before, after) = (from.commit()?, to.commit()?);
    if !from.same_as(&to) && from.branch() != to.branch() {
        return None;
    }
    Some(Undo {
        what: if amend {
            format!("amending “{subject}”")
        } else {
            format!("the commit “{subject}”")
        },
        changes: vec![Change::Moved {
            branch: to.branch().map(BStr::to_owned),
            before,
            after,
            mode: ResetMode::Soft,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn undo(what: &str) -> Undo {
        Undo {
            what: what.to_owned(),
            changes: Vec::new(),
        }
    }

    #[test]
    fn a_new_action_forgets_what_was_undone() {
        let mut journal = Journal::default();
        let inbox = journal.inbox();
        inbox
            .lock()
            .extend([Landed::Done(undo("one")), Landed::Done(undo("two"))]);
        journal.poll();
        assert_eq!(
            journal.next_undo().map(|undo| undo.what.as_str()),
            Some("two")
        );
        let taken = journal.take_undo().unwrap();
        inbox.lock().push(Landed::Undone(taken));
        journal.poll();
        assert_eq!(
            journal.next_redo().map(|undo| undo.what.as_str()),
            Some("two")
        );
        assert_eq!(
            journal.next_undo().map(|undo| undo.what.as_str()),
            Some("one")
        );
        inbox.lock().push(Landed::Done(undo("three")));
        journal.poll();
        assert_eq!(journal.next_redo(), None, "a new action: nothing to redo");
        assert_eq!(
            journal.next_undo().map(|undo| undo.what.as_str()),
            Some("three")
        );
    }

    #[test]
    fn at_most_a_hundred_are_kept() {
        let mut journal = Journal::default();
        for n in 0..150 {
            journal
                .inbox()
                .lock()
                .push(Landed::Done(undo(&n.to_string())));
        }
        journal.poll();
        let mut count = 0;
        while journal.take_undo().is_some() {
            count += 1;
        }
        assert_eq!(count, KEPT);
    }
}
