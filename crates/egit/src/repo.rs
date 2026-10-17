//! An open repository: what egit knows about it, kept current by background jobs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_git::{
    CancelToken, Committed, ConfigPlace, ConfigSnapshot, Git, Head, MailboxProgress, NewCommit,
    ObjectId, Operation, Repository, Stash, create_commit, stashes, write_commit_graph,
};
use egui_async::Bind;

use crate::forge::RepoForge;
use crate::history::{HistoryState, Scope};
use crate::jobs::{self, WriteLock};
use crate::problem::Problem;
use crate::staging::{self, StageRequest};
use crate::tables::SnapshotTable;
use crate::tasks::{Task, Tasks, has_remotes};
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

/// An open repository.
pub(crate) struct RepoTab {
    pub(crate) id: RepoId,
    pub(crate) repository: Arc<Repository>,
    /// The tab's title: the work tree's folder name.
    pub(crate) title: String,
    pub(crate) status: Bind<Status, Problem>,
    pub(crate) config: Bind<Arc<ConfigSnapshot>, Problem>,
    /// The stashes, the latest first.
    pub(crate) stashes: Bind<Arc<[Stash]>, Problem>,
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
    /// Whether a commit is under way.
    committing_open: bool,
    /// How many commits egit has made here (the form clears when it grows).
    pub(crate) commits_made: u64,
    /// What the user asked of it: fetching, pulling, pushing, ….
    pub(crate) tasks: Tasks,
    /// What egit did here, to take back (and what it took back, to do again).
    pub(crate) journal: Journal,
    /// Its remotes on GitHub and GitLab, and their requests.
    pub(crate) forge: RepoForge,
    /// The configuration the forge last worked its remotes out from.
    forge_config: Option<Arc<ConfigSnapshot>>,
    /// The background fetches the forge last followed.
    forge_fetches: u64,
    pub(crate) panels: egui_tiles::Tree<RepoPanel>,
    /// Commands that write to the repository take it.
    writes: WriteLock,
    /// Notices what changes the repository outside egit.
    watching: Watching,
    /// Wakes the window when a job has news.
    waker: egui::Context,
}

