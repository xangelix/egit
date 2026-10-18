//! What the user asks of a repository, as jobs: exchanges with its remotes (fetching, also in
//! the background every few minutes; pulling; pushing a branch or a tag; deleting a remote's
//! branch), switching and changing branches and tags, bringing commits in (merging, rebasing,
//! cherry-picking, reverting) and resetting, stashing, and giving up or carrying on an
//! operation git stopped in the middle of.
//!
//! One task the user asked for runs at a time, under the repository's `WriteLock`, its progress
//! in [`Tasks::progress`], stoppable with [`Tasks::cancel`]. The background fetch can't prompt,
//! doesn't take the lock (a fetch never touches the index), and is stopped when the user starts
//! a task; its failures are kept apart and said quietly.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bstr::{BStr, BString, ByteSlice as _};
use egit_forge::{
    Account, ForgeKind, ForgeRepository, NewPullRequest, Opened, create_pull_request,
};
use egit_git::{
    ApplyAs, BisectMark, CancelToken, Changes, ConfigEntry, ConfigSnapshot, ConfigTarget,
    DeleteBranchError, FastForward, FetchOptions, ForwardFrom, Forwarded, Git, GitError,
    Integrated, MAX_PLANNED, MAX_SIGNED, ObjectId, Operation, Planned, Progress, PullMode, Pulled,
    PushOptions, Pushed, RangeError, RebaseError, RebaseRange, RemoveWorktreeError, Repository,
    ResetMode, SignError, Stash, StashError, Step, SwitchError, SwitchTo, WorktreeFrom,
    abort_operation, add_remote, add_worktree, apply_patch, bisect_mark, changes_patch,
    cherry_pick, continue_operation, count_between, create_branch, create_tag, delete_branch,
    delete_remote_branch, delete_tag, fast_forward_branch, fast_forward_current, fetch,
    fetch_refspecs, fetch_remote, format_patch, format_patches, lock_worktree, merge, plan_signing,
    prune_worktrees, pull, push, push_tag, rebase, rebase_interactive, rebase_keeping_merges,
    rebase_range, remove_remote, remove_worktree, rename_branch, rename_remote, reset, revert,
    reword_head, set_config, set_remote_url, set_upstream, sign_commits, skip_operation,
    stash_apply, stash_drop, stash_push, switch, unlock_worktree, unset_config, update_submodules,
};
use egui_async::Bind;
use parking_lot::Mutex;

use crate::forge::{request_noun, request_number};
use crate::jobs::{self, WriteLock};
use crate::problem::Problem;
use crate::undo::{self, Landed, Undo};

/// How long after a repository opens its first background fetch starts.
const FIRST_BACKGROUND_FETCH: Duration = Duration::from_secs(10);

/// How often the background fetch runs, unless the preferences say otherwise.
const BACKGROUND_FETCH: Duration = Duration::from_secs(300);

/// How long the window may sleep when no background fetch is due.
const NO_FETCH_DUE: Duration = Duration::from_secs(3600);

/// A task the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Task {
    Fetch,
    Pull(PullMode),
    Push(PushOptions),
    Abort(Operation),
    Continue(Operation),
    /// Remember how pulls bring diverged branches together (`pull.rebase`), then pull so.
    PullAndRemember {
        rebase: bool,
    },
    /// Switch the work tree to a branch or a commit.
    Switch(SwitchTo),
    /// Stash the work tree's changes (untracked files too), then switch.
    StashAndSwitch(SwitchTo),
    /// Make a branch at a commit, without switching to it.
    CreateBranch {
        name: BString,
        at: ObjectId,
    },
    RenameBranch {
        old: BString,
        new: BString,
    },
    /// Delete a local branch (`force`: even if it isn't merged).
    DeleteBranch {
        name: BString,
        force: bool,
    },
    /// Delete a branch on a remote (its short name there).
    DeleteRemoteBranch {
        remote: BString,
        branch: BString,
    },
    /// Merge into the current branch: `what` is a full ref name or a commit's, `shown` its
    /// name for people.
    Merge {
        what: BString,
        shown: String,
        /// Whether it may just move the branch forward, or always makes a merge commit.
        fast_forward: FastForward,
    },
    /// Something of Git LFS's.
    Lfs(crate::lfs::LfsTask),
    /// Something done to submodules.
    Submodules(crate::submodules::SubmoduleTask),
    /// Rebase the current branch onto a ref or commit; `keep_merges`: its merges too
    /// (otherwise flattened).
    Rebase {
        onto: BString,
        shown: String,
        keep_merges: bool,
    },
    CherryPick(ObjectId),
    Revert(ObjectId),
    /// Rebase the current branch interactively: onto `base` (`None`: from the root), its
    /// commits as `plan` says (oldest first); `keep_merges`: the range holds merges, kept in
    /// git's own order.
    InteractiveRebase {
        base: Option<ObjectId>,
        plan: Vec<Planned>,
        keep_merges: bool,
    },
    /// Mark a commit good, bad, or skipped, starting a bisection when none is under way.
    Bisect {
        mark: BisectMark,
        commit: ObjectId,
    },
    /// Drop a commit from the current branch (what follows it rebased).
    DropCommit(ObjectId),
    /// Squash a commit into its parent (their messages joined).
    SquashIntoParent(ObjectId),
    /// Give a commit of the current branch a new message: `HEAD`'s by amending only its message
    /// (what is staged stays staged), an earlier one's by a rebase rewording it.
    Reword {
        commit: ObjectId,
        message: String,
    },
    /// Sign commits of the current branch: each made again, signed, and every commit after
    /// them too (`egit_git::sign_commits`).
    SignCommits {
        commits: Vec<ObjectId>,
    },
    /// Throw changes not staged away: whole files, or some lines of one.
    Discard(crate::discarding::Discard),
    /// Reset the current branch to a commit.
    Reset(ObjectId, ResetMode),
    /// Tag a commit: annotated with a message, or lightweight.
    CreateTag {
        name: BString,
        at: ObjectId,
        message: Option<String>,
    },
    DeleteTag(BString),
    PushTag {
        remote: BString,
        tag: BString,
    },
    /// Stash the work tree's changes.
    Stash {
        message: String,
        untracked: bool,
    },
    /// Apply a stash (`pop`: then drop it).
    ApplyStash {
        stash: Stash,
        pop: bool,
    },
    DropStash(Stash),
    /// Fetch a pull (or merge) request's head (`refspec`, from `remote`), then switch to it.
    CheckOutPullRequest {
        remote: BString,
        refspec: BString,
        kind: ForgeKind,
        number: u32,
        to: SwitchTo,
    },
    /// Open a pull request on a forge.
    CreatePullRequest(Box<PullRequestToOpen>),
    /// Fast-forward the current branch to a ref (its full name), `shown` its name for people.
    FastForward {
        to: BString,
        shown: String,
    },
    /// Fast-forward a branch that isn't checked out (its short name).
    FastForwardBranch {
        branch: BString,
        from: ForwardSource,
        shown: String,
    },
    /// Set a branch's upstream (a remote-tracking branch's short name), or forget it.
    SetUpstream {
        branch: BString,
        upstream: Option<BString>,
    },
    /// Delete a branch here, then its remote's (`force`: even if it isn't merged here).
    DeleteBranchEverywhere {
        name: BString,
        remote: BString,
        remote_branch: BString,
        force: bool,
    },
    /// Add a work tree at `path` checking out `from`; `open`: show it in a tab once made.
    AddWorktree {
        path: PathBuf,
        from: WorktreeSource,
        open: bool,
    },
    /// Save a commit's patch at `path`.
    SavePatch {
        commit: ObjectId,
        path: PathBuf,
    },
    /// Apply a patch to the current branch, as commits or changes; `commits`: how many it
    /// holds (for the words).
    ApplyPatch {
        patch: Patch,
        how: ApplyAs,
        commits: usize,
    },
    /// Skip what an operation stopped at, and carry on.
    Skip(Operation),
    /// Copy a commit's patch.
    CopyPatch(ObjectId),
    /// Save the commits from one up to `HEAD` as patches in `folder`.
    SavePatches {
        from: ObjectId,
        folder: PathBuf,
    },
    /// Copy uncommitted changes as a patch.
    CopyChanges(Changes),
    /// Save uncommitted changes as a patch at `path`.
    SaveChanges {
        changes: Changes,
        path: PathBuf,
    },
    /// Add the remote `name` at `url`; `fetch`: fetch from it once added.
    AddRemote {
        name: String,
        url: String,
        fetch: bool,
    },
    /// Fetch from `remote` alone; `prune`: its branches gone there go here too.
    FetchRemote {
        remote: BString,
        prune: bool,
    },
    /// Rename the remote `old` to `new` (its branches, and the branches following it, along).
    RenameRemote {
        old: BString,
        new: String,
    },
    /// Remove a remote, with its branches.
    RemoveRemote(BString),
    /// Point `remote` at `url` to fetch from, and `push` to push to (`None`: where it fetches
    /// from).
    SetRemoteUrls {
        remote: BString,
        url: String,
        push: Option<String>,
    },
    /// Remove the worktree at `path`; `force`: its changes with it.
    RemoveWorktree {
        path: PathBuf,
        force: bool,
    },
    /// Lock the worktree at `path`, saying why (or not).
    LockWorktree {
        path: PathBuf,
        reason: String,
    },
    /// Unlock the worktree at `path`.
    UnlockWorktree(PathBuf),
    /// Forget the worktrees whose folders are gone.
    PruneWorktrees,
    /// Bring the submodules at these paths (all, when none) to their recorded commits, setting
    /// up and cloning those not here yet.
    UpdateSubmodules(Vec<BString>),
    /// Take an action back.
    Undo(Box<Undo>),
    /// Do an action taken back again.
    Redo(Box<Undo>),
}

