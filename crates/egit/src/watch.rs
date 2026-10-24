//! Noticing what changes repositories and the user's configuration behind egit's back: a commit
//! or checkout in a terminal, a fetch, an edited configuration file.
//!
//! Watches (through `notify`: inotify on Linux, `FSEvents` on macOS, `ReadDirectoryChangesW` on
//! Windows) cover git directories, configuration files, and a work tree: on Linux each of its
//! folders on its own ([`TreeWatcher`]: only those git has files in, so a build's output or
//! downloaded dependencies cost no watch, as inotify's watches are one a folder and limited);
//! where one watch of a whole tree is cheap (macOS, Windows), the tree at once. What changed accumulates as
//! [`Changes`] and is handed over once nothing has changed for [`SETTLE`], so a command that
//! writes many files causes one refresh. Reads change nothing: egit's own
//! (`--no-optional-locks`) never trigger a refresh, and access events are ignored.
//!
//! A watch can fail to be set up: inotify's limits are per user, and an editor watching a large
//! tree can use them all. [`Watching`] then tries again, ever less often, and once it works it
//! hands over every kind of change, since what changed meanwhile went unseen.
//!
//! A repository on another computer is followed by sessions there (`egit_git::look_there`,
//! `look_at_folders_there`): told at once where `inotifywait` is installed there, else looking
//! at its git directories every [`LOOKING_THERE`] (and its work tree's status read every few
//! seconds by its owner). So is the user's configuration where git runs elsewhere (in a
//! browser, its relay's computer: `look_at_files_there`).

#[cfg(target_arch = "wasm32")]
use crate::web::notify;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use egit_git::{Follow, Git, INOTIFYWAIT, Liveness, Looked, Looking, Repository, Seen};
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
    /// A file in the work tree.
    pub(crate) const WORK_TREE: Self = Self(1 << 4);
    /// Everything.
    pub(crate) const ALL: Self = Self::REFS
        .union(Self::STATE)
        .union(Self::CONFIG)
        .union(Self::INDEX)
        .union(Self::WORK_TREE);

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// What a session there said changed.
    const fn from_looked(looked: Looked) -> Self {
        let mut changes = Self::NONE;
        if looked.intersects(Looked::REFS) {
            changes = changes.union(Self::REFS);
        }
        if looked.intersects(Looked::STATE) {
            changes = changes.union(Self::STATE);
        }
        if looked.intersects(Looked::CONFIG) {
            changes = changes.union(Self::CONFIG);
        }
        if looked.intersects(Looked::INDEX) {
            changes = changes.union(Self::INDEX);
        }
        changes
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

impl Pending {
    /// Notes `changes`, made at `now`.
    const fn note(&mut self, changes: Changes, now: Instant) {
        self.changes = self.changes.union(changes);
        self.last = Some(now);
    }

    /// What changed, once things have settled for [`SETTLE`] at `now`.
    fn take(&mut self, now: Instant) -> Taken {
        let Some(last) = self.last else {
            return Taken::Nothing;
        };
        let quiet = now.saturating_duration_since(last);
        if quiet < SETTLE {
            return Taken::Wait(SETTLE.saturating_sub(quiet));
        }
        self.last = None;
        Taken::Changed(std::mem::take(&mut self.changes))
    }
}

/// Watches some directories and accumulates what changed in them.
pub(crate) struct Watcher {
    inner: Inner,
    pending: Arc<Mutex<Pending>>,
}

/// What follows the changes: a watch here, or a loop looking there.
enum Inner {
    /// Kept for its watches.
    Here {
        _watcher: RecommendedWatcher,
    },
    There(egit_git::Looking),
}

/// How often a repository on another computer is looked at, where its session isn't told at
/// once.
pub(crate) const LOOKING_THERE: Duration = Duration::from_secs(2);

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
                shared.lock().note(changes, Instant::now());
                wake(SETTLE);
            })?;
        for (path, mode) in watches {
            inner.watch(path, *mode)?;
        }
        Ok(Self {
            inner: Inner::Here { _watcher: inner },
            pending,
        })
    }

    /// Follows a repository on another computer (`git`'s): a session there told at once what
    /// a watch here would see, where `inotifywait` is installed there, else looking every
    /// [`LOOKING_THERE`].
    pub(crate) fn elsewhere(
        git: &Git,
        repository: &Repository,
        wake: impl Fn(Duration) + Send + 'static,
    ) -> notify::Result<Self> {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let shared = Arc::clone(&pending);
        let follow = Follow::at_once(LOOKING_THERE);
        let looking = egit_git::look_there(git, repository, &follow, move |looked| {
            shared
                .lock()
                .note(Changes::from_looked(looked), Instant::now());
            wake(SETTLE);
        })
        .map_err(notify::Error::io)?;
        Ok(Self {
            inner: Inner::There(looking),
            pending,
        })
    }

    /// Whether it stopped following (a session there that ended: the connection was lost).
    pub(crate) fn has_ended(&self) -> bool {
        match &self.inner {
            Inner::Here { .. } => false,
            Inner::There(looking) => looking.has_ended(),
        }
    }

    /// For a repository on another computer, whether its session is told at once what changes
    /// (else it looks every [`LOOKING_THERE`]); `None` here.
    pub(crate) fn is_live(&self) -> Option<bool> {
        match &self.inner {
            Inner::Here { .. } => None,
            Inner::There(looking) => Some(looking.is_live()),
        }
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

    /// Follows configuration files on the computer `git` runs on, another (a session there:
    /// see [`Self::elsewhere`]).
    pub(crate) fn files_there(
        git: &Git,
        files: &[PathBuf],
        wake: impl Fn(Duration) + Send + 'static,
    ) -> notify::Result<Self> {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let shared = Arc::clone(&pending);
        let follow = Follow::at_once(LOOKING_THERE);
        let looking = egit_git::look_at_files_there(git, files, &follow, move || {
            shared.lock().note(Changes::CONFIG, Instant::now());
            wake(SETTLE);
        })
        .map_err(notify::Error::io)?;
        Ok(Self {
            inner: Inner::There(looking),
            pending,
        })
    }

    /// What changed, once things have settled for [`SETTLE`] at `now`.
    pub(crate) fn take(&self, now: Instant) -> Taken {
        self.pending.lock().take(now)
    }
}

