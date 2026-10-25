//! A repository's CI/CD pipelines (GitLab's, and GitHub Actions' workflow runs), for the commit
//! table's marks: each commit's pipelines among the forge's repositories, read in the background
//! (each repository's newest [`LIMIT`]), read again whenever the requests are (after a task
//! reaching the remotes, each background fetch, a sign-in, Refresh), and, while any shown may
//! still change by itself, the newest page read again every [`EVERY`] and merged in: a mark
//! turns green or red soon after its pipeline does.
//!
//! A commit's mark weighs its pipelines as GitHub's checks do: the newest of each series (each
//! GitHub workflow's runs; a GitLab commit's pipelines are one series, its newest standing for
//! it), the most telling shown (a failure, then one under way, waiting, cancelled, skipped,
//! passed: [`egit_forge::CiStatus::weight`]), the others beside it.
//!
//! A read takes over only once complete, so marks never blink out meanwhile; one that fails
//! keeps what is shown (a project whose pipelines can't be read has none to show). Each full
//! read says how each project went ([`CiSeen`]: how many, hidden by GitLab, failed and why),
//! and how many of the commits they ran for a history shows is counted in a job when asked
//! ([`Pipelines::on_history`]), so the repository's Overview can say why no mark shows.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use egit_forge::{
    Account, ForgeError, ForgeRepository, Pipeline, PipelineListing, has_pipelines, pipelines,
};
use egit_git::{CancelToken, ObjectId};
use egui_async::Bind;

use crate::history::History;
use crate::jobs;
use crate::problem::Problem;

/// The most pipelines read for each project: its newest, the latest commits'.
pub(crate) const LIMIT: u32 = 500;

/// How often the newest pipelines are read again while one shown is under way.
pub(crate) const EVERY: Duration = Duration::from_secs(20);

/// How many are read again then: GitLab's page.
const LIVE: u32 = 100;

/// A commit's pipelines: the most telling (shown), and which of the forge's repositories it ran
/// in; and the others (another GitHub workflow's runs), each with its repository's place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CiMark {
    /// Its repository's place in the forge's.
    pub(crate) repository: usize,
    pub(crate) pipeline: Pipeline,
    /// The commit's other pipelines, the most telling first.
    pub(crate) others: Vec<(usize, Pipeline)>,
}

impl CiMark {
    /// The commit's pipelines, the one shown first.
    pub(crate) fn runs(&self) -> impl Iterator<Item = (usize, &Pipeline)> {
        std::iter::once((self.repository, &self.pipeline))
            .chain(self.others.iter().map(|(at, pipeline)| (*at, pipeline)))
    }

    /// The same pipelines with other `at` (its place among the others) shown, the rest beside
    /// it.
    pub(crate) fn showing(&self, at: usize) -> Option<Self> {
        let mut others: Vec<(usize, Pipeline)> = self
            .runs()
            .map(|(place, pipeline)| (place, pipeline.clone()))
            .collect();
        let shown = at.checked_add(1).filter(|shown| *shown < others.len())?;
        let (repository, pipeline) = others.remove(shown);
        Some(Self {
            repository,
            pipeline,
            others,
        })
    }

    /// `runs` as a mark: the most telling shown (the newest of those alike), the rest beside.
    fn of(mut runs: Vec<(usize, Pipeline)>) -> Option<Self> {
        runs.sort_by(|(_, a), (_, b)| {
            b.status
                .weight()
                .cmp(&a.status.weight())
                .then_with(|| (b.created, b.id).cmp(&(a.created, a.id)))
        });
        let mut runs = runs.into_iter();
        let (repository, pipeline) = runs.next()?;
        Some(Self {
            repository,
            pipeline,
            others: runs.collect(),
        })
    }
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
            .any(|(_, mark)| mark.runs().any(|(_, pipeline)| pipeline.status.is_moving()))
    }

    /// `previous`'s marks with `found` taken in: a pipeline takes its series' place on its
    /// commit when it is the same pipeline read again, or a newer one (of any repository: a
    /// GitLab commit's newest pipeline of any of the projects).
    fn merged(previous: Option<&Self>, found: Vec<(usize, Pipeline)>) -> Self {
        let mut by: HashMap<ObjectId, Vec<(usize, Pipeline)>> =
            previous.map_or_default(|previous| {
                previous
                    .by_commit
                    .iter()
                    .map(|(id, mark)| {
                        let runs = mark
                            .runs()
                            .map(|(at, pipeline)| (at, pipeline.clone()))
                            .collect();
                        (*id, runs)
                    })
                    .collect()
            });
        for (repository, pipeline) in found {
            let runs = by.entry(pipeline.commit).or_default();
            match runs
                .iter_mut()
                .find(|(_, held)| held.series == pipeline.series)
            {
                Some(held) => {
                    let same = held.0 == repository && held.1.id == pipeline.id;
                    if same || pipeline.is_newer_than(&held.1) {
                        *held = (repository, pipeline);
                    }
                }
                None => runs.push((repository, pipeline)),
            }
        }
        let mut by_commit: Vec<(ObjectId, CiMark)> = by
            .into_iter()
            .filter_map(|(id, runs)| Some((id, CiMark::of(runs)?)))
            .collect();
        by_commit.sort_unstable_by_key(|(id, _)| *id);
        Self { by_commit }
    }
}

