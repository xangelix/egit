//! An open repository: what egit knows about it, kept current by background jobs.

#[cfg(target_arch = "wasm32")]
use crate::web::notify;
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use egit_git::{
    Bisection, CancelToken, Committed, ConfigPlace, ConfigSnapshot, Git, Head, Hooks,
    MailboxProgress, NewCommit, ObjectId, Operation, Repository, Stash, Submodule, Worktree,
    bisection, create_commit, installed_hooks, stashes, submodules, worktrees, write_commit_graph,
};
use egui_async::Bind;

use crate::commands::RepoCommand;
use crate::forge::RepoForge;
use crate::history::{HistoryState, Scope};
use crate::jobs::{self, WriteLock};
use crate::output::{COMMIT_HOOKS, Ended, Output};
use crate::preferences::Preferences;
use crate::problem::Problem;
use crate::staging::{self, StageRequest};
use crate::tables::SnapshotTable;
use crate::tasks::{Outcome, Task, Tasks, has_remotes};
use crate::undo::{self, Journal, Landed, Place};
use crate::views::history::HistoryView;
use crate::watch::{Changes, Taken, Watcher, Watching};
use crate::worktree::WorkTree;

/// Tells open repositories apart for the life of the window.
pub(crate) type RepoId = u64;

/// Where `HEAD` points and what the repository is in the middle of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) head: Head,
    pub(crate) operations: Vec<Operation>,
    /// How far a `git am` stopped part-way got.
    pub(crate) applying: Option<MailboxProgress>,
    /// Where a bisection under way stands.
    pub(crate) bisect: Option<Bisection>,
}

/// The parts of a repository's tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepoPanel {
    /// The commit graph, and the selected commit's details.
    History,
    /// The branch, its commit and upstream, the identity in effect, and the paths.
    Overview,
    /// Every configuration entry in effect.
    Configuration,
}

/// How changes made on another computer are followed there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FollowingThere {
    /// Told at once, refs and files alike (`inotifywait` there).
    AtOnce,
    /// The git directories told at once; the work tree's status read every few seconds
    /// (its watches there failed).
    FilesLooked,
    /// Looked for every few seconds (no `inotifywait` there).
    Looked,
}

impl FollowingThere {
    /// In words.
    pub(crate) const fn words(self) -> &'static str {
        match self {
            Self::AtOnce => "At once, as they are made there (inotifywait)",
            Self::FilesLooked => {
                "Its refs and index at once; its files' changes looked for every 5 seconds"
            }
            Self::Looked => {
                "Looked for every few seconds: install inotify-tools there to see them at once"
            }
        }
    }
}