/// A patch's bytes, shared (never copied with the task), shown by their length.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Patch(pub(crate) Arc<[u8]>);

impl std::fmt::Debug for Patch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Patch({} bytes)", self.0.len())
    }
}

/// Where a branch that isn't checked out fast-forwards from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ForwardSource {
    /// A remote's branch (its name there): pulling, without a merge.
    Remote { remote: BString, branch: BString },
    /// A ref here (its full name).
    Here(BString),
}

/// What a new work tree checks out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeSource {
    /// A local branch (its short name).
    Branch(BString),
    /// A new branch at a ref or commit (its full name, or the commit's); `track`: a remote's
    /// branch it follows.
    NewBranch {
        name: BString,
        at: BString,
        track: bool,
    },
    /// A commit, without a branch.
    Detached(ObjectId),
}

/// A pull request to open, and the repository it goes into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PullRequestToOpen {
    pub(crate) repository: ForgeRepository,
    pub(crate) new: NewPullRequest,
}

impl Task {
    /// The hooks git may run for it (the repository's own among them run: see `output`).
    pub(crate) const fn hooks(&self) -> &'static [&'static str] {
        match self {
            Self::Push(_)
            | Self::PushTag { .. }
            | Self::DeleteRemoteBranch { .. }
            | Self::DeleteBranchEverywhere { .. } => &["pre-push", "reference-transaction"],
            Self::Pull(_) | Self::PullAndRemember { .. } | Self::Continue(_) => &[
                "pre-merge-commit",
                "prepare-commit-msg",
                "commit-msg",
                "post-merge",
                "pre-rebase",
                "post-checkout",
                "post-rewrite",
                "reference-transaction",
            ],
            Self::Merge { .. } => &[
                "pre-merge-commit",
                "prepare-commit-msg",
                "commit-msg",
                "post-merge",
                "reference-transaction",
            ],
            Self::FastForward { .. } => &["post-merge", "reference-transaction"],
            Self::Rebase { .. }
            | Self::InteractiveRebase { .. }
            | Self::DropCommit(_)
            | Self::SquashIntoParent(_)
            | Self::Reword { .. } => &[
                "pre-rebase",
                "pre-commit",
                "prepare-commit-msg",
                "commit-msg",
                "post-commit",
                "post-checkout",
                "post-rewrite",
                "reference-transaction",
            ],
            Self::CherryPick(_) | Self::Revert(_) => &[
                "prepare-commit-msg",
                "commit-msg",
                "post-commit",
                "reference-transaction",
            ],
            Self::Switch(_)
            | Self::StashAndSwitch(_)
            | Self::CheckOutPullRequest { .. }
            | Self::AddWorktree { .. } => &["post-checkout", "reference-transaction"],
            Self::ApplyPatch { .. } | Self::Skip(_) => &[
                "applypatch-msg",
                "pre-applypatch",
                "post-applypatch",
                "reference-transaction",
            ],
            _ => &["reference-transaction"],
        }
    }

    /// What it is doing, for people ("Pulling…").
    pub(crate) fn doing(&self) -> &'static str {
        match self {
            Self::Fetch | Self::FetchRemote { .. } => "Fetching",
            Self::FastForward { .. }
            | Self::FastForwardBranch {
                from: ForwardSource::Here(_),
                ..
            } => "Fast-forwarding",
            Self::Pull(_) | Self::PullAndRemember { .. } | Self::FastForwardBranch { .. } => {
                "Pulling"
            }
            Self::Push(_) | Self::PushTag { .. } => "Pushing",
            Self::Abort(_) => "Aborting",
            Self::Continue(_) => "Continuing",
            Self::Switch(_) => "Switching",
            Self::StashAndSwitch(_) | Self::Stash { .. } => "Stashing",
            Self::CreateBranch { .. } => "Making the branch",
            Self::RenameBranch { .. } => "Renaming",
            Self::DeleteBranch { .. }
            | Self::DeleteRemoteBranch { .. }
            | Self::DeleteTag(_)
            | Self::DeleteBranchEverywhere { .. } => "Deleting",
            Self::Merge { .. } => "Merging",
            Self::Rebase { .. } | Self::InteractiveRebase { .. } => "Rebasing",
            Self::DropCommit(_) => "Dropping the commit",
            Self::Bisect { .. } => "Bisecting",
            Self::SquashIntoParent(_) => "Squashing",
            Self::Reword { .. } => "Rewording",
            Self::SignCommits { .. } => "Signing the commits",
            Self::Discard(_) => "Discarding",
            Self::CherryPick(_) => "Cherry-picking",
            Self::Revert(_) => "Reverting",
            Self::Reset(..) => "Resetting",
            Self::CreateTag { .. } => "Tagging",
            Self::ApplyStash { pop: false, .. } => "Applying the stash",
            Self::ApplyStash { pop: true, .. } => "Popping the stash",
            Self::DropStash(_) => "Dropping the stash",
            Self::CheckOutPullRequest {
                kind: ForgeKind::GitHub,
                ..
            } => "Checking out the pull request",
            Self::CheckOutPullRequest {
                kind: ForgeKind::GitLab,
                ..
            } => "Checking out the merge request",
            Self::CreatePullRequest(open) => match open.repository.host().kind() {
                ForgeKind::GitHub => "Opening the pull request",
                ForgeKind::GitLab => "Opening the merge request",
            },
            Self::SetUpstream { .. } => "Setting the upstream",
            Self::AddWorktree { .. } => "Making the worktree",
            Self::SavePatch { .. } | Self::SaveChanges { .. } => "Saving the patch",
            Self::ApplyPatch { .. } => "Applying the patch",
            Self::Skip(_) => "Skipping",
            Self::CopyPatch(_) | Self::CopyChanges(_) => "Making the patch",
            Self::SavePatches { .. } => "Saving the patches",
            Self::AddRemote { .. } => "Adding the remote",
            Self::RenameRemote { .. } => "Renaming the remote",
            Self::RemoveRemote(_) => "Removing the remote",
            Self::SetRemoteUrls { .. } => "Changing the remote's address",
            Self::RemoveWorktree { .. } => "Removing the worktree",
            Self::LockWorktree { .. } => "Locking the worktree",
            Self::UnlockWorktree(_) => "Unlocking the worktree",
            Self::PruneWorktrees => "Pruning the worktrees",
            Self::UpdateSubmodules(_) => "Updating the submodules",
            Self::Undo(_) => "Undoing",
            Self::Redo(_) => "Redoing",
            Self::Lfs(lfs) => lfs.doing(),
            Self::Submodules(task) => task.doing(),
        }
    }
}

