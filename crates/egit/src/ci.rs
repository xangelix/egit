//! A repository's CI/CD pipelines (GitLab's), for the commit table's marks: each commit's latest
//! pipeline among the forge's projects, read in the background (each project's newest
//! [`LIMIT`]), read again whenever the requests are (after a task reaching the remotes, each
//! background fetch, a sign-in, Refresh), and, while any shown may still change by itself, the
//! newest page read again every [`EVERY`] and merged in: a mark turns green or red soon after
//! its pipeline does.
//!
//! A read takes over only once complete, so marks never blink out meanwhile; one that fails
//! keeps what is shown (a project whose pipelines can't be read has none to show).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_forge::{Account, ForgeError, ForgeRepository, Pipeline, has_pipelines, pipelines};
use egit_git::{CancelToken, ObjectId};
use egui_async::Bind;

use crate::jobs;
use crate::problem::Problem;

/// The most pipelines read for each project: its newest, the latest commits'.
pub(crate) const LIMIT: u32 = 500;

/// How often the newest pipelines are read again while one shown is under way.
pub(crate) const EVERY: Duration = Duration::from_secs(20);

/// How many are read again then: GitLab's page.
const LIVE: u32 = 100;

/// A commit's latest pipeline, and which of the forge's repositories it ran in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CiMark {
    /// Its repository's place in the forge's.
    pub(crate) repository: usize,
    pub(crate) pipeline: Pipeline,
}

/// Each commit's latest pipeline, by commit.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Marks {
    /// Sorted by commit.
    by_commit: Vec<(ObjectId, CiMark)>,
}

impl Marks {
    /// `commit`'s latest pipeline, if it has one among those read.
    pub(crate) fn of(&self, commit: &ObjectId) -> Option<&CiMark> {
        self.by_commit
            .binary_search_by(|(id, _)| id.cmp(commit))
            .ok()
            .and_then(|at| self.by_commit.get(at))
            .map(|(_, mark)| mark)
    }

    /// How many commits have one.
    pub(crate) const fn len(&self) -> usize {
        self.by_commit.len()
    }

    /// Whether any may still change by itself.
    fn is_moving(&self) -> bool {
        self.by_commit
            .iter()
            .any(|(_, mark)| mark.pipeline.status.is_moving())
    }

    /// `previous`'s marks with `found` taken in: a pipeline replaces a commit's mark when it is
    /// the same pipeline read again, or a newer one (of any of the projects).
    fn merged(previous: Option<&Self>, found: Vec<(usize, Pipeline)>) -> Self {
        let mut by: HashMap<ObjectId, CiMark> = previous
            .map(|previous| previous.by_commit.iter().cloned().collect())
            .unwrap_or_default();
        for (repository, pipeline) in found {
            match by.entry(pipeline.commit) {
                Entry::Occupied(mut held) => {
                    let known = &held.get();
                    let same = known.repository == repository && known.pipeline.id == pipeline.id;
                    if same || pipeline.is_newer_than(&known.pipeline) {
                        held.insert(CiMark {
                            repository,
                            pipeline,
                        });
                    }
                }
                Entry::Vacant(empty) => {
                    empty.insert(CiMark {
                        repository,
                        pipeline,
                    });
                }
            }
        }
        let mut by_commit: Vec<(ObjectId, CiMark)> = by.into_iter().collect();
        by_commit.sort_unstable_by_key(|(id, _)| *id);
        Self { by_commit }
    }
}

/// A repository's pipelines: the marks shown, and the read under way.
pub(crate) struct Pipelines {
    shown: Arc<Marks>,
    reading: Bind<Arc<Marks>, Problem>,
    /// Whether to read them all again once the read under way ends.
    again: bool,
    /// When the newest are read again (while one shown is under way).
    next: Option<Instant>,
    cancel: CancelToken,
    /// Wakes the window when the newest are due again.
    waker: egui::Context,
}

impl std::fmt::Debug for Pipelines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipelines")
            .field("shown", &self.shown.len())
            .field("again", &self.again)
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

impl Pipelines {
    /// None known yet; `waker` is woken when a read is due.
    pub(crate) fn new(waker: &egui::Context) -> Self {
        Self {
            shown: Arc::default(),
            reading: Bind::new(true),
            again: false,
            next: None,
            cancel: CancelToken::new(),
            waker: waker.clone(),
        }
    }

    /// `commit`'s latest pipeline, if it has one.
    pub(crate) fn of(&self, commit: &ObjectId) -> Option<&CiMark> {
        self.shown.of(commit)
    }

    /// Whether any commit has one (the commit table then keeps room for a mark on every row).
    pub(crate) fn any(&self) -> bool {
        self.shown.len() > 0
    }

    /// Reads the newest again at the next poll, as it would every [`EVERY`] while one shown is
    /// under way (tests: not waiting that long).
    #[cfg(test)]
    pub(crate) fn read_newest_now(&mut self) {
        self.next = Some(Instant::now());
    }

    /// Forgets every mark (the remotes changed) and reads `repositories`' pipelines afresh.
    pub(crate) fn reset(
        &mut self,
        repositories: &Arc<[ForgeRepository]>,
        accounts: &[Arc<Account>],
    ) {
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.reading = Bind::new(true);
        self.shown = Arc::default();
        self.again = false;
        self.next = None;
        self.start(repositories, accounts, false);
    }

    /// Reads them all again (after the read under way, if any).
    pub(crate) fn refresh(
        &mut self,
        repositories: &Arc<[ForgeRepository]>,
        accounts: &[Arc<Account>],
    ) {
        if self.reading.is_pending() {
            self.again = true;
        } else {
            self.start(repositories, accounts, false);
        }
    }

