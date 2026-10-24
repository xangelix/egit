//! A repository's working tree: its status, read again whenever it may have changed.
//!
//! A read is asked for at once by egit's own writes and by the window regaining focus, and soon
//! by the repository's watcher (the index, `HEAD`, an operation) and the work tree's own (a file
//! saved in an editor). That one, a [`TreeWatcher`], follows the folders git has files in:
//! `HEAD`'s (listed once per commit), those of the status's entries, and folders created since
//! that git doesn't ignore (with up to [`NEW_FOLDERS`] folders inside them), so `mkdir -p` then a
//! new file is seen, and an `npm install` costs no watch. The folders are worked out and watched
//! in the job that reads the status.
//!
//! One read runs at a time (one asked for at once replaces it), and while files keep changing,
//! reads start at most every [`READ_GAP`]. Without a watch (inotify's limits), the status is read
//! every [`UNWATCHED_READS`] instead.
//!
//! On another computer the same folders are followed by a session there, through
//! `inotifywait` where it is installed; until it is told (or where it isn't, or while its
//! session is down), the status is read every [`UNWATCHED_READS`].

#[cfg(target_arch = "wasm32")]
use crate::web::notify;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use bstr::ByteSlice as _;

use bstr::{BStr, BString};
use egit_git::{CancelToken, Git, ObjectId, Repository, Status, ignored_paths, tree_folders};
use egui_async::Bind;
use parking_lot::Mutex;

use crate::jobs;
use crate::problem::Problem;
use crate::watch::{Taken, TreeWatcher};

/// The least time between reads the work tree's watcher asks for.
const READ_GAP: Duration = Duration::from_secs(1);

/// The most folders looked for inside a folder created since the last read.
const NEW_FOLDERS: usize = 1024;

/// How often the status is read while the work tree can't be watched.
const UNWATCHED_READS: Duration = Duration::from_secs(5);

/// How soon a read is wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Due {
    /// Once [`READ_GAP`] has passed since the last.
    Soon,
    /// Now: what egit itself did, or what the user came back to look at.
    Now,
}

/// `HEAD`'s folders, for the commit they were listed for.
type HeadFolders = Option<(ObjectId, Arc<[BString]>)>;

/// A repository's working tree as egit shows it.
pub(crate) struct WorkTree {
    status: Bind<Arc<Status>, Problem>,
    due: Option<Due>,
    /// When the last read started, and the `HEAD` it watched the folders of.
    started: Option<(Instant, Option<ObjectId>)>,
    watcher: Option<Arc<TreeWatcher>>,
    /// Why the work tree isn't (all) watched, when it isn't; reads update it.
    problem: Arc<Mutex<Option<String>>>,
    head_folders: Arc<Mutex<HeadFolders>>,
    repository: Arc<Repository>,
    /// Whether it is on this computer (on another, its folders are followed by a session there,
    /// made with the first read's git).
    here: bool,
    waker: egui::Context,
}

impl WorkTree {
    /// The working tree of `repository` (`here`: on this computer), read at the next
    /// [`Self::poll`]; `ctx` is woken when it changes.
    pub(crate) fn new(repository: Arc<Repository>, here: bool, ctx: &egui::Context) -> Self {
        let mut work_tree = Self {
            status: Bind::new(true),
            due: Some(Due::Now),
            started: None,
            watcher: None,
            problem: Arc::new(Mutex::new(None)),
            head_folders: Arc::new(Mutex::new(None)),
            repository,
            here,
            waker: ctx.clone(),
        };
        if here {
            work_tree.start_watching(None);
        }
        work_tree
    }

    /// The latest status read; the previous one stays while another is read.
    pub(crate) fn status(&mut self) -> Option<Result<&Arc<Status>, &Problem>> {
        self.status.read_as_ref()
    }

    /// Reads the status at once (egit changed it, or the user came back to it).
    pub(crate) const fn refresh_now(&mut self) {
        self.due = Some(Due::Now);
    }

    /// Reads the status soon: the repository's watcher saw it may have changed.
    pub(crate) fn changed(&mut self) {
        self.due = self.due.max(Some(Due::Soon));
    }