/// An open repository.
pub(crate) struct RepoTab {
    pub(crate) id: RepoId,
    pub(crate) repository: Arc<Repository>,
    /// The git its commands run with: this computer's, or one on the computer it is on.
    pub(crate) git: Git,
    /// The tab's title: the work tree's folder name.
    pub(crate) title: String,
    pub(crate) status: Bind<Status, Problem>,
    pub(crate) config: Bind<Arc<ConfigSnapshot>, Problem>,
    /// The stashes, the latest first.
    pub(crate) stashes: Bind<Arc<[Stash]>, Problem>,
    /// The stash list the history was last read again for, so one read of the list reads the
    /// history again at most once.
    stashes_followed: Option<Arc<[Stash]>>,
    /// The worktrees, the main one first.
    pub(crate) worktrees: Bind<Arc<[Worktree]>, Problem>,
    /// The submodules, by path.
    pub(crate) submodules: Bind<Arc<[Submodule]>, Problem>,
    /// Its Git LFS: installed, set up, the patterns it tracks.
    pub(crate) lfs: Bind<Arc<crate::lfs::LfsState>, Problem>,
    /// Where its submodules stand, nested ones too.
    pub(crate) submodule_states: Bind<Arc<crate::submodules::Standing>, Problem>,
    /// The work tree's status they were last read after (its address).
    submodules_seen: Option<usize>,
    pub(crate) config_table: SnapshotTable,
    pub(crate) history: HistoryState,
    pub(crate) history_view: HistoryView,
    /// The working tree's status, kept current.
    pub(crate) work_tree: WorkTree,
    /// Writing a commit-graph file.
    pub(crate) writing_commit_graph: Bind<(), Problem>,
    /// The last staging or unstaging, while it runs and after (a problem stays shown).
    pub(crate) staging: Bind<(), Problem>,
    /// Whether a staging is under way, so its end reads the work tree again.
    staging_open: bool,
    /// The last commit made, while it is made and after (its output or problem stays shown).
    pub(crate) committing: Bind<Committed, Problem>,
    /// A commit under way, and whether the branch is pushed once it is made.
    committing_open: Option<bool>,
    /// How many commits egit has made here (the form clears when it grows).
    pub(crate) commits_made: u64,
    /// What the user asked of it: fetching, pulling, pushing, ….
    pub(crate) tasks: Tasks,
    /// What its tasks and commits printed (hooks' output among it), and the panel showing it.
    pub(crate) output: Output,
    /// Whether the output panel opens by itself while hooks run (the preferences').
    output_automatic: bool,
    /// The runs of the task and the commit under way.
    task_run: Option<u64>,
    pub(crate) commit_run: Option<u64>,
    /// What stops the commit under way.
    commit_cancel: CancelToken,
    /// The hooks it has, read with its configuration and when the window comes back.
    pub(crate) hooks: Bind<Hooks, Problem>,
    /// What egit did here, to take back (and what it took back, to do again).
    pub(crate) journal: Journal,
    /// Its remotes on GitHub and GitLab, and their requests.
    pub(crate) forge: RepoForge,
    /// The configuration the forge last worked its remotes out from.
    forge_config: Option<Arc<ConfigSnapshot>>,
    /// The background fetches the forge last followed.
    forge_fetches: u64,
    /// What the tab shows: the history, or a page about the repository.
    pub(crate) panel: RepoPanel,
    /// Commands that write to the repository take it.
    writes: WriteLock,
    /// Notices what changes the repository outside egit.
    watching: Watching,
    /// Wakes the window when a job has news.
    waker: crate::repaint::Repaint,
    /// Its agents: conversations with Claude working in its work tree.
    pub(crate) agents: crate::agents::RepoAgents,
}

