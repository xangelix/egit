//! What the user asks of a repository, as jobs: exchanges with its remotes (fetching, also in
//! the background every few minutes; pulling; pushing), and giving up or carrying on an
//! operation git stopped in the middle of.
//!
//! One task the user asked for runs at a time, under the repository's `WriteLock`, its progress
//! in [`Tasks::progress`], stoppable with [`Tasks::cancel`]. The background fetch can't prompt,
//! doesn't take the lock (a fetch never touches the index), and is stopped when the user starts
//! a task; its failures are kept apart and said quietly.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bstr::{BStr, ByteSlice as _};
use egit_git::{
    CancelToken, ConfigEntry, ConfigSnapshot, ConfigTarget, FetchOptions, Git, GitError, Operation,
    Progress, PullMode, Pulled, PushOptions, Pushed, Repository, abort_operation,
    continue_operation, fetch, pull, push, set_config,
};
use egui_async::Bind;

use crate::jobs::{self, WriteLock};
use crate::problem::Problem;

/// How long after a repository opens its first background fetch starts.
const FIRST_BACKGROUND_FETCH: Duration = Duration::from_secs(10);

/// How often the background fetch runs.
const BACKGROUND_FETCH: Duration = Duration::from_secs(300);

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
}

impl Task {
    /// What it is doing, for people ("Pulling…").
    pub(crate) const fn doing(&self) -> &'static str {
        match self {
            Self::Fetch => "Fetching",
            Self::Pull(_) | Self::PullAndRemember { .. } => "Pulling",
            Self::Push(_) => "Pushing",
            Self::Abort(_) => "Aborting",
            Self::Continue(_) => "Continuing",
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
    background: Bind<(), Problem>,
    background_cancel: CancelToken,
    next_background: Instant,
    /// Why the last background fetch failed, if it did.
    pub(crate) background_problem: Option<Problem>,
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
            background_problem: None,
        }
    }

    /// Whether a task the user asked for is running.
    pub(crate) fn is_busy(&mut self) -> bool {
        self.result.is_pending()
    }

    /// Starts `task` (stopping the background fetch first).
    pub(crate) fn start(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        writes: &WriteLock,
        task: Task,
    ) {
        self.background_cancel.cancel();
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.progress = Progress::new();
        let (git, repository, writes) = (git.clone(), Arc::clone(repository), writes.clone());
        let (progress, cancel) = (self.progress.clone(), self.cancel.clone());
        let running = task.clone();
        self.result.request(jobs::blocking(move || {
            let _queued = writes.hold();
            run(&git, &repository, &running, &progress, &cancel)
        }));
        self.task = Some(task);
        self.started = self.started.wrapping_add(1);
        self.open = true;
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
            self.background = Bind::new(true);
        }
        let due = now >= self.next_background;
        if due && has_remotes && !self.result.is_pending() && !self.background.is_pending() {
            self.next_background = now.checked_add(BACKGROUND_FETCH).unwrap_or(now);
            self.background_cancel = CancelToken::new();
            let (git, repository) = (git.clone(), Arc::clone(repository));
            let (progress, cancel) = (Progress::new(), self.background_cancel.clone());
            self.background.request(jobs::blocking(move || {
                let options = FetchOptions {
                    all: true,
                    prune: true,
                    background: true,
                };
                fetch(&git, &repository, options, &progress, &cancel)
                    .map_err(|error| Problem::git("Fetching in the background failed", &error))
            }));
        }
        (
            finished,
            self.next_background.saturating_duration_since(now),
        )
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
) -> Result<Outcome, Problem> {
    // A cancelled task isn't a failure: the user asked for it.
    let failed = |title: &'static str| {
        move |error: GitError| match error {
            GitError::Cancelled { .. } => Ok(Outcome::Cancelled),
            error => Err(Problem::git(title, &error)),
        }
    };
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
        Task::Abort(operation) => abort_operation(git, repository, *operation, cancel)
            .map(|()| Outcome::Aborted)
            .or_else(failed("It couldn't be aborted")),
        Task::Continue(operation) => continue_operation(git, repository, *operation, cancel)
            .map(|()| Outcome::Continued)
            .or_else(failed("It couldn't carry on")),
    }
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