/// How a task ended, when git didn't fail outright.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The user stopped it (git cleaned up after itself).
    Cancelled,
    Fetched,
    Pulled(Pulled),
    Pushed(Pushed),
    Aborted,
    Continued,
    /// Done, as the task says.
    Done,
    /// Commits brought in (or a stash applied): done, stopped at conflicts, or empty.
    Integrated(Integrated),
    /// Stashed, if there was anything to stash.
    Stashed(bool),
    /// A switch refused: these changes in the work tree are in the way.
    WouldOverwrite {
        paths: Vec<BString>,
        untracked: bool,
    },
    /// A branch not deleted: it isn't merged.
    NotMerged,
    /// A pull request opened.
    Opened(Opened),
    /// A fast-forward: done, there already, or refused as it would take a merge.
    Forwarded(Forwarded),
    /// A patch made, to copy (its text).
    Copied(String),
    /// Patches saved: how many.
    SavedPatches(usize),
    /// A worktree not removed: it has changes, which removing it would lose.
    WorktreeChanges,
    /// Commits signed: how many were made again, how many of them were asked for, and whether
    /// any had been pushed.
    Signed {
        made: usize,
        asked: usize,
        pushed: bool,
    },
    /// Changes discarded: each file as it was and as it is now, kept for Undo (`None`: too
    /// many, or too large, to keep).
    Discarded(Option<Vec<egit_git::DiscardedFile>>),
}

/// What background fetches brought the current branch's upstream, since the user last looked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Brought {
    /// The upstream, by its full name (`refs/remotes/origin/main`).
    pub(crate) upstream: BString,
    /// Where it was before the first of these fetches, and is now.
    pub(crate) from: ObjectId,
    pub(crate) to: ObjectId,
    /// How many commits it gained.
    pub(crate) commits: u64,
}

impl Brought {
    /// The upstream's short name (`origin/main`).
    pub(crate) fn short_name(&self) -> &BStr {
        self.upstream
            .strip_prefix(b"refs/remotes/")
            .unwrap_or(&self.upstream)
            .as_bstr()
    }

    /// `self`, then `later` (a fetch after): one notice, counting both, when `later` starts
    /// where `self` ends; else `later` alone.
    fn then(self, later: Self) -> Self {
        let continues = later.from == self.to;
        if later.upstream == self.upstream && continues {
            Self {
                to: later.to,
                commits: self.commits.saturating_add(later.commits),
                ..self
            }
        } else {
            later
        }
    }
}

/// A repository's exchanges with its remotes.
#[derive(Debug)]
pub(crate) struct Tasks {
    /// The last task the user asked for, while it runs and after.
    pub(crate) task: Option<Task>,
    /// How many tasks were started (a view notices a new one's outcome by it).
    pub(crate) started: u64,
    pub(crate) result: Bind<Outcome, Problem>,
    /// How far the running task (or background fetch) has got.
    pub(crate) progress: Progress,
    cancel: CancelToken,
    /// Whether a task's end hasn't been taken in yet.
    open: bool,
    background: Bind<Option<Brought>, Problem>,
    background_cancel: CancelToken,
    next_background: Instant,
    /// How often the background fetch runs (`None`: never).
    background_every: Option<Duration>,
    /// Why the last background fetch failed, if it did.
    pub(crate) background_problem: Option<Problem>,
    /// How many background fetches ended (what follows the remotes reads again when it grows).
    pub(crate) background_fetches: u64,
    /// What they brought the current branch's upstream, until the user looks or dismisses it.
    pub(crate) brought: Option<Brought>,
}

