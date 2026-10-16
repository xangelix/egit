//! Noticing what changes repositories and the user's configuration behind egit's back: a commit
//! or checkout in a terminal, a fetch, an edited configuration file.
//!
//! inotify watches (through `notify`) cover git directories and configuration files only, never
//! a work tree. What changed accumulates as [`Changes`] and is handed over once nothing has
//! changed for [`SETTLE`], so a command that writes many files causes one refresh. Reads change
//! nothing: egit's own (`--no-optional-locks`) never trigger a refresh, and access events are
//! ignored.
//!
//! A watch can fail to be set up: inotify's limits are per user, and an editor watching a large
//! tree can use them all. [`Watching`] then tries again, ever less often, and once it works it
//! hands over every kind of change, since what changed meanwhile went unseen.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_git::Repository;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use parking_lot::Mutex;

/// How long things must stay quiet before what changed is handed over.
pub(crate) const SETTLE: Duration = Duration::from_millis(120);

/// How long after a watch failed it is first tried again; each failure doubles it, up to
/// [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(1);

/// The longest wait between tries of a watch that keeps failing.
const MAX_RETRY: Duration = Duration::from_secs(30);

/// What changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes(u8);

impl Changes {
    /// Nothing.
    pub(crate) const NONE: Self = Self(0);
    /// `HEAD`, a branch, a tag, or a remote-tracking branch.
    pub(crate) const REFS: Self = Self(1);
    /// An operation's state (a merge or rebase starting or ending).
    pub(crate) const STATE: Self = Self(1 << 1);
    /// Configuration.
    pub(crate) const CONFIG: Self = Self(1 << 2);
    /// The index (what is staged).
    pub(crate) const INDEX: Self = Self(1 << 3);
    /// Everything.
    pub(crate) const ALL: Self = Self::REFS
        .union(Self::STATE)
        .union(Self::CONFIG)
        .union(Self::INDEX);

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What [`Watcher::take`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    /// Nothing changed.
    Nothing,
    /// Things are still changing: ask again after this long.
    Wait(Duration),
    /// These changed, and things have settled.
    Changed(Changes),
}

#[derive(Debug, Default)]
struct Pending {
    changes: Changes,
    last: Option<Instant>,
}

/// Watches some directories and accumulates what changed in them.
pub(crate) struct Watcher {
    _inner: RecommendedWatcher,
    pending: Arc<Mutex<Pending>>,
}

impl Watcher {
    /// Watches `watches`, sorting each changed path with `classify`; `wake` is called (on the
    /// watcher's thread) whenever something changed, to have [`Self::take`] called after
    /// [`SETTLE`].
    fn start(
        watches: &[(PathBuf, RecursiveMode)],
        classify: impl Fn(&Path) -> Changes + Send + 'static,
        wake: impl Fn(Duration) + Send + 'static,
    ) -> notify::Result<Self> {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let shared = Arc::clone(&pending);
        let mut inner =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::debug!(%error, "a watch failed");
                        return;
                    }
                };
                // Events were dropped (inotify's queue overflowed): anything may have changed.
                let changes = if event.need_rescan() {
                    Changes::ALL
                } else if matters(event.kind) {
                    event
                        .paths
                        .iter()
                        .fold(Changes::NONE, |changes, path| changes.union(classify(path)))
                } else {
                    Changes::NONE
                };
                if changes.is_empty() {
                    return;
                }
                {
                    let mut pending = shared.lock();
                    pending.changes = pending.changes.union(changes);
                    pending.last = Some(Instant::now());
                }
                wake(SETTLE);
            })?;
        for (path, mode) in watches {
            inner.watch(path, *mode)?;
        }
        Ok(Self {
            _inner: inner,
            pending,
        })
    }

    /// Watches a repository's git directories: `HEAD`, refs, configuration, the index, and
    /// operation state.
    pub(crate) fn repository(
        repository: &Repository,
        wake: impl Fn(Duration) + Send + 'static,
    ) -> notify::Result<Self> {
        let git_dir = repository.git_dir().to_owned();
        let common_dir = repository.common_dir().to_owned();
        let mut watches = vec![(git_dir.clone(), RecursiveMode::NonRecursive)];
        if common_dir != git_dir {
            watches.push((common_dir.clone(), RecursiveMode::NonRecursive));
        }
        let refs = common_dir.join("refs");
        if refs.is_dir() {
            watches.push((refs, RecursiveMode::Recursive));
        }
        Self::start(
            &watches,
            move |path| classify_repository(path, &git_dir, &common_dir),
            wake,
        )
    }

    /// Watches configuration files (through their folders, since editors replace files rather
    /// than write them in place).
    pub(crate) fn files(
        files: Vec<PathBuf>,
        wake: impl Fn(Duration) + Send + 'static,
    ) -> notify::Result<Self> {
        let mut folders: Vec<PathBuf> = files
            .iter()
            .filter_map(|file| file.parent())
            .filter(|folder| folder.is_dir())
            .map(Path::to_owned)
            .collect();
        folders.sort();
        folders.dedup();
        let watches: Vec<_> = folders
            .into_iter()
            .map(|folder| (folder, RecursiveMode::NonRecursive))
            .collect();
        Self::start(
            &watches,
            move |path| {
                if files.iter().any(|file| file == path) {
                    Changes::CONFIG
                } else {
                    Changes::NONE
                }
            },
            wake,
        )
    }

    /// What changed, once things have settled for [`SETTLE`] at `now`.
    pub(crate) fn take(&self, now: Instant) -> Taken {
        let mut pending = self.pending.lock();
        let Some(last) = pending.last else {
            return Taken::Nothing;
        };
        let quiet = now.saturating_duration_since(last);
        if quiet < SETTLE {
            return Taken::Wait(SETTLE.saturating_sub(quiet));
        }
        let changes = std::mem::take(&mut pending.changes);
        pending.last = None;
        drop(pending);
        Taken::Changed(changes)
    }
}