    /// Whether no read is due or under way (tests: a screen is checked once what the disk
    /// holds has landed).
    #[cfg(test)]
    pub(crate) fn is_settled(&mut self) -> bool {
        self.due.is_none() && !self.status.is_pending()
    }

    /// Whether `folder` is watched for changes.
    #[cfg(test)]
    pub(crate) fn watches(&self, folder: &Path) -> bool {
        self.watcher
            .as_ref()
            .is_some_and(|watcher| watcher.watches(folder))
    }

    /// Whether its watcher is told what changes (else its status is read every few seconds).
    pub(crate) fn is_live(&self) -> bool {
        self.watcher
            .as_ref()
            .is_some_and(|watcher| watcher.is_live())
    }

    /// Why the work tree's changes aren't (all) followed, while they aren't.
    pub(crate) fn problem(&self) -> Option<String> {
        self.problem.lock().clone()
    }

    /// Takes in the watcher's news and starts a read when one is due, `head` being where
    /// `HEAD` points (`None` before the first commit, or until known); returns how long to
    /// wait before asking again, when something is waiting on time.
    pub(crate) fn poll(
        &mut self,
        git: &Git,
        head: Option<ObjectId>,
        now: Instant,
    ) -> Option<Duration> {
        let mut wait = None;
        // A watcher not told yet (there: its session setting up, down, or without
        // `inotifywait`) is no watcher: the status is read every few seconds meanwhile.
        let watcher = self.watcher.as_ref().filter(|watcher| watcher.is_live());
        match watcher.map(|watcher| watcher.take(now)) {
            Some(Taken::Changed(_)) => self.changed(),
            Some(Taken::Wait(delay)) => wait = Some(delay),
            Some(Taken::Nothing) => {}
            None => {
                let since = self.started.map_or(UNWATCHED_READS, |(started, _)| {
                    now.saturating_duration_since(started)
                });
                if since >= UNWATCHED_READS {
                    self.changed();
                } else {
                    wait = Some(UNWATCHED_READS.saturating_sub(since));
                }
            }
        }
        // A `HEAD` other than the one the folders were watched for: watch the new one's.
        if self.started.is_some_and(|(_, watched)| watched != head) {
            self.changed();
        }
        match self.due {
            None => {}
            Some(Due::Now) => self.start(git, head, now),
            // It starts once the read under way lands (the job wakes the window).
            Some(Due::Soon) if self.status.is_pending() => {}
            Some(Due::Soon) => {
                let ready = self
                    .started
                    .and_then(|(started, _)| started.checked_add(READ_GAP))
                    .unwrap_or(now);
                if now >= ready {
                    self.start(git, head, now);
                } else {
                    let delay = ready.saturating_duration_since(now);
                    wait = Some(wait.map_or(delay, |wait: Duration| wait.min(delay)));
                }
            }
        }
        wait
    }

    /// Starts a read (one under way is left to finish unheeded).
    fn start(&mut self, git: &Git, head: Option<ObjectId>, now: Instant) {
        self.due = None;
        self.started = Some((now, head));
        if self.watcher.is_none() {
            self.start_watching(Some(git));
        }
        let (git, repository) = (git.clone(), Arc::clone(&self.repository));
        let watcher = self.watcher.clone();
        let (problem, head_folders) = (Arc::clone(&self.problem), Arc::clone(&self.head_folders));
        self.status.request(jobs::blocking(async move {
            let cancel = CancelToken::new();
            let status = Status::read(&git, &repository, &cancel)
                .await
                .map_err(|error| Problem::git("The work tree's status couldn't be read", &error))?;
            if let Some(watcher) = watcher {
                let outcome = folders(
                    &git,
                    &repository,
                    &status,
                    head,
                    &head_folders,
                    &watcher,
                    &cancel,
                )
                .await
                .and_then(|folders| {
                    watcher
                        .watch(folders)
                        .map_err(|error| watch_problem(&error))
                });
                *problem.lock() = outcome.err();
            }
            Ok(Arc::new(status))
        }));
    }