/// How reading a project's pipelines went, last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CiSeen {
    /// Not read (yet).
    NotRead,
    /// Read: how many (each project's newest [`LIMIT`] at most), and the newest.
    Read {
        count: usize,
        newest: Option<Box<Pipeline>>,
    },
    /// The forge lists none to this account: GitLab's CI/CD turned off for the project, or a
    /// role that may not see them; GitHub shows no Actions there.
    Hidden,
    /// Not asked: its host failed for another project first (a sign-in, the rate limit).
    Skipped,
    /// Couldn't be read: why.
    Failed(String),
}

/// What a read found.
#[derive(Debug)]
struct Read {
    /// The marks: those shown before (a live read's) with what was found taken in.
    marks: Marks,
    /// How each project went: a full read's (a live one reads only the newest page).
    seen: Option<Vec<CiSeen>>,
    /// Why nothing could be read, when nothing could.
    problem: Option<Problem>,
}

/// The history and the marks a count of commits was made for: the history's identity, and
/// where the marks are.
type Counted = ((u64, usize), usize);

/// A repository's pipelines: the marks shown, and the read under way.
pub(crate) struct Pipelines {
    shown: Arc<Marks>,
    reading: Bind<Read, Problem>,
    /// Why the last read failed (none of the projects could be read), until one goes through.
    problem: Option<Problem>,
    /// How each project went in the last full read, by its place in the forge's.
    seen: Arc<[CiSeen]>,
    /// How many of the marks' commits a history shows, counted for one history and marks.
    counting: Option<(Counted, Bind<usize, Problem>)>,
    /// Whether to read them all again once the read under way ends.
    again: bool,
    /// When the newest are read again (while one shown is under way).
    next: Option<Instant>,
    cancel: CancelToken,
    /// Wakes the window when the newest are due again.
    waker: crate::repaint::Repaint,
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
    pub(crate) fn new(waker: &crate::repaint::Repaint) -> Self {
        Self {
            shown: Arc::default(),
            reading: Bind::new(true),
            problem: None,
            seen: Arc::from([]),
            counting: None,
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

    /// Why the pipelines couldn't be read, when the last read failed.
    pub(crate) const fn problem(&self) -> Option<&Problem> {
        self.problem.as_ref()
    }

    /// How each project went in the last full read, by its place in the forge's (none yet).
    pub(crate) fn seen(&self) -> &[CiSeen] {
        &self.seen
    }

    /// Whether a read is under way.
    pub(crate) fn is_reading(&mut self) -> bool {
        self.reading.is_pending()
    }

    /// How many commits have a pipeline among those read.
    pub(crate) fn commits(&self) -> usize {
        self.shown.len()
    }

    /// How many of the commits with a pipeline `history` shows: counted in a job the first
    /// time it is asked for this history and these marks (`None` until it lands).
    pub(crate) fn on_history(&mut self, history: &History) -> Option<usize> {
        let key: Counted = (
            history.identity(),
            Arc::as_ptr(&self.shown).cast::<u8>().addr(),
        );
        if self
            .counting
            .as_ref()
            .is_none_or(|(counted, _)| *counted != key)
        {
            let (names, marks) = (history.names(), Arc::clone(&self.shown));
            let mut counting = Bind::new(true);
            counting.request(jobs::blocking(async move {
                let mut commits: Vec<&[u8]> = marks
                    .by_commit
                    .iter()
                    .map(|(id, _)| id.as_bytes())
                    .collect();
                commits.sort_unstable();
                let width = names.width();
                let shown = names
                    .chunks()
                    .flat_map(|(_, chunk)| chunk.chunks_exact(width))
                    .filter(|name| commits.binary_search(name).is_ok())
                    .count();
                Ok(shown)
            }));
            self.counting = Some((key, counting));
        }
        self.counting
            .as_mut()
            .and_then(|(_, counting)| counting.ok_ref().copied())
    }

    /// Reads the newest again at the next poll, as it would every [`EVERY`] while one shown is
    /// under way (after something asked of GitLab; tests: not waiting that long).
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
        self.problem = None;
        self.seen = Arc::from([]);
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
                Some(Ok(read)) => {
                    if let Some(seen) = read.seen {
                        self.seen = seen.into();
                    }
                    if let Some(problem) = read.problem {
                        tracing::debug!(%problem, "pipelines unread");
                        self.problem = Some(problem);
                    } else {
                        self.shown = Arc::new(read.marks);
                        self.problem = None;
                    }
                }
                Some(Err(problem)) => tracing::debug!(%problem, "pipelines unread"),
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
        self.reading.request(jobs::blocking(async move {
            read(
                &repositories,
                &accounts,
                previous.as_deref(),
                (limit, live),
                &cancel,
            )
            .await
        }));
    }
}

impl Drop for Pipelines {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Reads each project's newest `limit` pipelines (`live`: the newest page, merged into
/// `previous`'s marks), saying how each project went (a full read). Fails only when cancelled;
/// a read in which no project could be read says why.
async fn read(
    repositories: &[ForgeRepository],
    accounts: &[Arc<Account>],
    previous: Option<&Marks>,
    (limit, live): (u32, bool),
    cancel: &CancelToken,
) -> Result<Read, Problem> {
    let mut found = Vec::new();
    let mut seen = vec![CiSeen::NotRead; repositories.len()];
    let mut failed = None;
    let mut read_any = false;
    // A host that needs signing in fails the same way for each of its projects.
    let mut failed_hosts = Vec::new();
    for ((index, repository), went) in repositories.iter().enumerate().zip(&mut seen) {
        let host = repository.host();
        if !has_pipelines(host.kind()) {
            continue;
        }
        if failed_hosts.contains(&host) {
            *went = CiSeen::Skipped;
            continue;
        }
        let Some(account) = accounts.iter().find(|account| account.host() == host) else {
            continue;
        };
        let before = found.len();
        let pages = pipelines(account, repository, limit, cancel, |page| {
            found.extend(page.into_iter().map(|pipeline| (index, pipeline)));
        })
        .await;
        let project = repository.full_name();
        match pages {
            Ok(listing) => {
                read_any = true;
                let count = found.len().saturating_sub(before);
                tracing::debug!(%project, count, ?listing, "pipelines read");
                *went = match listing {
                    PipelineListing::Hidden => CiSeen::Hidden,
                    PipelineListing::Listed => CiSeen::Read {
                        count,
                        newest: found
                            .get(before)
                            .map(|(_, pipeline)| Box::new(pipeline.clone())),
                    },
                };
            }
            Err(ForgeError::Cancelled) => {
                return Err(Problem::new("Reading the pipelines stopped", String::new()));
            }
            Err(error) => {
                tracing::debug!(%project, %error, "pipelines unread");
                if error.needs_sign_in() || matches!(error, ForgeError::RateLimited { .. }) {
                    failed_hosts.push(host);
                }
                *went = CiSeen::Failed(error.to_string());
                failed = Some(error);
            }
        }
    }
    let problem = failed
        .filter(|_| !read_any)
        .map(|error| Problem::new("The pipelines couldn't be read", error.to_string()));
    Ok(Read {
        marks: Marks::merged(previous, found),
        seen: (!live).then_some(seen),
        problem,
    })
}

#[cfg(test)]
mod tests {
    use egit_forge::CiStatus;

