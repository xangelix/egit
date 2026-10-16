//! An open repository: what egit knows about it, kept current by background jobs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_git::{
    CancelToken, ConfigPlace, ConfigSnapshot, Git, Head, Operation, Repository, write_commit_graph,
};
use egui_async::Bind;

use crate::history::HistoryState;
use crate::jobs::{self, WriteLock};
use crate::problem::Problem;
use crate::tables::SnapshotTable;
use crate::views::history::HistoryView;
use crate::watch::{Changes, Taken, Watcher};

/// Tells open repositories apart for the life of the window.
pub(crate) type RepoId = u64;

/// Where `HEAD` points and what the repository is in the middle of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) head: Head,
    pub(crate) operations: Vec<Operation>,
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
    pub(crate) config_table: SnapshotTable,
    pub(crate) history: HistoryState,
    pub(crate) history_view: HistoryView,
    /// Writing a commit-graph file.
    pub(crate) writing_commit_graph: Bind<(), Problem>,
    pub(crate) panels: egui_tiles::Tree<RepoPanel>,
    /// Commands that write to the repository take it.
    writes: WriteLock,
    /// Notices what changes the repository outside egit; `None` when it can't be watched.
    watcher: Option<Watcher>,
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
        let waker = ctx.clone();
        let watcher =
            Watcher::repository(&repository, move |delay| waker.request_repaint_after(delay))
                .inspect_err(|error| {
                    tracing::warn!(%error, "changes made outside egit won't show until refreshed");
                })
                .ok();
        let repository = Arc::new(repository);
        let mut tab = Self {
            id,
            history: HistoryState::new(git, &repository, ctx),
            history_view: HistoryView::new(id),
            repository,
            title,
            status: Bind::new(true),
            config: Bind::new(true),
            config_table: SnapshotTable::new(format!("repo-config-{id}")),
            writing_commit_graph: Bind::new(true),
            panels: egui_tiles::Tree::new_tabs(
                egui::Id::new(("repo-panels", id)),
                vec![
                    RepoPanel::History,
                    RepoPanel::Overview,
                    RepoPanel::Configuration,
                ],
            ),
            writes: WriteLock::default(),
            watcher,
            waker: ctx.clone(),
        };
        tab.refresh_status(git);
        tab.refresh_config(git);
        tab
    }

    /// Takes in what its jobs finished.
    pub(crate) fn poll(&mut self) {
        self.history.poll();
        if matches!(self.writing_commit_graph.read_as_ref(), Some(Ok(()))) {
            self.history.shown.wrote_commit_graph();
        }
    }

    /// Reads again what changed outside egit, once the changes have settled; returns how long
    /// to wait before asking again while they haven't.
    pub(crate) fn follow_changes(&mut self, git: &Git, now: Instant) -> Option<Duration> {
        let taken = self
            .watcher
            .as_ref()
            .map_or(Taken::Nothing, |watcher| watcher.take(now));
        match taken {
            Taken::Nothing => None,
            Taken::Wait(delay) => Some(delay),
            Taken::Changed(changes) => {
                if changes.intersects(Changes::REFS.union(Changes::STATE)) {
                    self.refresh_status(git);
                }
                if changes.intersects(Changes::REFS) {
                    self.history.reload(git, &self.repository, &self.waker);
                }
                if changes.intersects(Changes::CONFIG) {
                    self.refresh_config(git);
                }
                None
            }
        }
    }

    /// Reads everything again; what is shown stays until the new state arrives.
    pub(crate) fn refresh(&mut self, git: &Git) {
        self.refresh_status(git);
        self.refresh_config(git);
        self.history.reload(git, &self.repository, &self.waker);
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
    pub(crate) fn refresh_status(&mut self, git: &Git) {
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.status.request(jobs::blocking(move || {
            let head = repository
                .head(&git)
                .map_err(|error| Problem::git("Where HEAD points couldn't be read", &error))?;
            let operations = repository.operations().map_err(|error| {
                Problem::new("The repository's state couldn't be read", error.to_string())
            })?;
            Ok(Status { head, operations })
        }));
    }

    /// Reads the configuration in effect in the repository.
    pub(crate) fn refresh_config(&mut self, git: &Git) {
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        self.config.request(jobs::blocking(move || {
            ConfigSnapshot::read(&git, ConfigPlace::Repository(repository.work_tree()))
                .map(Arc::new)
                .map_err(|error| Problem::git("The repository's settings couldn't be read", &error))
        }));
    }
}