    /// Makes the work tree's watcher (it watches nothing until the next read): here, or on the
    /// other computer `git` runs on.
    fn start_watching(&mut self, git: Option<&Git>) {
        let waker = self.waker.clone();
        if !self.here {
            if let Some(git) = git {
                self.watcher = Some(Arc::new(TreeWatcher::elsewhere(
                    git.clone(),
                    Arc::clone(&self.repository),
                    move |delay| waker.request_repaint_after(delay),
                )));
            }
            return;
        }
        match TreeWatcher::new(
            self.repository.work_tree().to_owned(),
            self.repository.git_dir().to_owned(),
            move |delay| {
                waker.request_repaint_after(delay);
            },
        ) {
            Ok(watcher) => self.watcher = Some(Arc::new(watcher)),
            Err(error) => *self.problem.lock() = Some(watch_problem(&error)),
        }
    }
}

/// Why a watch failed, for people.
fn watch_problem(error: &notify::Error) -> String {
    match error.kind {
        notify::ErrorKind::MaxFilesWatch => "the system's limit on watched folders is reached \
             (fs.inotify.max_user_watches); other programs may be using it up"
            .to_owned(),
        _ => error.to_string(),
    }
}

/// The folders to watch: the work tree's top, `HEAD`'s folders (`head_folders` keeps them for a
/// commit), those of `status`'s entries, and those created since that git doesn't ignore.
async fn folders(
    git: &Git,
    repository: &Repository,
    status: &Status,
    head: Option<ObjectId>,
    head_folders: &Mutex<HeadFolders>,
    watcher: &TreeWatcher,
    cancel: &CancelToken,
) -> Result<HashSet<PathBuf>, String> {
    let root = repository.work_tree();
    let mut folders = HashSet::with_capacity(watcher.len().saturating_add(16));
    folders.insert(root.to_owned());
    if let Some(head) = head {
        let cached = head_folders
            .lock()
            .as_ref()
            .filter(|(commit, _)| *commit == head)
            .map(|(_, listed)| Arc::clone(listed));
        let listed = if let Some(listed) = cached {
            listed
        } else {
            let listed: Arc<[BString]> = tree_folders(git, repository, head, cancel)
                .await
                .map_err(|error| error.to_string())?
                .into();
            *head_folders.lock() = Some((head, Arc::clone(&listed)));
            listed
        };
        folders.extend(
            listed
                .iter()
                .map(|folder| root.join(folder.to_os_str_lossy())),
        );
    }
    for entry in status.entries() {
        for parent in entry.path.to_path_lossy().ancestors().skip(1) {
            // Once one is in, so are those above it.
            if parent.as_os_str().is_empty() || !folders.insert(root.join(parent)) {
                break;
            }
        }
    }
    let created = created_folders(git, root, &watcher.take_created(), &folders).await?;
    if !created.is_empty() {
        let relative: Vec<BString> = created
            .iter()
            .map(|folder| {
                let mut path = egit_git::git_path(folder.strip_prefix(root).unwrap_or(folder));
                path.push(b'/');
                path
            })
            .collect();
        let paths: Vec<&BStr> = relative.iter().map(AsRef::as_ref).collect();
        let ignored: HashSet<BString> = ignored_paths(git, repository, &paths, cancel)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect();
        folders.extend(
            created
                .into_iter()
                .zip(&relative)
                .filter(|(_, path)| !ignored.contains(*path))
                .map(|(folder, _)| folder),
        );
    }
    Ok(folders)
}

/// The folders in `created` (made since the last read) and those inside them, up to
/// [`NEW_FOLDERS`], but those `known` already and git's own, on the computer `git` runs on.
async fn created_folders(
    git: &Git,
    root: &Path,
    created: &[PathBuf],
    known: &HashSet<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let roots: Vec<&Path> = created
        .iter()
        .filter(|folder| folder.starts_with(root) && !known.contains(*folder))
        .map(PathBuf::as_path)
        .collect();
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let mut found = git
        .files()
        .folders_within(&roots, NEW_FOLDERS)
        .await
        .map_err(|error| error.to_string())?;
    found.retain(|folder| folder.starts_with(root) && !known.contains(folder));
    found.sort();
    found.dedup();
    Ok(found)
}