    use super::*;

    fn pipeline(id: u64, commit: u8, status: CiStatus, created: i64) -> Pipeline {
        Pipeline {
            id,
            iid: id,
            name: None,
            number: id,
            series: 0,
            attempt: 1,
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

    #[test]
    fn a_commit_s_workflows_stand_side_by_side_the_most_telling_shown() {
        let run = |id, series, status, created| Pipeline {
            series,
            ..pipeline(id, 1, status, created)
        };
        // CI's newest attempt passed, Docs failed, Lint runs: Docs' failure shows.
        let marks = Marks::merged(
            None,
            vec![
                (0, run(1, 11, CiStatus::Failed, 100)),
                (0, run(2, 11, CiStatus::Success, 200)),
                (0, run(3, 12, CiStatus::Failed, 150)),
                (0, run(4, 13, CiStatus::Running, 120)),
            ],
        );
        let mark = marks
            .of(&pipeline(0, 1, CiStatus::Unknown, 0).commit)
            .unwrap();
        assert_eq!(mark.pipeline.id, 3);
        let others: Vec<u64> = mark.others.iter().map(|(_, run)| run.id).collect();
        assert_eq!(others, [4, 2]);
        assert!(marks.is_moving());
        // Docs read again, passed: Lint's run under way shows.
        let again = Marks::merged(Some(&marks), vec![(0, run(3, 12, CiStatus::Success, 150))]);
        let mark = again
            .of(&pipeline(0, 1, CiStatus::Unknown, 0).commit)
            .unwrap();
        assert_eq!((mark.pipeline.id, mark.others.len()), (4, 2));
    }
}