/// Watches a work tree, reporting [`Changes::WORK_TREE`]: on Linux its folders each on its own
/// (never their subfolders), on macOS and Windows the whole tree in one watch, and on another
/// computer its folders each on its own through a session there (where `inotifywait` is
/// installed there).
///
/// Which folders is up to its owner ([`Self::watch`]): those git has files in, so a build's
/// output or downloaded dependencies cost no watch. Folders created meanwhile are noted for the
/// owner to consider ([`Self::take_created`]), and a folder removed loses its watch.
pub(crate) struct TreeWatcher {
    work_tree: PathBuf,
    watches: Mutex<Watches>,
    events: Arc<Mutex<TreeEvents>>,
    /// Whether it watches here (where it is told once made).
    here: bool,
    /// There, how its session stands (apart from `watches`, which a job may hold a while).
    liveness: Mutex<Option<Liveness>>,
}

/// Whether one watch takes in a whole tree here (`FSEvents`, `ReadDirectoryChangesW`), so a work
/// tree is watched at once rather than folder by folder (inotify's watches are one a folder).
const WHOLE_TREE: bool = cfg!(any(target_os = "macos", windows));

/// How long after a session there failed (inotify's limits there, a connection lost) one is
/// started again.
const RETRY_THERE: Duration = Duration::from_secs(30);

/// What follows the work tree's folders.
enum Watches {
    Here(TreeWatches),
    There(TreeThere),
}

/// The watcher here and the folders it watches.
struct TreeWatches {
    watcher: RecommendedWatcher,
    folders: HashSet<PathBuf>,
    /// Whether the whole work tree is watched ([`WHOLE_TREE`]).
    whole: bool,
}

/// The session following the folders there, started again whenever they change.
struct TreeThere {
    git: Git,
    repository: Arc<Repository>,
    wake: Arc<dyn Fn(Duration) + Send + Sync>,
    session: Option<Looking>,
    folders: HashSet<PathBuf>,
    /// Why the last session failed, and when one may be started again.
    failed: Option<(String, Instant)>,
}

/// What the watcher's events left for its owner.
#[derive(Debug, Default)]
struct TreeEvents {
    pending: Pending,
    /// Folders created since last asked.
    created: Vec<PathBuf>,
    /// Folders removed (their watches went with them).
    removed: HashSet<PathBuf>,
    /// There: `inotifywait` isn't installed (its status is read every few seconds instead).
    missing: bool,
    /// There: why its session gave up, when it did.
    failed: Option<String>,
}