impl RepoTab {
    /// A tab for `repository`, starting to read its state and to watch it (waking `ctx` when
    /// it changes).
    pub(crate) fn new(id: RepoId, repository: Repository, git: &Git, ctx: &egui::Context) -> Self {
        let title = repository.work_tree().file_name().map_or_else(
            || repository.work_tree().display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let watching = Watching::new(Instant::now(), || watch(&repository, ctx));
        let repository = Arc::new(repository);
        let mut tab = Self {
            id,
            history: HistoryState::new(git, &repository, ctx),
            history_view: HistoryView::new(id),
            work_tree: WorkTree::new(Arc::clone(&repository), ctx),
            repository,
            title,
            status: Bind::new(true),
            config: Bind::new(true),
            stashes: Bind::new(true),
            config_table: SnapshotTable::new(format!("repo-config-{id}")),
            writing_commit_graph: Bind::new(true),
            staging: Bind::new(true),
            staging_open: false,
            committing: Bind::new(true),
            committing_open: false,
            commits_made: 0,
            tasks: Tasks::new(Instant::now()),
            journal: Journal::default(),
            forge: RepoForge::new(ctx),
            forge_config: None,
            forge_fetches: 0,
            panels: egui_tiles::Tree::new_tabs(
                egui::Id::new(("repo-panels", id)),
                vec![
                    RepoPanel::History,
                    RepoPanel::Overview,
                    RepoPanel::Configuration,
                ],
            ),
            writes: WriteLock::default(),
            watching,
            waker: ctx.clone(),
        };
        tab.refresh_status(git);
        tab.refresh_config(git);
        tab.refresh_stashes(git);
        tab
    }

    /// Takes in what its jobs finished.
    pub(crate) fn poll(&mut self) {
        self.history.poll();
        self.journal.poll();
        if matches!(self.writing_commit_graph.read_as_ref(), Some(Ok(()))) {
            self.history.shown.wrote_commit_graph();
        }
        if self.staging_open && !self.staging.is_pending() {
            self.staging_open = false;
            self.work_tree.refresh_now();
        }
        // The history and `HEAD` follow through the repository's watcher.
        if self.committing_open && !self.committing.is_pending() {
            self.committing_open = false;
            if self.committing.is_ok() {
                self.commits_made = self.commits_made.wrapping_add(1);
            }
            self.work_tree.refresh_now();
        }
    }

    /// Commits what the index holds (after the repository's other writes).
    pub(crate) fn commit(&mut self, git: &Git, commit: NewCommit) {
        let (git, repository, writes) = (
            git.clone(),
            Arc::clone(&self.repository),
            self.writes.clone(),
        );
        let journal = self.journal.inbox();
        self.committing.request(jobs::blocking(move || {
            let _queued = writes.hold();
            let head = || Place::read(&git, &repository);
            let before = head();
            let made = create_commit(&git, &repository, &commit, &CancelToken::new());
            if made.is_ok() {
                let subject = commit.message.lines().next().unwrap_or_default().trim();
                if let Some(undo) = undo::committed(before, head(), subject, commit.amend) {
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
        self.committing_open = true;
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
    pub(crate) fn follow_changes(&mut self, git: &Git, now: Instant) -> Option<Duration> {
        let (repository, waker) = (&self.repository, &self.waker);
        let taken = self.watching.take(now, || watch(repository, waker));
        let wait = match taken {
            Taken::Nothing => None,
            Taken::Wait(delay) => Some(delay),
            Taken::Changed(changes) => {
                if changes.intersects(Changes::REFS.union(Changes::STATE)) {
                    self.refresh_status(git);
                }
                if changes.intersects(Changes::REFS) {
                    self.history.reload(git, &self.repository, &self.waker);
                    // `refs/stash` is a ref.
                    self.refresh_stashes(git);
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
        if finished {
            // Refs and operations follow through the watcher too; the work tree may have
            // changed under a pull, an abort, or a continue; and the configuration under a
            // first push or a new upstream, which menus read at once.
            self.refresh_status(git);
            self.refresh_config(git);
            self.refresh_stashes(git);
            self.work_tree.refresh_now();
        }
        self.follow_forge(git, finished);
        [wait, tree_wait, Some(fetch_wait)]
            .into_iter()
            .flatten()
            .min()
    }

    /// Starts `task`.
    pub(crate) fn run_task(&mut self, git: &Git, task: Task) {
        self.tasks.start(
            git,
            &self.repository,
            &self.writes,
            self.forge.accounts(),
            self.journal.inbox(),
            task,
        );
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
            && self
                .forge_config
                .as_ref()
                .is_none_or(|seen| !Arc::ptr_eq(seen, config))
        {
            let config = Arc::clone(config);
            self.forge.configured(git, &self.repository, &config);
            self.forge_config = Some(config);
        }
        let exchanged = finished
            && matches!(
                self.tasks.task,
                Some(
                    Task::Fetch
                        | Task::Pull(_)
                        | Task::PullAndRemember { .. }
                        | Task::Push(_)
                        | Task::PushTag { .. }
                        | Task::DeleteRemoteBranch { .. }
                        | Task::CheckOutPullRequest { .. }
                        | Task::CreatePullRequest(_)
                )
            );
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
        self.refresh_stashes(git);
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
        self.writing_commit_graph.request(jobs::blocking(move || {
            let _queued = writes.hold();
            write_commit_graph(&git, &repository, &CancelToken::new())
                .map_err(|error| Problem::git("The commit-graph couldn't be written", &error))
        }));
    }

    /// Reads where `HEAD` points and what is in progress.
    /// Reads the stash list again (what is shown stays until it lands).
    pub(crate) fn refresh_stashes(&mut self, git: &Git) {
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.stashes.request(jobs::blocking(move || {
            stashes(&git, &repository, &CancelToken::new())
                .map(Into::into)
                .map_err(|error| Problem::git("The stashes couldn't be read", &error))
        }));
    }

    pub(crate) fn refresh_status(&mut self, git: &Git) {
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.status.request(jobs::blocking(move || {
            let head = repository
                .head(&git)
                .map_err(|error| Problem::git("Where HEAD points couldn't be read", &error))?;
            let unreadable = |error: std::io::Error| {
                Problem::new("The repository's state couldn't be read", error.to_string())
            };
            let operations = repository.operations().map_err(unreadable)?;
            let applying = repository.mailbox_progress().map_err(unreadable)?;
            Ok(Status {
                head,
                operations,
                applying,
            })
        }));
    }

    /// Reads the configuration in effect in the repository; signatures are checked again (the
    /// keys and signers trusted may have changed).
    pub(crate) fn refresh_config(&mut self, git: &Git) {
        self.history.signatures.forget();
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.config.request(jobs::blocking(move || {
            ConfigSnapshot::read(&git, ConfigPlace::Repository(repository.work_tree()))
                .map(Arc::new)
                .map_err(|error| Problem::git("The repository's settings couldn't be read", &error))
        }));
    }
}

/// Watches `repository`, waking `ctx` when it changes.
fn watch(repository: &Repository, ctx: &egui::Context) -> notify::Result<Watcher> {
    let waker = ctx.clone();
    Watcher::repository(repository, move |delay| waker.request_repaint_after(delay))
}