/// A watch kept trying: a [`Watcher`], or why there is none and when to try again.
pub(crate) struct Watching {
    state: State,
}

enum State {
    Active(Watcher),
    Failed {
        /// Why, for people.
        reason: String,
        retry_at: Instant,
        /// The wait before `retry_at`, doubled at each failure.
        backoff: Duration,
    },
}

impl Watching {
    /// Starts a watch with `start` at `now`; one that fails is tried again by [`Self::take`].
    pub(crate) fn new(now: Instant, start: impl FnOnce() -> notify::Result<Watcher>) -> Self {
        let state = match start() {
            Ok(watcher) => State::Active(watcher),
            Err(error) => failed(&error, now, FIRST_RETRY),
        };
        Self { state }
    }

    /// What changed (see [`Watcher::take`]). A failed watch is tried again with `start` once due,
    /// and everything counts as changed when it then works.
    pub(crate) fn take(
        &mut self,
        now: Instant,
        start: impl FnOnce() -> notify::Result<Watcher>,
    ) -> Taken {
        let State::Failed {
            retry_at, backoff, ..
        } = &self.state
        else {
            return match &self.state {
                State::Active(watcher) => watcher.take(now),
                State::Failed { .. } => Taken::Nothing,
            };
        };
        if now < *retry_at {
            return Taken::Wait(retry_at.saturating_duration_since(now));
        }
        let backoff = backoff.saturating_mul(2).min(MAX_RETRY);
        match start() {
            Ok(watcher) => {
                tracing::info!("watching for changes made outside egit again");
                self.state = State::Active(watcher);
                Taken::Changed(Changes::ALL)
            }
            Err(error) => {
                self.state = failed(&error, now, backoff);
                Taken::Wait(backoff)
            }
        }
    }

    /// Why changes aren't being followed, while they aren't.
    pub(crate) fn problem(&self) -> Option<&str> {
        match &self.state {
            State::Active(_) => None,
            State::Failed { reason, .. } => Some(reason),
        }
    }
}