impl TreeWatcher {
    /// A watcher for `work_tree`, whose git directory is `git_dir` (its changes are left to the
    /// repository's own watcher), watching nothing yet; `wake` is called when something
    /// changed, to have [`Self::take`] called after [`SETTLE`].
    pub(crate) fn new(
        work_tree: PathBuf,
        git_dir: PathBuf,
        wake: impl Fn(Duration) + Send + 'static,
    ) -> notify::Result<Self> {
        let events = Arc::new(Mutex::new(TreeEvents::default()));
        let shared = Arc::clone(&events);
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    tracing::debug!(%error, "a work tree watch failed");
                    return;
                }
            };
            let in_work_tree = |path: &&PathBuf| !path.starts_with(&git_dir);
            let changed = event.need_rescan()
                || (matters(event.kind) && event.paths.iter().any(|path| in_work_tree(&path)));
            if !changed {
                return;
            }
            {
                let mut events = shared.lock();
                events.pending.note(Changes::WORK_TREE, Instant::now());
                let folders = event.paths.iter().filter(in_work_tree).cloned();
                match event.kind {
                    EventKind::Create(notify::event::CreateKind::Folder) => {
                        events.created.extend(folders);
                    }
                    EventKind::Remove(notify::event::RemoveKind::Folder) => {
                        events.removed.extend(folders);
                    }
                    _ => {}
                }
            }
            wake(SETTLE);
        })?;
        Ok(Self {
            work_tree,
            watches: Mutex::new(Watches::Here(TreeWatches {
                watcher,
                folders: HashSet::new(),
                whole: false,
            })),
            events,
            here: true,
            liveness: Mutex::new(None),
        })
    }

    /// A watcher for `repository`'s work tree on the other computer `git` runs on: a session
    /// there, once [`Self::watch`] names its folders.
    pub(crate) fn elsewhere(
        git: Git,
        repository: Arc<Repository>,
        wake: impl Fn(Duration) + Send + Sync + 'static,
    ) -> Self {
        Self {
            work_tree: repository.work_tree().to_owned(),
            watches: Mutex::new(Watches::There(TreeThere {
                git,
                repository,
                wake: Arc::new(wake),
                session: None,
                folders: HashSet::new(),
                failed: None,
            })),
            events: Arc::new(Mutex::new(TreeEvents::default())),
            here: false,
            liveness: Mutex::new(None),
        }
    }

    /// Watches exactly `folders`: those new are added, those gone dropped. A folder that no
    /// longer exists is left out. Each new watch is a system call (there, a session started
    /// again): run it in a job.
    ///
    /// # Errors
    ///
    /// When a watch can't be added (inotify's per-user limit, most likely); those added
    /// before it stay. There, when its session gave up (not tried again for a while).
    pub(crate) fn watch(&self, folders: HashSet<PathBuf>) -> notify::Result<()> {
        let mut state = self.watches.lock();
        match &mut *state {
            Watches::Here(watches) => {
                let removed = std::mem::take(&mut self.events.lock().removed);
                self.watch_here(watches, folders, &removed)
            }
            Watches::There(there) => self.watch_there(there, folders),
        }
    }

    /// [`Self::watch`] here.
    fn watch_here(
        &self,
        state: &mut TreeWatches,
        folders: HashSet<PathBuf>,
        removed: &HashSet<PathBuf>,
    ) -> notify::Result<()> {
        let TreeWatches {
            watcher,
            folders: watched,
            whole,
        } = state;
        if WHOLE_TREE {
            if !*whole {
                watcher.watch(&self.work_tree, RecursiveMode::Recursive)?;
                *whole = true;
            }
            // Each of them is in the tree watched.
            *watched = folders;
            return Ok(());
        }
        watched.retain(|folder| !removed.contains(folder));
        let gone: Vec<PathBuf> = watched.difference(&folders).cloned().collect();
        for folder in gone {
            // A folder deleted took its watch with it: there is nothing to undo.
            let _ = watcher.unwatch(&folder);
            watched.remove(&folder);
        }
        for folder in folders {
            if watched.contains(&folder) {
                continue;
            }
            match watcher.watch(&folder, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    watched.insert(folder);
                }
                Err(error) if gone_already(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// [`Self::watch`] there: the session started again with `folders` unless it follows them
    /// already; none where `inotifywait` isn't installed, nor for a while after one failed.
    fn watch_there(&self, there: &mut TreeThere, folders: HashSet<PathBuf>) -> notify::Result<()> {
        let now = Instant::now();
        let (missing, failed) = {
            let mut events = self.events.lock();
            (events.missing, events.failed.take())
        };
        if missing {
            there.session = None;
            *self.liveness.lock() = None;
            return Ok(());
        }
        let ended = there.session.as_ref().is_some_and(Looking::has_ended);
        if let Some(reason) = failed
            .or_else(|| ended.then(|| "the connection to the other computer was lost".to_owned()))
        {
            there.session = None;
            there.failed = Some((reason, now.checked_add(RETRY_THERE).unwrap_or(now)));
        }
        if there.session.is_some() && there.folders == folders {
            return Ok(());
        }
        if let Some((reason, retry_at)) = &there.failed
            && now < *retry_at
        {
            return Err(notify::Error::generic(&format!(
                "{reason} (on the other computer)"
            )));
        }
        there.failed = None;
        let (shared, wake) = (Arc::clone(&self.events), Arc::clone(&there.wake));
        let session = egit_git::look_at_folders_there(
            &there.git,
            &there.repository,
            folders.iter().map(PathBuf::as_path),
            std::ffi::OsStr::new(INOTIFYWAIT),
            move |seen| {
                {
                    let mut events = shared.lock();
                    match seen {
                        // What changed while its watches were set up, too.
                        Seen::Ready | Seen::Changed => {}
                        Seen::Created(folder) => events.created.push(folder),
                        Seen::Missing => {
                            events.missing = true;
                            return;
                        }
                        Seen::Failed(reason) => {
                            events.failed = Some(if reason.is_empty() {
                                "inotifywait stopped".to_owned()
                            } else {
                                reason
                            });
                        }
                    }
                    events.pending.note(Changes::WORK_TREE, Instant::now());
                }
                wake(SETTLE);
            },
        )
        .map_err(notify::Error::io)?;
        // The session it replaces stops as it is dropped.
        *self.liveness.lock() = Some(session.liveness());
        there.session = Some(session);
        there.folders = folders;
        Ok(())
    }

    /// Whether it is told what changes: here once made; there while its session is (else its
    /// owner reads the status every few seconds).
    pub(crate) fn is_live(&self) -> bool {
        self.here || self.liveness.lock().as_ref().is_some_and(Liveness::is_live)
    }

    /// Whether `folder` is watched.
    #[cfg(test)]
    pub(crate) fn watches(&self, folder: &Path) -> bool {
        match &*self.watches.lock() {
            Watches::Here(watches) => watches.folders.contains(folder),
            Watches::There(there) => there.folders.contains(folder),
        }
    }

    /// How many folders are watched.
    pub(crate) fn len(&self) -> usize {
        match &*self.watches.lock() {
            Watches::Here(watches) => watches.folders.len(),
            Watches::There(there) => there.folders.len(),
        }
    }

    /// The folders created since last asked.
    pub(crate) fn take_created(&self) -> Vec<PathBuf> {
        std::mem::take(&mut self.events.lock().created)
    }

    /// What changed, once things have settled for [`SETTLE`] at `now`.
    pub(crate) fn take(&self, now: Instant) -> Taken {
        self.events.lock().pending.take(now)
    }
}

/// Whether a watch failed because its folder no longer exists.
fn gone_already(error: &notify::Error) -> bool {
    match &error.kind {
        notify::ErrorKind::PathNotFound => true,
        notify::ErrorKind::Io(error) => error.kind() == std::io::ErrorKind::NotFound,
        _ => false,
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
                // There, its session ended: tried again, as a watch that failed.
                State::Active(watcher) if watcher.has_ended() => {
                    self.state = State::Failed {
                        reason: "the connection to the other computer was lost".to_owned(),
                        retry_at: now.checked_add(FIRST_RETRY).unwrap_or(now),
                        backoff: FIRST_RETRY,
                    };
                    Taken::Wait(FIRST_RETRY)
                }
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

    /// For a repository on another computer, whether its session is told at once what changes
    /// (else it looks every [`LOOKING_THERE`]); `None` here, or while nothing follows them.
    pub(crate) fn is_live(&self) -> Option<bool> {
        match &self.state {
            State::Active(watcher) => watcher.is_live(),
            State::Failed { .. } => None,
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
    Changes::from_looked(egit_git::classify(path, git_dir, common_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_changes_over_once_settled() {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let watcher = Watcher {
            inner: Inner::Here {
                _watcher: notify::recommended_watcher(|_: notify::Result<notify::Event>| {})
                    .unwrap(),
            },
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
            inner: Inner::Here {
                _watcher: notify::recommended_watcher(|_: notify::Result<notify::Event>| {})?,
            },
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