impl Tasks {
    /// A repository's exchanges, opened at `now`.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            task: None,
            started: 0,
            result: Bind::new(true),
            progress: Progress::new(),
            cancel: CancelToken::new(),
            open: false,
            background: Bind::new(true),
            background_cancel: CancelToken::new(),
            next_background: now.checked_add(FIRST_BACKGROUND_FETCH).unwrap_or(now),
            background_every: Some(BACKGROUND_FETCH),
            background_problem: None,
            background_fetches: 0,
            brought: None,
        }
    }

    /// Runs the background fetch every `every` from now on (`None`: never); a shorter wait
    /// than the one under way takes over at once.
    pub(crate) fn fetch_every(&mut self, every: Option<Duration>, now: Instant) {
        if every == self.background_every {
            return;
        }
        if let Some(every) = every {
            let soon = now.checked_add(every).unwrap_or(now);
            if self.background_every.is_none() || soon < self.next_background {
                self.next_background = soon;
            }
        }
        self.background_every = every;
    }

    /// How often the background fetch runs (`None`: never).
    #[cfg(test)]
    pub(crate) const fn fetches_every(&self) -> Option<Duration> {
        self.background_every
    }

    /// Holds the background fetch off for an hour (tests that aren't about it: it would read
    /// the history again under them).
    #[cfg(test)]
    pub(crate) fn hold_background_fetch(&mut self) {
        let now = Instant::now();
        self.next_background = now.checked_add(Duration::from_secs(3600)).unwrap_or(now);
    }

    /// Runs the background fetch at the next frame (tests about it).
    #[cfg(test)]
    pub(crate) fn fetch_in_background_now(&mut self) {
        self.next_background = Instant::now();
    }

    /// Whether a task the user asked for is running.
    pub(crate) fn is_busy(&mut self) -> bool {
        self.result.is_pending()
    }

    /// Starts `task` (stopping the background fetch first); a forge's task signs in with
    /// `accounts`; how to take it back goes to `journal`. Returns what stops it.
    pub(crate) fn start(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        writes: &WriteLock,
        accounts: &[Arc<Account>],
        journal: Arc<Mutex<Vec<Landed>>>,
        task: Task,
    ) -> CancelToken {
        self.background_cancel.cancel();
        // Whatever the user does next, they have seen what the fetches brought.
        self.brought = None;
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.progress = Progress::new();
        let (git, repository, writes) = (git.clone(), Arc::clone(repository), writes.clone());
        let (progress, cancel) = (self.progress.clone(), self.cancel.clone());
        let running = task.clone();
        let accounts = accounts.to_vec();
        self.result.request(jobs::blocking(move || {
            let _queued = writes.hold();
            match &running {
                Task::Undo(undo) | Task::Redo(undo) => {
                    let redo = matches!(running, Task::Redo(_));
                    let done = if redo {
                        undo::do_again(&git, &repository, undo, &cancel).map(Landed::Redone)
                    } else {
                        undo::take_back(&git, &repository, undo, &cancel).map(Landed::Undone)
                    };
                    match done {
                        Ok(landed) => {
                            journal.lock().push(landed);
                            Ok(Outcome::Done)
                        }
                        Err(stopped) => {
                            // Nothing changed: it can be tried again.
                            if stopped.again {
                                let undo = Undo::clone(undo);
                                journal.lock().push(Landed::Kept { undo, redo });
                            }
                            Err(stopped.problem)
                        }
                    }
                }
                task => {
                    let before = undo::before(&git, &repository, task);
                    let ran = run(&git, &repository, task, &progress, &cancel, &accounts);
                    if let Ok(outcome) = &ran
                        && let Some(noted) = undo::noted(&git, &repository, task, before, outcome)
                    {
                        journal.lock().push(Landed::Done(noted));
                    }
                    ran
                }
            }
        }));
        self.task = Some(task);
        self.started = self.started.wrapping_add(1);
        self.open = true;
        self.cancel.clone()
    }

    /// Stops the running task (git removes what it was doing, as on the command line).
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Takes in a finished task (returning whether one finished: the work tree and status are
    /// then read again), and starts the background fetch when due (only with remotes to fetch
    /// from); returns how long until it is.
    pub(crate) fn poll(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        has_remotes: bool,
        now: Instant,
    ) -> (bool, Duration) {
        let finished = self.open && !self.result.is_pending();
        if finished {
            self.open = false;
        }
        if self.background.is_finished() {
            self.background_problem = self.background.err_ref().cloned();
            if let Some(Ok(Some(brought))) = self.background.take() {
                self.brought = Some(match self.brought.take() {
                    Some(earlier) => earlier.then(brought),
                    None => brought,
                });
            }
            self.background = Bind::new(true);
            self.background_fetches = self.background_fetches.wrapping_add(1);
        }
        let due = self.background_every.is_some() && now >= self.next_background;
        if due && has_remotes && !self.result.is_pending() && !self.background.is_pending() {
            let every = self.background_every.unwrap_or(BACKGROUND_FETCH);
            self.next_background = now.checked_add(every).unwrap_or(now);
            self.background_cancel = CancelToken::new();
            let (git, repository) = (git.clone(), Arc::clone(repository));
            let (progress, cancel) = (Progress::new(), self.background_cancel.clone());
            self.background.request(jobs::blocking(move || {
                let options = FetchOptions {
                    all: true,
                    prune: true,
                    background: true,
                };
                let failed = |error| Problem::git("Fetching in the background failed", &error);
                // Where the current branch's upstream is, before and after: a problem reading
                // it only goes without the notice.
                let before = repository.upstream_tip(&git).ok().flatten();
                fetch(&git, &repository, options, &progress, &cancel).map_err(failed)?;
                let Some((upstream, from)) = before else {
                    return Ok(None);
                };
                let after = repository.upstream_tip(&git).ok().flatten();
                Ok(after
                    .filter(|(name, to)| *name == upstream && *to != from)
                    .and_then(|(_, to)| {
                        let commits = count_between(&git, &repository, from, to, &cancel).ok()?;
                        (commits > 0).then_some(Brought {
                            upstream,
                            from,
                            to,
                            commits,
                        })
                    }))
            }));
        }
        let wait = if self.background_every.is_some() {
            self.next_background.saturating_duration_since(now)
        } else {
            NO_FETCH_DUE
        };
        (finished, wait)
    }
}

impl Drop for Tasks {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.background_cancel.cancel();
    }
}