/// The state of a watch that failed at `now`, tried again after `backoff`.
fn failed(error: &notify::Error, now: Instant, backoff: Duration) -> State {
    let reason = match error.kind {
        notify::ErrorKind::MaxFilesWatch => "the system's limit on watched folders is reached \
             (fs.inotify.max_user_watches); other programs may be using it up"
            .to_owned(),
        _ => error.to_string(),
    };
    tracing::warn!(%error, ?backoff, "changes made outside egit aren't followed; trying again");
    State::Failed {
        reason,
        retry_at: now.checked_add(backoff).unwrap_or(now),
        backoff,
    }
}

/// Whether an event can mean a change: creations, writes, renames, and removals, not reads or
/// attribute updates.
const fn matters(kind: EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        EventKind::Modify(modify) => !matches!(modify, notify::event::ModifyKind::Metadata(_)),
        EventKind::Access(_) | EventKind::Other => false,
    }
}

/// What a changed path in a repository's git directories means.
fn classify_repository(path: &Path, git_dir: &Path, common_dir: &Path) -> Changes {
    // A lock file means a write in progress; its rename into place is what counts.
    if path
        .extension()
        .is_some_and(|extension| extension == "lock")
    {
        return Changes::NONE;
    }
    // A linked work tree's git directory is inside the common one: look at it first.
    if let Ok(relative) = path.strip_prefix(git_dir) {
        let found = classify_git_dir(relative);
        if !found.is_empty() || git_dir != common_dir {
            return found;
        }
    }
    path.strip_prefix(common_dir)
        .map_or(Changes::NONE, classify_common_dir)
}

/// A path relative to a work tree's own git directory.
fn classify_git_dir(relative: &Path) -> Changes {
    let Some(first) = relative.components().next() else {
        return Changes::NONE;
    };
    match first.as_os_str().to_str() {
        Some("HEAD") => Changes::REFS.union(Changes::STATE),
        Some("index") => Changes::INDEX,
        Some("config.worktree") => Changes::CONFIG,
        Some(
            "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "BISECT_LOG" | "rebase-merge"
            | "rebase-apply" | "sequencer",
        ) => Changes::STATE,
        _ => Changes::NONE,
    }
}