    /// Takes in a finished read, and starts the next when one is due.
    pub(crate) fn poll(
        &mut self,
        repositories: &Arc<[ForgeRepository]>,
        accounts: &[Arc<Account>],
    ) {
        let now = Instant::now();
        if self.reading.is_finished() {
            match self.reading.take() {
                Some(Ok(marks)) => self.shown = marks,
                Some(Err(problem)) => {
                    tracing::debug!(%problem, "pipelines unread");
                }
                None => {}
            }
            // Under way, read again soon; a read that failed too, while some are.
            self.next = self
                .shown
                .is_moving()
                .then(|| now.checked_add(EVERY))
                .flatten();
        }
        if self.reading.is_pending() {
            return;
        }
        if self.again {
            self.again = false;
            self.start(repositories, accounts, false);
            return;
        }
        if let Some(next) = self.next {
            if now >= next {
                self.next = None;
                self.start(repositories, accounts, true);
            } else {
                self.waker
                    .request_repaint_after(next.saturating_duration_since(now));
            }
        }
    }

    /// Starts a read: every project's newest [`LIMIT`] (`live`: the newest page, merged into
    /// what is shown). Nothing where no project has pipelines.
    fn start(
        &mut self,
        repositories: &Arc<[ForgeRepository]>,
        accounts: &[Arc<Account>],
        live: bool,
    ) {
        let wanted = repositories
            .iter()
            .any(|repository| has_pipelines(repository.host().kind()));
        if !wanted {
            return;
        }
        let repositories = Arc::clone(repositories);
        let accounts = accounts.to_vec();
        let previous = live.then(|| Arc::clone(&self.shown));
        let limit = if live { LIVE } else { LIMIT };
        let cancel = self.cancel.clone();
        self.reading.request(jobs::blocking(move || {
            read(
                &repositories,
                &accounts,
                previous.as_deref(),
                limit,
                &cancel,
            )
            .map(Arc::new)
        }));
    }
}

impl Drop for Pipelines {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Reads each project's newest `limit` pipelines, merged into `previous`'s marks (none: from
/// nothing). Fails only when no project could be read.
fn read(
    repositories: &[ForgeRepository],
    accounts: &[Arc<Account>],
    previous: Option<&Marks>,
    limit: u32,
    cancel: &CancelToken,
) -> Result<Marks, Problem> {
    let mut found = Vec::new();
    let mut failed = None;
    let mut read_any = false;
    // A host that needs signing in fails the same way for each of its projects.
    let mut failed_hosts = Vec::new();
    for (index, repository) in repositories.iter().enumerate() {
        let host = repository.host();
        if !has_pipelines(host.kind()) || failed_hosts.contains(&host) {
            continue;
        }
        let Some(account) = accounts.iter().find(|account| account.host() == host) else {
            continue;
        };
        let pages = pipelines(account, repository, limit, cancel, |page| {
            found.extend(page.into_iter().map(|pipeline| (index, pipeline)));
        });
        match pages {
            Ok(()) => read_any = true,
            Err(ForgeError::Cancelled) => {
                return Err(Problem::new("Reading the pipelines stopped", String::new()));
            }
            Err(error) => {
                if error.needs_sign_in() || matches!(error, ForgeError::RateLimited { .. }) {
                    failed_hosts.push(host);
                }
                failed = Some(error);
            }
        }
    }
    if !read_any && let Some(error) = failed {
        return Err(Problem::new(
            "The pipelines couldn't be read",
            error.to_string(),
        ));
    }
    Ok(Marks::merged(previous, found))
}

#[cfg(test)]
mod tests {
    use egit_forge::CiStatus;

    use super::*;

    fn pipeline(id: u64, commit: u8, status: CiStatus, created: i64) -> Pipeline {
        Pipeline {
            id,
            iid: id,
            commit: ObjectId::from_hex(format!("{commit:040x}").as_bytes()).unwrap(),
            status,
            warnings: false,
            git_ref: None,
            created,
            url: String::new(),
        }
    }

    #[test]
    fn a_commit_shows_its_newest_pipeline_of_any_project() {
        let marks = Marks::merged(
            None,
            vec![
                (0, pipeline(12, 1, CiStatus::Failed, 200)),
                (0, pipeline(11, 1, CiStatus::Success, 100)),
                (1, pipeline(30, 1, CiStatus::Running, 300)),
                (0, pipeline(10, 2, CiStatus::Success, 50)),
            ],
        );
        let first = marks
            .of(&pipeline(0, 1, CiStatus::Unknown, 0).commit)
            .unwrap();
        assert_eq!((first.repository, first.pipeline.id), (1, 30));
        assert_eq!(marks.len(), 2);
        assert!(marks.is_moving());
        assert_eq!(marks.of(&pipeline(0, 9, CiStatus::Unknown, 0).commit), None);

        // Read again: the same pipeline, finished, takes over; an older one doesn't.
        let again = Marks::merged(
            Some(&marks),
            vec![
                (1, pipeline(30, 1, CiStatus::Success, 300)),
                (0, pipeline(9, 2, CiStatus::Failed, 10)),
            ],
        );
        let first = again
            .of(&pipeline(0, 1, CiStatus::Unknown, 0).commit)
            .unwrap();
        assert_eq!(first.pipeline.status, CiStatus::Success);
        let second = again
            .of(&pipeline(0, 2, CiStatus::Unknown, 0).commit)
            .unwrap();
        assert_eq!(second.pipeline.id, 10);
        assert!(!again.is_moving());
    }
}