/// Runs `task`.
fn run(
    git: &Git,
    repository: &Repository,
    task: &Task,
    progress: &Progress,
    cancel: &CancelToken,
    accounts: &[Arc<Account>],
) -> Result<Outcome, Problem> {
    match task {
        // `start` takes them back and does them again itself.
        Task::Undo(_) | Task::Redo(_) => Ok(Outcome::Done),
        Task::CheckOutPullRequest { .. } | Task::CreatePullRequest(_) => {
            forge(git, repository, task, progress, cancel, accounts)
        }
        Task::Lfs(lfs) => crate::lfs::run(git, repository, lfs, progress, cancel),
        Task::Submodules(task) => crate::submodules::run(git, repository, task, progress, cancel),
        Task::Fetch
        | Task::Pull(_)
        | Task::PullAndRemember { .. }
        | Task::Push(_)
        | Task::PushTag { .. }
        | Task::DeleteRemoteBranch { .. }
        | Task::FastForwardBranch { .. }
        | Task::DeleteBranchEverywhere { .. } => remote(git, repository, task, progress, cancel),
        Task::Abort(_)
        | Task::Continue(_)
        | Task::Merge { .. }
        | Task::Rebase { .. }
        | Task::CherryPick(_)
        | Task::Revert(_)
        | Task::Reset(..)
        | Task::FastForward { .. }
        | Task::ApplyPatch { .. }
        | Task::Skip(_)
        | Task::Bisect { .. } => history(git, repository, task, cancel),
        Task::InteractiveRebase { .. }
        | Task::DropCommit(_)
        | Task::SquashIntoParent(_)
        | Task::Reword { .. } => rewrite(git, repository, task, cancel),
        Task::SignCommits { commits } => sign(git, repository, commits, progress, cancel),
        Task::Discard(discard) => crate::discarding::run(git, repository, discard, cancel),
        Task::Switch(_)
        | Task::StashAndSwitch(_)
        | Task::CreateBranch { .. }
        | Task::RenameBranch { .. }
        | Task::DeleteBranch { .. }
        | Task::CreateTag { .. }
        | Task::DeleteTag(_)
        | Task::SetUpstream { .. } => references(git, repository, task, cancel),
        Task::AddWorktree { .. }
        | Task::SavePatch { .. }
        | Task::CopyPatch(_)
        | Task::SavePatches { .. }
        | Task::CopyChanges(_)
        | Task::SaveChanges { .. } => places(git, repository, task, cancel),
        Task::Stash { .. } | Task::ApplyStash { .. } | Task::DropStash(_) => {
            stashing(git, repository, task, cancel)
        }
        Task::AddRemote { .. }
        | Task::FetchRemote { .. }
        | Task::RenameRemote { .. }
        | Task::RemoveRemote(_)
        | Task::SetRemoteUrls { .. } => remotes(git, repository, task, progress, cancel),
        Task::RemoveWorktree { .. }
        | Task::LockWorktree { .. }
        | Task::UnlockWorktree(_)
        | Task::PruneWorktrees
        | Task::UpdateSubmodules(_) => trees(git, repository, task, progress, cancel),
    }
}

/// What a failure is: a cancelled task isn't one (the user asked for it).
fn failed(title: &'static str) -> impl Fn(GitError) -> Result<Outcome, Problem> {
    move |error| match error {
        GitError::Cancelled { .. } => Ok(Outcome::Cancelled),
        error => Err(Problem::git(title, &error)),
    }
}