/// A path relative to the directory every work tree shares.
fn classify_common_dir(relative: &Path) -> Changes {
    let Some(first) = relative.components().next() else {
        return Changes::NONE;
    };
    match first.as_os_str().to_str() {
        Some("refs" | "packed-refs") => Changes::REFS,
        Some("config") => Changes::CONFIG,
        _ => Changes::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_worktree(path: &str) -> Changes {
        classify_repository(Path::new(path), Path::new("/r/.git"), Path::new("/r/.git"))
    }

    fn linked_worktree(path: &str) -> Changes {
        classify_repository(
            Path::new(path),
            Path::new("/r/.git/worktrees/w"),
            Path::new("/r/.git"),
        )
    }

    #[test]
    fn sorts_a_main_work_trees_changes() {
        assert_eq!(
            main_worktree("/r/.git/HEAD"),
            Changes::REFS.union(Changes::STATE)
        );
        assert_eq!(main_worktree("/r/.git/refs/heads/feature/x"), Changes::REFS);
        assert_eq!(main_worktree("/r/.git/packed-refs"), Changes::REFS);
        assert_eq!(main_worktree("/r/.git/config"), Changes::CONFIG);
        assert_eq!(main_worktree("/r/.git/index"), Changes::INDEX);
        assert_eq!(main_worktree("/r/.git/MERGE_HEAD"), Changes::STATE);
        assert_eq!(main_worktree("/r/.git/rebase-merge/done"), Changes::STATE);
    }

    #[test]
    fn ignores_what_isnt_state() {
        assert_eq!(main_worktree("/r/.git/index.lock"), Changes::NONE);
        assert_eq!(main_worktree("/r/.git/refs/heads/main.lock"), Changes::NONE);
        assert_eq!(main_worktree("/r/.git/objects/ab/cdef"), Changes::NONE);
        assert_eq!(main_worktree("/r/.git/logs/HEAD"), Changes::NONE);
        assert_eq!(main_worktree("/r/.git/FETCH_HEAD"), Changes::NONE);
        assert_eq!(main_worktree("/elsewhere/HEAD"), Changes::NONE);
    }

    #[test]
    fn a_linked_work_tree_has_its_own_head_and_shares_refs() {
        assert_eq!(
            linked_worktree("/r/.git/worktrees/w/HEAD"),
            Changes::REFS.union(Changes::STATE)
        );
        assert_eq!(linked_worktree("/r/.git/worktrees/w/index"), Changes::INDEX);
        assert_eq!(linked_worktree("/r/.git/refs/heads/main"), Changes::REFS);
        assert_eq!(linked_worktree("/r/.git/config"), Changes::CONFIG);
        // The main work tree's own HEAD and index aren't this one's.
        assert_eq!(linked_worktree("/r/.git/HEAD"), Changes::NONE);
        assert_eq!(linked_worktree("/r/.git/index"), Changes::NONE);
    }

    #[test]
    fn hands_changes_over_once_settled() {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let watcher = Watcher {
            _inner: notify::recommended_watcher(|_: notify::Result<notify::Event>| {}).unwrap(),
            pending: Arc::clone(&pending),
        };
        let start = Instant::now();
        assert_eq!(watcher.take(start), Taken::Nothing);
        {
            let mut pending = pending.lock();
            pending.changes = Changes::REFS;
            pending.last = Some(start);
        }
        assert_eq!(
            watcher.take(start + Duration::from_millis(20)),
            Taken::Wait(SETTLE.saturating_sub(Duration::from_millis(20)))
        );
        assert_eq!(watcher.take(start + SETTLE), Taken::Changed(Changes::REFS));
        assert_eq!(watcher.take(start + SETTLE * 2), Taken::Nothing);
    }

    fn quiet_watcher() -> notify::Result<Watcher> {
        Ok(Watcher {
            _inner: notify::recommended_watcher(|_: notify::Result<notify::Event>| {})?,
            pending: Arc::new(Mutex::new(Pending::default())),
        })
    }

    fn at_limit() -> notify::Result<Watcher> {
        Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch))
    }

    #[test]
    fn a_failed_watch_is_tried_again_ever_less_often_and_then_reports_everything() {
        let start = Instant::now();
        let mut watching = Watching::new(start, at_limit);
        assert!(watching.problem().unwrap().contains("max_user_watches"));
        assert_eq!(
            watching.take(start + Duration::from_millis(400), || panic!("not due")),
            Taken::Wait(Duration::from_millis(600))
        );
        assert_eq!(
            watching.take(start + FIRST_RETRY, at_limit),
            Taken::Wait(FIRST_RETRY * 2)
        );
        assert_eq!(
            watching.take(start + FIRST_RETRY * 3, at_limit),
            Taken::Wait(FIRST_RETRY * 4)
        );
        let later = start + FIRST_RETRY * 7;
        assert_eq!(
            watching.take(later, quiet_watcher),
            Taken::Changed(Changes::ALL)
        );
        assert_eq!(watching.problem(), None);
        assert_eq!(watching.take(later, || panic!("working")), Taken::Nothing);
    }

    #[test]
    fn retries_slow_down_no_further_than_the_limit() {
        let mut now = Instant::now();
        let mut watching = Watching::new(now, at_limit);
        let mut last = Duration::ZERO;
        for _ in 0..12 {
            now += MAX_RETRY;
            if let Taken::Wait(wait) = watching.take(now, at_limit) {
                last = wait;
            }
        }
        assert_eq!(last, MAX_RETRY);
    }

    #[test]
    fn reads_and_attribute_changes_dont_matter() {
        use notify::event::{AccessKind, CreateKind, MetadataKind, ModifyKind, RemoveKind};
        assert!(matters(EventKind::Create(CreateKind::File)));
        assert!(matters(EventKind::Remove(RemoveKind::File)));
        assert!(matters(EventKind::Modify(ModifyKind::Any)));
        assert!(!matters(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Any
        ))));
        assert!(!matters(EventKind::Access(AccessKind::Any)));
    }
}