impl RepoTab {
    /// A tab for `repository`, read with `git` (`here`: this computer's, when that is another
    /// computer's; `run`, what the run shares with its forge: sign-ins, ssh hosts learned),
    /// starting to read its state and to watch it (waking `ctx` when it changes).
    pub(crate) fn new(
        id: RepoId,
        repository: Repository,
        (git, here): (&Git, Option<&Git>),
        run: crate::forge::ForgeRun,
        ctx: &egui::Context,
    ) -> Self {
        let name = repository.work_tree().file_name().map_or_else(
            || repository.work_tree().display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        // On another computer, which one.
        let title = match git.host().ssh() {
            Some(ssh) => format!("{name} · {}", ssh.destination),
            None => name,
        };
        let repaint = crate::repaint::Repaint::of(ctx);
        let watching = Watching::new(Instant::now(), || watch(git, &repository, &repaint));
        let repository = Arc::new(repository);
        let mut tab = Self {
            id,
            git: git.clone(),
            history: HistoryState::new(git, &repository, &repaint),
            history_view: HistoryView::new(id),
            work_tree: WorkTree::new(Arc::clone(&repository), git.host().is_local(), ctx),
            repository,
            title,
            status: Bind::new(true),
            config: Bind::new(true),
            stashes: Bind::new(true),
            stashes_followed: None,
            worktrees: Bind::new(true),
            submodules: Bind::new(true),
            lfs: Bind::new(true),
            submodule_states: Bind::new(true),
            submodules_seen: None,
            config_table: SnapshotTable::new(format!("repo-config-{id}")),
            writing_commit_graph: Bind::new(true),
            staging: Bind::new(true),
            staging_open: false,
            committing: Bind::new(true),
            committing_open: None,
            commits_made: 0,
            tasks: Tasks::new(Instant::now()),
            output: Output::default(),
            output_automatic: true,
            task_run: None,
            commit_run: None,
            commit_cancel: CancelToken::new(),
            hooks: Bind::new(true),
            journal: Journal::default(),
            forge: RepoForge::new(&repaint, here.cloned(), run),
            forge_config: None,
            forge_fetches: 0,
            panel: RepoPanel::History,
            writes: WriteLock::default(),
            watching,
            agents: crate::agents::RepoAgents::new(&repaint),
            waker: repaint,
        };
        tab.refresh_status(git);
        tab.refresh_config(git);
        tab.refresh_lists(git);
        tab
    }

    /// Takes in what its jobs finished.
    pub(crate) fn poll(&mut self) {
        self.history.poll();
        // The stash list read anew lists other stashes than the history shows: read it again
        // (once for each list read, should the history still not show them).
        if let Some(Ok(listed)) = self.stashes.read_as_ref()
            && !self
                .stashes_followed
                .as_ref()
                .is_some_and(|followed| Arc::ptr_eq(followed, listed))
            && self.history.misses_stashes(listed)
        {
            self.stashes_followed = Some(Arc::clone(listed));
            self.history
                .reload(&self.git, &self.repository, &self.waker);
        }
        self.agents.poll();
        self.journal.poll();
        if matches!(self.writing_commit_graph.read_as_ref(), Some(Ok(()))) {
            self.history.shown.wrote_commit_graph();
        }
        if self.staging_open && !self.staging.is_pending() {
            self.staging_open = false;
            self.work_tree.refresh_now();
        }
        // The history and `HEAD` follow through the repository's watcher.
        if let Some(push) = self.committing_open
            && !self.committing.is_pending()
        {
            self.committing_open = None;
            let ended = if self.committing.is_ok() {
                self.commits_made = self.commits_made.wrapping_add(1);
                // Pushed as Push would, where it still can be (a task started meanwhile).
                if push && self.allows(&RepoCommand::Push).is_ok() {
                    self.history_view.queue(RepoCommand::Push);
                }
                Ended::Done
            } else if self.commit_cancel.is_cancelled() {
                Ended::Cancelled
            } else {
                Ended::Failed
            };
            if let Some(run) = self.commit_run.take() {
                self.output.end(run, ended);
            }
            self.work_tree.refresh_now();
        }
    }

    /// Commits what the index holds (after the repository's other writes), then pushes the
    /// branch if `push` (as Push would: its upstream, or asking where).
    pub(crate) fn commit(&mut self, git: &Git, commit: NewCommit, push: bool) {
        // Its hooks' output, as git prints it, in a run of its own.
        self.commit_cancel = CancelToken::new();
        let hooks = self.installed_hooks(&commit.checks.running(COMMIT_HOOKS));
        let title = if commit.amend {
            "Amending"
        } else {
            "Committing"
        };
        let cancel = self.commit_cancel.clone();
        let (run, transcript) = self.output.begin(title, hooks, Some(cancel.clone()));
        self.commit_run = Some(run);
        let (git, repository, writes) = (
            git.with_transcript(&transcript),
            Arc::clone(&self.repository),
            self.writes.clone(),
        );
        let journal = self.journal.inbox();
        self.committing.request(jobs::blocking(async move {
            let _queued = writes.hold().await;
            let head = || Place::read(&git, &repository);
            let before = head().await;
            let made = create_commit(&git, &repository, &commit, &cancel).await;
            if made.is_ok() {
                let subject = commit.message.lines().next().unwrap_or_default().trim();
                if let Some(undo) = undo::committed(before, head().await, subject, commit.amend) {
                    journal.lock().push(Landed::Done(undo));
                }
            }
            made.map_err(|error| {
                const TITLE: &str = "The commit wasn't made";
                // A hook's whole complaint, not only its last line.
                error.failure().map_or_else(
                    || Problem::git(TITLE, &error),
                    |failure| {
                        Problem::new(
                            TITLE,
                            String::from_utf8_lossy(failure.stderr.as_bytes()).trim(),
                        )
                    },
                )
            })
        }));
        self.committing_open = Some(push);
    }

    /// Stages or unstages as `request` says (after the repository's other writes).
    pub(crate) fn stage(&mut self, git: &Git, request: StageRequest) {
        self.staging.request(staging::run(
            git.clone(),
            Arc::clone(&self.repository),
            self.history.objects.clone(),
            self.writes.clone(),
            request,
        ));
        self.staging_open = true;
    }

    /// Reads again what changed outside egit, once the changes have settled; returns how long
    /// to wait before asking again while they haven't.
    pub(crate) fn follow_changes(&mut self, now: Instant) -> Option<Duration> {
        let git = &self.git.clone();
        let (repository, waker) = (&self.repository, &self.waker);
        let taken = self.watching.take(now, || watch(git, repository, waker));
        let wait = match taken {
            Taken::Nothing => None,
            Taken::Wait(delay) => Some(delay),
            Taken::Changed(changes) => {
                if changes.intersects(Changes::REFS.union(Changes::STATE)) {
                    self.refresh_status(git);
                }
                if changes.intersects(Changes::REFS) {
                    self.history.reload(git, &self.repository, &self.waker);
                    // `refs/stash` is a ref; a worktree's branch, or a submodule's commit
                    // (with a commit), may have moved.
                    self.refresh_lists(git);
                }
                if changes.intersects(Changes::CONFIG) {
                    self.refresh_config(git);
                }
                if changes.intersects(
                    Changes::INDEX
                        .union(Changes::REFS)
                        .union(Changes::STATE)
                        .union(Changes::WORK_TREE),
                ) {
                    self.work_tree.changed();
                }
                None
            }
        };
        let head = self.head_commit();
        let tree_wait = self.work_tree.poll(git, head, now);
        let has_remotes = self
            .config
            .ok_ref()
            .is_some_and(|config| has_remotes(config));
        let (finished, fetch_wait) = self.tasks.poll(git, &self.repository, has_remotes, now);
        if finished && let Some(run) = self.task_run.take() {
            let ended = match self.tasks.result.read_as_ref() {
                Some(Ok(Outcome::Cancelled)) => Ended::Cancelled,
                Some(Ok(outcome)) if outcome.asks() => Ended::Asked,
                Some(Ok(outcome)) if outcome.stopped() => Ended::Stopped,
                Some(Ok(_)) => Ended::Done,
                _ => Ended::Failed,
            };
            self.output.end(run, ended);
        }
        let output_wait = self.output.follow(now, self.output_automatic);
        if finished {
            // Refs and operations follow through the watcher too; the work tree may have
            // changed under a pull, an abort, or a continue; and the configuration under a
            // first push or a new upstream, which menus read at once.
            self.refresh_status(git);
            self.refresh_config(git);
            self.refresh_lists(git);
            self.work_tree.refresh_now();
        }
        self.follow_forge(git, finished);
        [wait, tree_wait, Some(fetch_wait), output_wait]
            .into_iter()
            .flatten()
            .min()
    }

    /// Starts `task`.
    pub(crate) fn run_task(&mut self, git: &Git, task: Task) {
        // Its commands' output (and its hooks'), as git prints it, in a run of its own.
        let hooks = self.installed_hooks(&self.tasks.checks().running(task.hooks()));
        let (run, transcript) = self.output.begin(task.doing(), hooks, None);
        self.task_run = Some(run);
        let cancel = self.tasks.start(
            &git.with_transcript(&transcript),
            &self.repository,
            &self.writes,
            self.forge.accounts(),
            self.journal.inbox(),
            task,
        );
        self.output.cancel_with(run, cancel);
    }

    /// Runs, or skips, the hooks that check commits, for this tab's commits and tasks from now
    /// on.
    pub(crate) const fn set_checks(&mut self, checks: egit_git::Checks) {
        self.tasks.set_checks(checks);
    }

    /// Those of `hooks` the repository has.
    fn installed_hooks(&mut self, hooks: &[&'static str]) -> Vec<&'static str> {
        self.hooks
            .ok_ref()
            .map_or_default(|installed| installed.of(hooks))
    }

    /// Reads which hooks the repository has (again).
    pub(crate) fn refresh_hooks(&mut self, git: &Git) {
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.hooks.request(jobs::blocking(async move {
            installed_hooks(&git, &repository)
                .await
                .map_err(|error| Problem::git("The repository's hooks couldn't be read", &error))
        }));
    }

    /// Carries out what was asked of its output.
    pub(crate) fn output_asked(&mut self, asked: crate::app::OutputAsk) {
        match asked {
            crate::app::OutputAsk::CancelCommit => self.commit_cancel.cancel(),
            crate::app::OutputAsk::Show => {
                self.output.open = true;
                self.output.picked = None;
            }
            crate::app::OutputAsk::Toggle => self.output.toggle(),
        }
    }

    /// Puts the user's preferences in effect for this tab: how often it fetches in the
    /// background, and whether its sidebar shows (as it opens).
    pub(crate) fn prefer(&mut self, preferences: Preferences, now: Instant) {
        self.tasks.fetch_every(preferences.fetch.interval(), now);
        self.output_automatic = preferences.output;
        self.history_view.show_sidebar(preferences.sidebar);
    }

    /// Whether `command` can run here now, and why not (for the palette).
    pub(crate) fn allows(&mut self, command: &RepoCommand) -> Result<(), &'static str> {
        let busy = self.tasks.is_busy();
        let has_remotes = self
            .config
            .ok_ref()
            .is_some_and(|config| crate::tasks::has_remotes(config));
        let (on_branch, has_upstream, has_commit, operation) =
            self.status
                .ok_ref()
                .map_or((false, false, false, false), |status| {
                    let (on_branch, upstream, commit) = match &status.head {
                        Head::Branch(branch) => {
                            (true, branch.upstream.is_some(), branch.commit.is_some())
                        }
                        Head::Detached(_) => (false, false, true),
                    };
                    (on_branch, upstream, commit, !status.operations.is_empty())
                });
        let has_changes = self
            .work_tree
            .status()
            .and_then(Result::ok)
            .is_some_and(|status| !status.is_empty());
        let idle = if busy {
            Err("A task is running")
        } else {
            Ok(())
        };
        match command {
            RepoCommand::Fetch if !has_remotes => Err("The repository has no remote"),
            RepoCommand::Pull if !has_upstream => Err("The branch has no upstream"),
            RepoCommand::Push if !on_branch || !has_remotes => {
                Err("Push from a branch, to a remote")
            }
            RepoCommand::Undo if self.journal.next_undo().is_none() => Err("Nothing to undo"),
            RepoCommand::Redo if self.journal.next_redo().is_none() => Err("Nothing to redo"),
            RepoCommand::Stash if !has_changes => Err("There are no changes to stash"),
            RepoCommand::CreateBranch | RepoCommand::CreateTag if !has_commit => {
                Err("There is no commit yet")
            }
            RepoCommand::OpenFolder if !self.git.host().is_local() => {
                Err("It is on another computer")
            }
            RepoCommand::Switch(_) if operation => Err("An operation is in progress"),
            RepoCommand::Pipelines if self.forge.repositories.is_empty() => {
                Err("No remote is on GitHub or GitLab")
            }
            RepoCommand::Fetch
            | RepoCommand::Pull
            | RepoCommand::Push
            | RepoCommand::Undo
            | RepoCommand::Redo
            | RepoCommand::Stash
            | RepoCommand::ApplyPatches
            | RepoCommand::PastePatch
            | RepoCommand::CreateBranch
            | RepoCommand::CreateTag
            | RepoCommand::AddRemote
            | RepoCommand::Switch(_) => idle,
            RepoCommand::Search
            | RepoCommand::FilterSidebar
            | RepoCommand::ToggleSidebar
            | RepoCommand::ToggleOutput
            | RepoCommand::ToggleAgents
            | RepoCommand::Reflog
            | RepoCommand::WorkTree
            | RepoCommand::PullRequests
            | RepoCommand::Pipelines
            | RepoCommand::GoTo(_)
            | RepoCommand::Stats
            | RepoCommand::Overview
            | RepoCommand::Settings
            | RepoCommand::GitLfs
            | RepoCommand::Submodules
            | RepoCommand::OpenFolder
            | RepoCommand::Refresh => Ok(()),
        }
    }

    /// Takes the last action back (`redo`: does the last one taken back again).
    pub(crate) fn undo(&mut self, git: &Git, redo: bool) {
        if self.tasks.is_busy() {
            return;
        }
        let taken = if redo {
            self.journal
                .take_redo()
                .map(|undo| Task::Redo(Box::new(undo)))
        } else {
            self.journal
                .take_undo()
                .map(|undo| Task::Undo(Box::new(undo)))
        };
        if let Some(task) = taken {
            self.run_task(git, task);
        }
    }

    /// Keeps the forge current: its remotes worked out again from new configuration, and its
    /// pull requests read again after anything that exchanged with a remote (`finished`: a
    /// task just ended).
    fn follow_forge(&mut self, git: &Git, finished: bool) {
        if let Some(config) = self.config.ok_ref()
            && (self
                .forge_config
                .as_ref()
                .is_none_or(|seen| !Arc::ptr_eq(seen, config))
                || self.forge.learned_more())
        {
            let config = Arc::clone(config);
            self.forge.configured(git, &self.repository, &config);
            self.forge_config = Some(config);
        }
        let exchanged = finished
            && self.tasks.task.as_ref().is_some_and(|task| {
                task.reaches_remotes()
                    || matches!(task, Task::CreatePullRequest(_) | Task::StartPipeline(_))
            });
        let fetched = self.tasks.background_fetches != self.forge_fetches;
        self.forge_fetches = self.tasks.background_fetches;
        if exchanged || fetched {
            self.forge.refresh();
        }
        self.forge.poll(git, &self.repository);
    }

    /// The commit `HEAD` points at, once read (`None` before the first commit too).
    fn head_commit(&mut self) -> Option<ObjectId> {
        match &self.status.ok_ref()?.head {
            Head::Branch(branch) => branch.commit.as_ref().map(|commit| commit.id),
            Head::Detached(commit) => Some(commit.id),
        }
    }

    /// How changes made on the other computer the repository is on are followed (`None` here,
    /// where they are watched, or while they aren't followed at all: see
    /// [`Self::watch_problem`]).
    pub(crate) fn following_there(&self) -> Option<FollowingThere> {
        let live = self.watching.is_live()?;
        Some(match (live, self.work_tree.is_live()) {
            (true, true) => FollowingThere::AtOnce,
            (true, false) => FollowingThere::FilesLooked,
            (false, _) => FollowingThere::Looked,
        })
    }

    /// Why changes made outside egit aren't being followed, while they aren't.
    pub(crate) fn watch_problem(&self) -> Option<String> {
        self.watching
            .problem()
            .map(ToOwned::to_owned)
            .or_else(|| self.work_tree.problem())
    }

    /// Reads everything again; what is shown stays until the new state arrives.
    pub(crate) fn refresh(&mut self, git: &Git) {
        self.refresh_status(git);
        self.refresh_config(git);
        self.refresh_lists(git);
        self.work_tree.refresh_now();
        self.history.reload(git, &self.repository, &self.waker);
    }

    /// Shows `scope` of the history (read again).
    pub(crate) fn set_scope(&mut self, git: &Git, scope: Scope) {
        self.history
            .set_scope(git, &self.repository, scope, &self.waker);
    }

    /// Writes a commit-graph file, so walks of the history start at once.
    pub(crate) fn write_commit_graph(&mut self, git: &Git) {
        let (git, repository, writes) = (
            git.clone(),
            Arc::clone(&self.repository),
            self.writes.clone(),
        );
        self.writing_commit_graph
            .request(jobs::blocking(async move {
                let _queued = writes.hold().await;
                write_commit_graph(&git, &repository, &CancelToken::new())
                    .await
                    .map_err(|error| Problem::git("The commit-graph couldn't be written", &error))
            }));
    }

    /// Reads the stash list, the worktrees, and the submodules again (what is shown stays until
    /// each lands).
    pub(crate) fn refresh_lists(&mut self, git: &Git) {
        let read = |repository: &Arc<Repository>| (git.clone(), Arc::clone(repository));
        let (git_, repository) = read(&self.repository);
        self.stashes.request(jobs::blocking(async move {
            stashes(&git_, &repository, &CancelToken::new())
                .await
                .map(Into::into)
                .map_err(|error| Problem::git("The stashes couldn't be read", &error))
        }));
        let (git_, repository) = read(&self.repository);
        self.worktrees.request(jobs::blocking(async move {
            worktrees(&git_, &repository)
                .await
                .map(Into::into)
                .map_err(|error| Problem::git("The worktrees couldn't be read", &error))
        }));
        let (git_, repository) = read(&self.repository);
        self.submodules.request(jobs::blocking(async move {
            submodules(&git_, &repository)
                .await
                .map(Into::into)
                .map_err(|error| Problem::git("The submodules couldn't be read", &error))
        }));
        let (git_, repository) = read(&self.repository);
        self.submodule_states
            .request(crate::submodules::read_states(git_, repository));
        let (git_, repository) = read(&self.repository);
        self.lfs.request(jobs::blocking(async move {
            crate::lfs::read_lfs(&git_, &repository, crate::lfs::Reading::WhereUsed)
                .await
                .map(Arc::new)
        }));
    }

    /// Reads where the submodules stand again whenever a new status of the work tree lands (a
    /// pointer staged, a commit made in one, a checkout in one), where there are any.
    pub(crate) fn follow_submodules(&mut self, git: &Git) {
        let status = self
            .work_tree
            .status()
            .and_then(Result::ok)
            .map(|status| Arc::as_ptr(status).addr());
        if status == self.submodules_seen {
            return;
        }
        self.submodules_seen = status;
        let any = self
            .submodule_states
            .ok_ref()
            .is_some_and(|standing| !standing.states.is_empty());
        if any && !self.submodule_states.is_pending() {
            self.submodule_states
                .request(crate::submodules::read_states(
                    git.clone(),
                    Arc::clone(&self.repository),
                ));
        }
    }

    /// Reads where `HEAD` points and what is in progress.
    pub(crate) fn refresh_status(&mut self, git: &Git) {
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.status.request(jobs::blocking(async move {
            let head = repository
                .head(&git)
                .await
                .map_err(|error| Problem::git("Where HEAD points couldn't be read", &error))?;
            let unreadable = |error: std::io::Error| {
                Problem::new("The repository's state couldn't be read", error.to_string())
            };
            let operations = repository.operations(&git).await.map_err(unreadable)?;
            let applying = repository
                .mailbox_progress(&git)
                .await
                .map_err(unreadable)?;
            let bisect = if operations.contains(&Operation::Bisect) {
                bisection(&git, &repository)
                    .await
                    .map_err(|error| Problem::git("The bisection couldn't be read", &error))?
            } else {
                None
            };
            Ok(Status {
                head,
                operations,
                applying,
                bisect,
            })
        }));
    }

    /// Reads the configuration in effect in the repository; signatures are checked again (the
    /// keys and signers trusted may have changed).
    pub(crate) fn refresh_config(&mut self, git: &Git) {
        self.history.signatures.forget();
        // `core.hooksPath` may have changed.
        self.refresh_hooks(git);
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.config.request(jobs::blocking(async move {
            ConfigSnapshot::read(&git, ConfigPlace::Repository(repository.work_tree()))
                .await
                .map(Arc::new)
                .map_err(|error| Problem::git("The repository's settings couldn't be read", &error))
        }));
    }
}

/// Watches `repository`, waking `ctx` when it changes.
fn watch(
    git: &Git,
    repository: &Repository,
    ctx: &crate::repaint::Repaint,
) -> notify::Result<Watcher> {
    let waker = ctx.clone();
    let wake = move |delay| waker.request_repaint_after(delay);
    match git.host() {
        egit_git::Host::Local => Watcher::repository(repository, wake),
        // There, a session of its own looking every few seconds.
        egit_git::Host::Ssh(_) | egit_git::Host::Relay(_) => {
            Watcher::elsewhere(git, repository, wake)
        }
    }
}