/// Pull requests: checking one out, or opening one.
fn forge(
    git: &Git,
    repository: &Repository,
    task: &Task,
    progress: &Progress,
    cancel: &CancelToken,
    accounts: &[Arc<Account>],
) -> Result<Outcome, Problem> {
    match task {
        Task::CheckOutPullRequest {
            remote,
            refspec,
            kind,
            number,
            to,
        } => {
            let named = format!("{} {}", request_noun(*kind), request_number(*kind, *number));
            let fetched = fetch_refspecs(
                git,
                repository,
                remote.as_ref(),
                std::slice::from_ref(refspec),
                &format!("Fetching {named}"),
                progress,
                cancel,
            )
            .map(|()| Outcome::Done)
            .or_else(failed(match kind {
                ForgeKind::GitHub => "The pull request couldn't be fetched",
                ForgeKind::GitLab => "The merge request couldn't be fetched",
            }))?;
            if fetched == Outcome::Cancelled {
                return Ok(fetched);
            }
            match switch(git, repository, to, cancel) {
                Ok(()) => Ok(Outcome::Done),
                Err(SwitchError::WouldOverwrite { paths, untracked }) => {
                    Ok(Outcome::WouldOverwrite { paths, untracked })
                }
                Err(SwitchError::Git(error)) => failed("Switching failed")(error),
            }
        }
        Task::CreatePullRequest(open) => {
            let PullRequestToOpen {
                repository: on,
                new,
            } = &**open;
            let title = match on.host().kind() {
                ForgeKind::GitHub => "The pull request wasn't opened",
                ForgeKind::GitLab => "The merge request wasn't opened",
            };
            let account = accounts
                .iter()
                .find(|account| account.host() == on.host())
                .ok_or_else(|| {
                    Problem::new(
                        title,
                        format!("{} isn't one of this repository's remotes.", on.full_name()),
                    )
                })?;
            create_pull_request(account, on, new, cancel)
                .map(Outcome::Opened)
                .map_err(|error| Problem::new(title, error.to_string()))
        }
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Exchanges with a remote.
fn remote(
    git: &Git,
    repository: &Repository,
    task: &Task,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    match task {
        Task::Fetch => {
            let options = FetchOptions {
                all: true,
                prune: true,
                background: false,
            };
            fetch(git, repository, options, progress, cancel)
                .map(|()| Outcome::Fetched)
                .or_else(failed("Fetching failed"))
        }
        Task::Pull(mode) => pull(git, repository, *mode, progress, cancel)
            .map(Outcome::Pulled)
            .or_else(failed("Pulling failed")),
        Task::PullAndRemember { rebase } => {
            let value = if *rebase { "true" } else { "false" };
            set_config(
                git,
                ConfigTarget::Repository(repository.work_tree()),
                "pull.rebase",
                value,
            )
            .map_err(|error| Problem::git("The choice couldn't be remembered", &error))?;
            let mode = if *rebase {
                PullMode::Rebase
            } else {
                PullMode::Merge
            };
            pull(git, repository, mode, progress, cancel)
                .map(Outcome::Pulled)
                .or_else(failed("Pulling failed"))
        }
        Task::Push(options) => push(git, repository, options, progress, cancel)
            .map(Outcome::Pushed)
            .or_else(failed("Pushing failed")),
        Task::PushTag { remote, tag } => push_tag(
            git,
            repository,
            remote.as_ref(),
            tag.as_ref(),
            progress,
            cancel,
        )
        .map(Outcome::Pushed)
        .or_else(failed("The tag couldn't be pushed")),
        Task::DeleteRemoteBranch { remote, branch } => {
            let (remote, branch) = (remote.as_ref(), branch.as_ref());
            delete_remote_branch(git, repository, remote, branch, progress, cancel)
                .map(Outcome::Pushed)
                .or_else(failed("The branch couldn't be deleted on the remote"))
        }
        Task::FastForwardBranch { branch, from, .. } => {
            let from = match from {
                ForwardSource::Remote { remote, branch } => ForwardFrom::Remote {
                    remote: remote.as_ref(),
                    branch: branch.as_ref(),
                },
                ForwardSource::Here(name) => ForwardFrom::Here(name.as_ref()),
            };
            fast_forward_branch(git, repository, branch.as_ref(), from, progress, cancel)
                .map(Outcome::Forwarded)
                .or_else(failed("The branch couldn't be fast-forwarded"))
        }
        Task::DeleteBranchEverywhere {
            name,
            remote,
            remote_branch,
            force,
        } => {
            match delete_branch(git, repository, name.as_ref(), *force, cancel) {
                Ok(()) => {}
                Err(DeleteBranchError::NotMerged) => return Ok(Outcome::NotMerged),
                Err(DeleteBranchError::Git(error)) => {
                    return failed("The branch couldn't be deleted")(error);
                }
            }
            let (remote, branch) = (remote.as_ref(), remote_branch.as_ref());
            delete_remote_branch(git, repository, remote, branch, progress, cancel)
                .map(|pushed| {
                    if pushed.succeeded() {
                        Outcome::Done
                    } else {
                        Outcome::Pushed(pushed)
                    }
                })
                .or_else(failed("The branch was deleted here, but not on the remote"))
        }
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Changes to the current branch's history, and an operation's end.
fn history(
    git: &Git,
    repository: &Repository,
    task: &Task,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    match task {
        Task::Abort(operation) => abort_operation(git, repository, *operation, cancel)
            .map(|()| Outcome::Aborted)
            .or_else(failed("It couldn't be aborted")),
        Task::Continue(operation) => continue_operation(git, repository, *operation, cancel)
            .map(|()| Outcome::Continued)
            .or_else(failed("It couldn't carry on")),
        Task::Merge {
            what, fast_forward, ..
        } => merge(git, repository, what.as_ref(), *fast_forward, cancel)
            .map(Outcome::Integrated)
            .or_else(failed("Merging failed")),
        Task::Rebase {
            onto, keep_merges, ..
        } => rebase(git, repository, onto.as_ref(), *keep_merges, cancel)
            .map(Outcome::Integrated)
            .or_else(failed("Rebasing failed")),
        Task::Bisect { mark, commit } => bisect_mark(git, repository, *mark, *commit, cancel)
            .map(|()| Outcome::Done)
            .or_else(failed("The commit wasn't marked")),
        Task::CherryPick(commit) => cherry_pick(git, repository, *commit, cancel)
            .map(Outcome::Integrated)
            .or_else(failed("Cherry-picking failed")),
        Task::Revert(commit) => revert(git, repository, *commit, cancel)
            .map(Outcome::Integrated)
            .or_else(failed("Reverting failed")),
        Task::Reset(commit, mode) => reset(git, repository, *commit, *mode, cancel)
            .map(|()| Outcome::Done)
            .or_else(failed("Resetting failed")),
        Task::FastForward { to, .. } => fast_forward_current(git, repository, to.as_ref(), cancel)
            .map(Outcome::Forwarded)
            .or_else(failed("The branch couldn't be fast-forwarded")),
        Task::ApplyPatch { patch, how, .. } => apply_patch(git, repository, &patch.0, *how, cancel)
            .map(Outcome::Integrated)
            .or_else(failed("The patch couldn't be applied")),
        Task::Skip(operation) => skip_operation(git, repository, *operation, cancel)
            .map(Outcome::Integrated)
            .or_else(failed("It couldn't be skipped")),
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// The current branch's history rewritten: interactively, or one commit dropped or squashed.
fn rewrite(
    git: &Git,
    repository: &Repository,
    task: &Task,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    const TITLE: &str = "Rebasing failed";
    let range =
        |from| rebase_range(git, repository, from, cancel).map_err(|error| range_problem(&error));
    let (base, plan, keep_merges) = match task {
        Task::InteractiveRebase {
            base,
            plan,
            keep_merges,
        } => (*base, plan.clone(), *keep_merges),
        Task::DropCommit(commit) => {
            let range = range(*commit)?;
            (
                range.base,
                planned(&range, |id| (id == *commit).then_some(Step::Drop)),
                range.merges() > 0,
            )
        }
        Task::SquashIntoParent(commit) => {
            let parent = range(*commit)?.base.ok_or_else(|| {
                Problem::new(
                    TITLE,
                    "It is the first commit: there is nothing to squash it into.",
                )
            })?;
            let range = range(parent)?;
            (
                range.base,
                planned(&range, |id| (id == *commit).then_some(Step::Squash)),
                range.merges() > 0,
            )
        }
        Task::Reword { commit, message } => {
            // `HEAD`'s own: its message amended, nothing else (a rebase would refuse while
            // files are changed).
            let head = repository
                .head_ref(git)
                .map_err(|error| Problem::git("HEAD couldn't be read", &error))?;
            if head.commit == Some(*commit) {
                return reword_head(git, repository, message, cancel)
                    .map(|_| Outcome::Done)
                    .or_else(failed("The message couldn't be changed"));
            }
            let range = range(*commit)?;
            (
                range.base,
                planned(&range, |id| {
                    (id == *commit).then(|| Step::Reword(message.clone()))
                }),
                range.merges() > 0,
            )
        }
        // `run` sends the others elsewhere.
        _ => return Ok(Outcome::Done),
    };
    let rebased = if keep_merges {
        rebase_keeping_merges(git, repository, base, &plan, cancel)
    } else {
        rebase_interactive(git, repository, base, &plan, cancel)
    };
    match rebased {
        Ok(integrated) => Ok(Outcome::Integrated(integrated)),
        Err(RebaseError::Git(error)) => failed(TITLE)(error),
        Err(error) => Err(Problem::new(TITLE, format!("The plan won't do: {error}."))),
    }
}

/// Signs `commits` of the current branch, every commit after them made again and signed too,
/// how far it got in `progress`.
fn sign(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let plan = match plan_signing(git, repository, commits, cancel) {
        Ok(plan) => plan,
        Err(SignError::Git(GitError::Cancelled { .. })) => return Ok(Outcome::Cancelled),
        Err(error) => return Err(sign_problem(&error)),
    };
    let total = u64::try_from(plan.commits.len()).unwrap_or(u64::MAX);
    let signed = sign_commits(git, repository, &plan, cancel, |made| {
        progress.step(
            "Signing commits",
            u64::try_from(made).unwrap_or(u64::MAX),
            total,
        );
    });
    match signed {
        Ok(signed) => Ok(Outcome::Signed {
            made: signed.made,
            asked: plan.asked,
            pushed: plan.pushed,
        }),
        Err(SignError::Git(GitError::Cancelled { .. })) => Ok(Outcome::Cancelled),
        Err(error) => Err(sign_problem(&error)),
    }
}

/// Why commits couldn't be signed, for people.
fn sign_problem(error: &SignError) -> Problem {
    const TITLE: &str = "The commits couldn't be signed";
    let said = match error {
        SignError::Nothing => "No commit was asked for.".to_owned(),
        SignError::Detached => "HEAD isn't on a branch: switch to the branch first.".to_owned(),
        SignError::NotInBranch => "Not every one is in the current branch's history: only \
                                   its own commits can be signed."
            .to_owned(),
        SignError::TooMany => format!("It would make more than {MAX_SIGNED} commits again."),
        SignError::Unreadable(id) => format!("Commit {id} couldn't be read."),
        SignError::Moved => "The branch moved while they were being signed: nothing changed. \
                             Try again."
            .to_owned(),
        SignError::Git(error) => {
            return Problem::git(TITLE, error);
        }
    };
    Problem::new(TITLE, said)
}

/// `range`'s commits, picked unless `step` says otherwise.
fn planned(range: &RebaseRange, step: impl Fn(ObjectId) -> Option<Step>) -> Vec<Planned> {
    range
        .commits
        .iter()
        .map(|commit| Planned {
            id: commit.id,
            step: step(commit.id).unwrap_or(Step::Pick),
        })
        .collect()
}

/// Why a range can't be rewritten, for people.
pub(crate) fn range_problem(error: &RangeError) -> Problem {
    const TITLE: &str = "This can't be rewritten here";
    match error {
        RangeError::NotInBranch => Problem::new(
            TITLE,
            "The commit isn't in the current branch: only its own commits can be rewritten.",
        ),
        RangeError::TooMany => Problem::new(
            TITLE,
            format!("It would rewrite more than {MAX_PLANNED} commits."),
        ),
        RangeError::Git(error) => Problem::git("The commits couldn't be read", error),
    }
}

/// Switching, and changes to branches and tags.
fn references(
    git: &Git,
    repository: &Repository,
    task: &Task,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let switched = |to: &SwitchTo| match switch(git, repository, to, cancel) {
        Ok(()) => Ok(Outcome::Done),
        Err(SwitchError::WouldOverwrite { paths, untracked }) => {
            Ok(Outcome::WouldOverwrite { paths, untracked })
        }
        Err(SwitchError::Git(error)) => failed("Switching failed")(error),
    };
    match task {
        Task::Switch(to) => switched(to),
        Task::StashAndSwitch(to) => {
            let stashed = stash_push(git, repository, "Before switching", true, cancel)
                .map(Outcome::Stashed)
                .or_else(failed("Your changes couldn't be stashed"))?;
            if stashed == Outcome::Cancelled {
                return Ok(stashed);
            }
            switched(to).map_err(|problem| {
                Problem::new(
                    problem.title.clone(),
                    format!(
                        "{} Your changes are in the stash list (the latest stash): apply it \
                         once you are ready.",
                        problem.detail
                    ),
                )
            })
        }
        Task::CreateBranch { name, at } => {
            create_branch(git, repository, name.as_ref(), *at, cancel)
                .map(|()| Outcome::Done)
                .or_else(failed("The branch couldn't be made"))
        }
        Task::RenameBranch { old, new } => {
            rename_branch(git, repository, old.as_ref(), new.as_ref(), cancel)
                .map(|()| Outcome::Done)
                .or_else(failed("The branch couldn't be renamed"))
        }
        Task::DeleteBranch { name, force } => {
            match delete_branch(git, repository, name.as_ref(), *force, cancel) {
                Ok(()) => Ok(Outcome::Done),
                Err(DeleteBranchError::NotMerged) => Ok(Outcome::NotMerged),
                Err(DeleteBranchError::Git(error)) => {
                    failed("The branch couldn't be deleted")(error)
                }
            }
        }
        Task::CreateTag { name, at, message } => create_tag(
            git,
            repository,
            name.as_ref(),
            *at,
            message.as_deref(),
            cancel,
        )
        .map(|()| Outcome::Done)
        .or_else(failed("The tag couldn't be made")),
        Task::DeleteTag(name) => delete_tag(git, repository, name.as_ref(), cancel)
            .map(|()| Outcome::Done)
            .or_else(failed("The tag couldn't be deleted")),
        Task::SetUpstream { branch, upstream } => set_upstream(
            git,
            repository,
            branch.as_ref(),
            upstream.as_ref().map(AsRef::as_ref),
            cancel,
        )
        .map(|()| Outcome::Done)
        .or_else(failed("The upstream couldn't be set")),
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Places of the repository's outside it: a new work tree, a patch saved.
fn places(
    git: &Git,
    repository: &Repository,
    task: &Task,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    match task {
        Task::AddWorktree { path, from, .. } => {
            let from = match from {
                WorktreeSource::Branch(branch) => WorktreeFrom::Branch(branch.as_ref()),
                WorktreeSource::NewBranch { name, at, track } => WorktreeFrom::NewBranch {
                    name: name.as_ref(),
                    at: at.as_ref(),
                    track: *track,
                },
                WorktreeSource::Detached(commit) => WorktreeFrom::Detached(*commit),
            };
            add_worktree(git, repository, path, from, cancel)
                .map(|()| Outcome::Done)
                .or_else(failed("The worktree couldn't be made"))
        }
        Task::SavePatch { commit, path } => {
            let patch = match format_patch(git, repository, *commit, cancel) {
                Ok(patch) => patch,
                Err(error) => return failed("The patch couldn't be made")(error),
            };
            write_patch(path, &patch)
        }
        Task::SaveChanges { changes, path } => {
            let patch = match changes_patch(git, repository, *changes, cancel) {
                Ok(patch) => patch,
                Err(error) => return failed("The patch couldn't be made")(error),
            };
            write_patch(path, &patch)
        }
        Task::CopyPatch(commit) => match format_patch(git, repository, *commit, cancel) {
            Ok(patch) => copied(patch),
            Err(error) => failed("The patch couldn't be made")(error),
        },
        Task::CopyChanges(changes) => match changes_patch(git, repository, *changes, cancel) {
            Ok(patch) => copied(patch),
            Err(error) => failed("The patch couldn't be made")(error),
        },
        Task::SavePatches { from, folder } => {
            format_patches(git, repository, *from, folder, cancel)
                .map(|written| Outcome::SavedPatches(written.len()))
                .or_else(failed("The patches couldn't be saved"))
        }
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Remotes added, fetched alone, renamed, removed, or pointed elsewhere.
fn remotes(
    git: &Git,
    repository: &Repository,
    task: &Task,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let done =
        |result: Result<(), GitError>, title| result.map(|()| Outcome::Done).or_else(failed(title));
    match task {
        Task::AddRemote { name, url, fetch } => {
            if let Err(error) = add_remote(git, repository, name, url, cancel) {
                return failed("The remote wasn't added")(error);
            }
            if !*fetch {
                return Ok(Outcome::Done);
            }
            done(
                fetch_remote(git, repository, BStr::new(name), false, progress, cancel),
                "The remote was added, but couldn't be fetched from",
            )
        }
        Task::FetchRemote { remote, prune } => done(
            fetch_remote(git, repository, remote.as_ref(), *prune, progress, cancel),
            "The fetch failed",
        ),
        Task::RenameRemote { old, new } => done(
            rename_remote(git, repository, old.as_ref(), new, cancel),
            "The remote wasn't renamed",
        ),
        Task::RemoveRemote(name) => done(
            remove_remote(git, repository, name.as_ref(), cancel),
            "The remote wasn't removed",
        ),
        Task::SetRemoteUrls { remote, url, push } => {
            let title = "The remote's address wasn't changed";
            if let Err(error) = set_remote_url(git, repository, remote.as_ref(), url, false, cancel)
            {
                return failed(title)(error);
            }
            // Pushing goes where fetching comes from, unless told otherwise.
            let pushed = push.as_ref().map_or_else(
                || {
                    unset_config(
                        git,
                        ConfigTarget::Repository(repository.work_tree()),
                        &format!("remote.{}.pushurl", remote.to_str_lossy()),
                    )
                },
                |push| set_remote_url(git, repository, remote.as_ref(), push, true, cancel),
            );
            done(pushed, title)
        }
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Worktrees removed, locked, unlocked, or pruned; submodules updated.
fn trees(
    git: &Git,
    repository: &Repository,
    task: &Task,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let done =
        |result: Result<(), GitError>, title| result.map(|()| Outcome::Done).or_else(failed(title));
    match task {
        Task::RemoveWorktree { path, force } => {
            match remove_worktree(git, repository, path, *force, cancel) {
                Ok(()) => Ok(Outcome::Done),
                Err(RemoveWorktreeError::Changes) => Ok(Outcome::WorktreeChanges),
                Err(RemoveWorktreeError::Locked) => Err(Problem::new(
                    "The worktree wasn't removed",
                    "It is locked: unlock it first.",
                )),
                Err(RemoveWorktreeError::Git(error)) => {
                    failed("The worktree wasn't removed")(error)
                }
            }
        }
        Task::LockWorktree { path, reason } => done(
            lock_worktree(git, repository, path, reason, cancel),
            "The worktree wasn't locked",
        ),
        Task::UnlockWorktree(path) => done(
            unlock_worktree(git, repository, path, cancel),
            "The worktree wasn't unlocked",
        ),
        Task::PruneWorktrees => done(
            prune_worktrees(git, repository, cancel),
            "The worktrees weren't pruned",
        ),
        Task::UpdateSubmodules(paths) => {
            let paths: Vec<&BStr> = paths.iter().map(AsRef::as_ref).collect();
            done(
                update_submodules(git, repository, &paths, progress, cancel),
                "The submodules weren't updated",
            )
        }
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Stashing, and a stash applied or dropped.
fn stashing(
    git: &Git,
    repository: &Repository,
    task: &Task,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let stale = |title: &'static str| {
        move |error| match error {
            StashError::Stale => Err(Problem::new(
                title,
                "The stash list changed since it was read (another stash was made or \
                 dropped). Look at it again.",
            )),
            StashError::Git(error) => failed(title)(error),
        }
    };
    match task {
        Task::Stash { message, untracked } => {
            stash_push(git, repository, message, *untracked, cancel)
                .map(Outcome::Stashed)
                .or_else(failed("Your changes couldn't be stashed"))
        }
        Task::ApplyStash { stash, pop } => stash_apply(git, repository, stash, *pop, cancel)
            .map(Outcome::Integrated)
            .or_else(stale("The stash couldn't be applied")),
        Task::DropStash(stash) => stash_drop(git, repository, stash, cancel)
            .map(|()| Outcome::Done)
            .or_else(stale("The stash couldn't be dropped")),
        // `run` sends the others elsewhere.
        _ => Ok(Outcome::Done),
    }
}

/// Writes a patch to `path`.
fn write_patch(path: &std::path::Path, bytes: &[u8]) -> Result<Outcome, Problem> {
    std::fs::write(path, bytes)
        .map(|()| Outcome::Done)
        .map_err(|error| {
            Problem::new(
                "The patch couldn't be saved",
                format!("{}: {error}.", path.display()),
            )
        })
}

/// A patch to copy, if it is text (a binary file's is: git encodes it).
fn copied(patch: Vec<u8>) -> Result<Outcome, Problem> {
    if patch.is_empty() {
        return Err(Problem::new(
            "There is nothing to copy",
            "No changes: the patch would be empty.",
        ));
    }
    String::from_utf8(patch).map(Outcome::Copied).map_err(|_| {
        Problem::new(
            "The patch can't be copied",
            "It holds text that isn't UTF-8, which the clipboard can't take: save it instead.",
        )
    })
}

/// The remotes a configuration names (`remote.<name>.url`), in order.
pub(crate) fn remote_names(config: &ConfigSnapshot) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in config.entries().filter_map(remote_of) {
        if !names
            .iter()
            .any(|known| known.as_bytes() == name.as_bytes())
        {
            names.push(name.to_str_lossy().into_owned());
        }
    }
    names
}

/// Whether a configuration names a remote.
pub(crate) fn has_remotes(config: &ConfigSnapshot) -> bool {
    config.entries().any(|entry| remote_of(entry).is_some())
}

/// The remote an entry gives a URL for (`remote.<name>.url`); a remote can have several.
fn remote_of(entry: ConfigEntry<'_>) -> Option<&BStr> {
    let name = entry.key.strip_prefix(b"remote.")?.strip_suffix(b".url")?;
    Some(BStr::new(name))
}

/// Where `branch` pushes to by default: its upstream's remote and branch
/// (`branch.<name>.remote` and `.merge`), when it has one.
pub(crate) fn upstream_of(config: &ConfigSnapshot, branch: &str) -> Option<(String, String)> {
    let remote = config
        .get(&format!("branch.{branch}.remote"))?
        .text()
        .to_string();
    let merge = config
        .get(&format!("branch.{branch}.merge"))?
        .text()
        .to_string();
    let remote_branch = merge.strip_prefix("refs/heads/")?.to_owned();
    // `.` is the repository itself: nothing to push to.
    (remote != ".").then_some((remote, remote_branch))
}
