//! CI/CD pipelines: GitLab's, through its GraphQL API ([`gitlab`]), and GitHub Actions'
//! workflow runs, through GitHub's REST API ([`github`]), alike. A repository's latest (the
//! newest first, a page at a time), each with its commit, status, and page; and one whole: its
//! stages (a GitHub workflow's jobs grouped) and jobs (the latest run of each, the runs before
//! it beside), what can be done to them, and whether the account may; and asking that of the
//! forge. A GitHub workflow's file says what each job needs ([`Workflow`]): the caller reads it
//! at the run's commit and lays it over the details ([`PipelineDetails::with_workflow`]).

use egit_git::{CancelToken, ObjectId};
use serde::Deserialize;

use crate::account::Account;
use crate::artifacts::Artifact;
use crate::error::ForgeError;
use crate::host::ForgeKind;
use crate::repository::ForgeRepository;

mod github;
mod gitlab;
mod workflow;

pub(crate) use github::{github_job, github_log};
pub use workflow::Workflow;

/// Pipelines (or a run's jobs) a page: either forge's largest.
const PAGE: u32 = 100;

/// A pipeline's, a stage's, or a job's status, as GitLab names it (GitHub's read into them:
/// [`CiStatus::from_github`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CiStatus {
    /// Made, nothing started yet.
    Created,
    /// Waiting for a resource group's other job to finish.
    WaitingForResource,
    /// Its environment being prepared.
    Preparing,
    /// Waiting for something outside GitLab to call back.
    WaitingForCallback,
    /// Queued for a runner.
    Pending,
    /// Running.
    Running,
    /// Passed.
    Success,
    /// Failed.
    Failed,
    /// Being cancelled.
    Canceling,
    /// Cancelled.
    Canceled,
    /// Skipped (a job before it failed, or its rules left it out).
    Skipped,
    /// Waiting for someone to start it (a manual job).
    Manual,
    /// Waiting for its time (a delayed job).
    Scheduled,
    /// One GitLab added after egit was written.
    Unknown,
}

/// How a status shows: the five groups egit colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CiGroup {
    /// Passed: green.
    Success,
    /// Failed: red.
    Failed,
    /// Under way (queued or running): blue.
    Running,
    /// Waiting for someone, or for its time: orange.
    Paused,
    /// Cancelled or skipped (or unknown): grey.
    Canceled,
}

impl CiStatus {
    /// The status GitLab names `name` (its enum's `RUNNING`, a stage's `running`, a detailed
    /// status's `waiting-for-resource`); [`Self::Unknown`] for one egit doesn't know.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "created" => Self::Created,
            "waiting_for_resource" => Self::WaitingForResource,
            "preparing" => Self::Preparing,
            "waiting_for_callback" => Self::WaitingForCallback,
            "pending" => Self::Pending,
            "running" => Self::Running,
            "success" | "passed" => Self::Success,
            "failed" => Self::Failed,
            "canceling" => Self::Canceling,
            "canceled" | "cancelled" => Self::Canceled,
            "skipped" => Self::Skipped,
            "manual" => Self::Manual,
            "scheduled" => Self::Scheduled,
            _ => Self::Unknown,
        }
    }

    /// The status GitHub gives as a status and, once `completed`, a conclusion: queued (or
    /// requested, pending) is pending, `waiting` (for a deployment's approval) and
    /// `action_required` wait for someone, `timed_out` and `startup_failure` failed, `neutral`
    /// passed, `stale` cancelled.
    #[must_use]
    pub fn from_github(status: Option<&str>, conclusion: Option<&str>) -> Self {
        match status.unwrap_or_default() {
            "completed" => match conclusion.unwrap_or_default() {
                "success" | "neutral" => Self::Success,
                "failure" | "timed_out" | "startup_failure" => Self::Failed,
                "cancelled" | "stale" => Self::Canceled,
                "skipped" => Self::Skipped,
                "action_required" => Self::Manual,
                _ => Self::Unknown,
            },
            "in_progress" => Self::Running,
            "queued" | "requested" | "pending" => Self::Pending,
            "waiting" => Self::Manual,
            _ => Self::Unknown,
        }
    }

    /// How much it says of a commit, among its pipelines: a failure first, then one under way,
    /// waiting for someone, cancelled, skipped, and last one that passed (a mark shows the most
    /// telling).
    #[must_use]
    pub const fn weight(self) -> u8 {
        match self.group() {
            CiGroup::Failed => 5,
            CiGroup::Running => 4,
            CiGroup::Paused => 3,
            CiGroup::Canceled if matches!(self, Self::Skipped) => 1,
            CiGroup::Canceled => 2,
            CiGroup::Success => 0,
        }
    }

    /// Which of the five it shows as.
    #[must_use]
    pub const fn group(self) -> CiGroup {
        match self {
            Self::Success => CiGroup::Success,
            Self::Failed => CiGroup::Failed,
            Self::Created
            | Self::WaitingForResource
            | Self::Preparing
            | Self::WaitingForCallback
            | Self::Pending
            | Self::Running => CiGroup::Running,
            Self::Manual | Self::Scheduled => CiGroup::Paused,
            Self::Canceling | Self::Canceled | Self::Skipped | Self::Unknown => CiGroup::Canceled,
        }
    }

    /// Whether it may still change by itself (it is under way, or will start at its time):
    /// worth reading again soon.
    #[must_use]
    pub const fn is_moving(self) -> bool {
        matches!(self.group(), CiGroup::Running)
            || matches!(self, Self::Scheduled | Self::Canceling)
    }

    /// In words, as GitLab says it ("passed", "waiting for resource").
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::WaitingForResource => "waiting for resource",
            Self::Preparing => "preparing",
            Self::WaitingForCallback => "waiting for callback",
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "passed",
            Self::Failed => "failed",
            Self::Canceling => "canceling",
            Self::Canceled => "canceled",
            Self::Skipped => "skipped",
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
            Self::Unknown => "unknown",
        }
    }
}

/// A pipeline, as a list of them shows it: a GitLab pipeline, or a GitHub Actions workflow's
/// run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    /// The forge's number for it, which what is asked of it names (GitLab's global id, a
    /// GitHub run's id).
    pub id: u64,
    /// The number it is read again by: GitLab's within its project, a GitHub run's id.
    pub iid: u64,
    /// Its name, where the forge gives one: a GitHub workflow's ("CI"); none for GitLab's.
    pub name: Option<String>,
    /// The number its page shows (`#123456`): GitLab's global one, a GitHub workflow's run
    /// number.
    pub number: u64,
    /// Which pipelines of a commit stand side by side: each GitHub workflow's runs are a
    /// series of their own (its id), as GitHub's checks show each workflow; a GitLab commit's
    /// pipelines are one (0), its newest standing for it.
    pub series: u64,
    /// Which attempt of it this is: GitHub runs a workflow again in place (2 after one
    /// re-run); 1 for GitLab, whose retries are its jobs'.
    pub attempt: u32,
    /// The commit it ran for.
    pub commit: ObjectId,
    /// Where it stands.
    pub status: CiStatus,
    /// Whether it passed with warnings (a job allowed to fail did).
    pub warnings: bool,
    /// The branch or tag it ran for, if any (a merge request's has its own ref).
    pub git_ref: Option<String>,
    /// When it was made (seconds since the Unix epoch).
    pub created: i64,
    /// Its page.
    pub url: String,
}

impl Pipeline {
    /// Whether it is newer than `other`: made later (or, made in the same second, numbered
    /// later). Of a commit's pipelines of a series, the newest says how it stands.
    #[must_use]
    pub fn is_newer_than(&self, other: &Self) -> bool {
        (self.created, self.id) > (other.created, other.id)
    }

    /// What its page is titled: "Pipeline #123456", a GitHub workflow's run "CI #57".
    #[must_use]
    pub fn title(&self) -> String {
        format!(
            "{} #{}",
            self.name.as_deref().unwrap_or("Pipeline"),
            self.number
        )
    }
}

/// A pipeline whole, as its page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineDetails {
    /// What a list shows of it.
    pub pipeline: Pipeline,
    /// When it started and finished (seconds since the Unix epoch), if it has.
    pub started: Option<i64>,
    /// When it finished.
    pub finished: Option<i64>,
    /// How long it ran, in seconds, once known.
    pub duration: Option<u64>,
    /// Who made it run (their username).
    pub user: Option<String>,
    /// What made it run (GitLab's `push`, `merge_request_event`, `schedule`, `web`, …;
    /// GitHub's `push`, `pull_request`, `workflow_dispatch`, …).
    pub source: Option<String>,
    /// Its stages: GitLab's, in the order they run; a GitHub workflow's jobs, each with its
    /// matrix's (or a reusable workflow's) beside it, in the order they were made.
    pub stages: Vec<Stage>,
    /// What can be done to it, as the forge says.
    pub actions: PipelineActions,
    /// Whether the account may retry and cancel its jobs (its role on the project; on GitHub,
    /// whether it may push to the repository).
    pub can_update: bool,
    /// Whether its stages run in order, each once those before it are done (GitLab's); a
    /// GitHub workflow's jobs run as their needs say.
    pub ordered_stages: bool,
    /// The file its jobs and their needs are written in, as the repository holds it at its
    /// commit (a GitHub workflow's, `.github/workflows/ci.yml`), for [`Self::with_workflow`].
    pub workflow: Option<String>,
    /// What the run uploaded to download (a GitHub run's artifacts; a GitLab pipeline's are
    /// its jobs': [`Job::artifacts`]).
    pub artifacts: Vec<Artifact>,
}

/// What can be done to a pipeline, as the forge says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineActions {
    /// Whether it has jobs to retry (failed, cancelled).
    pub retry: bool,
    /// Whether it has jobs to cancel (under way, waiting).
    pub cancel: bool,
    /// Whether every job of it can be run again (GitHub's, once it ended).
    pub rerun: bool,
}

/// An earlier run of a job: one retried since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarlierRun {
    /// Its number, for its log.
    pub id: u64,
    /// How it ended.
    pub status: CiStatus,
    /// How long it ran, in seconds.
    pub duration: Option<u64>,
    /// When it started (seconds since the Unix epoch).
    pub started: Option<i64>,
    /// Its page.
    pub url: Option<String>,
}

/// A stage of a pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    /// Its name.
    pub name: String,
    /// Where it stands, all its jobs together.
    pub status: CiStatus,
    /// Its jobs (each one's latest run).
    pub jobs: Vec<Job>,
}

/// A job of a stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// Its number, for its log (none for a trigger job, which runs another pipeline).
    pub id: Option<u64>,
    /// Its name.
    pub name: String,
    /// Where it stands.
    pub status: CiStatus,
    /// Whether it may fail without failing the pipeline.
    pub allow_failure: bool,
    /// Whether someone starts it by hand.
    pub manual: bool,
    /// How long it ran, in seconds, once known.
    pub duration: Option<u64>,
    /// When it started (seconds since the Unix epoch), if it has.
    pub started: Option<i64>,
    /// Its page (a trigger job's: its downstream pipeline's), if it has one.
    pub url: Option<String>,
    /// The jobs it waits for by name (`needs`), where it is scheduled by them; none where it
    /// waits for every job of the stages before it (GitLab's alone: a GitHub job without needs
    /// waits for none).
    pub needs: Option<Vec<String>>,
    /// What can be done to it, as the forge says.
    pub actions: JobActions,
    /// Whether the account may do those (its role on the project).
    pub can_update: bool,
    /// Its earlier runs in this place of the pipeline (retried since), the newest first.
    pub earlier: Vec<EarlierRun>,
    /// What it kept to download (a GitLab job's archive and reports; a GitHub run's are the
    /// run's: [`PipelineDetails::artifacts`]).
    pub artifacts: Vec<Artifact>,
}

/// What can be done to a job, as the forge says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct JobActions {
    /// Whether it can be run again (it ended).
    #[serde(default, rename = "retryable")]
    pub retry: bool,
    /// Whether it can be cancelled (under way, or waiting).
    #[serde(default, rename = "cancelable")]
    pub cancel: bool,
    /// Whether it can be started (a manual job waiting).
    #[serde(default, rename = "playable")]
    pub play: bool,
}

/// Whether the forge listed a repository's pipelines to the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineListing {
    /// Listed (none, perhaps: the project has run none).
    Listed,
    /// Hidden: GitLab gives no list (CI/CD turned off for the project, or the account's role
    /// may not see its pipelines); GitHub shows the account no Actions there.
    Hidden,
}

/// Whether repositories on `kind` have pipelines egit reads: GitLab's, and GitHub's Actions.
#[must_use]
pub const fn has_pipelines(kind: ForgeKind) -> bool {
    matches!(kind, ForgeKind::GitLab | ForgeKind::GitHub)
}

/// Reads `repository`'s latest pipelines (a GitHub repository's workflow runs), and says
/// whether the forge listed them at all.
///
/// The newest first, a page at a time (each handed to `found` as it lands), up to `limit` of
/// them.
///
/// # Errors
///
/// Any of [`ForgeError`]'s, from the page that failed (the pages before it were handed over).
pub fn pipelines(
    account: &Account,
    repository: &ForgeRepository,
    limit: u32,
    cancel: &CancelToken,
    mut found: impl FnMut(Vec<Pipeline>),
) -> Result<PipelineListing, ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitLab => gitlab::pipelines(account, repository, limit, cancel, &mut found),
        ForgeKind::GitHub => github::pipelines(account, repository, limit, cancel, &mut found),
    }
}

/// Reads pipeline `iid` of `repository` whole (GitLab's number within its project, a GitHub
/// run's id: [`Pipeline::iid`]).
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub fn pipeline_details(
    account: &Account,
    repository: &ForgeRepository,
    iid: u64,
    cancel: &CancelToken,
) -> Result<PipelineDetails, ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitLab => gitlab::pipeline_details(account, repository, iid, cancel),
        ForgeKind::GitHub => github::pipeline_details(account, repository, iid, cancel),
    }
}

/// Asks `action` of `repository`'s forge; returns the run a job's retry made where it is known
/// (GitLab's answer; GitHub's looked for once its new attempt has it), or the job asked of.
///
/// # Errors
///
/// Any of [`ForgeError`]'s ([`ForgeError::Forbidden`] where the account may not: its role, or
/// a token without the scope: GitLab's `api`, GitHub's `repo` or Actions' write);
/// [`ForgeError::NotFound`] where the forge has no such action (GitHub's for a job but a
/// re-run).
pub fn ci_act(
    account: &Account,
    repository: &ForgeRepository,
    action: CiAction,
    cancel: &CancelToken,
) -> Result<Option<u64>, ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitLab => gitlab::ci_act(account, repository, action, cancel),
        ForgeKind::GitHub => github::ci_act(account, repository, action, cancel),
    }
}

/// `jobs` grouped into stages by `key` (a group's key and its stage's name), in the order each
/// group first comes; each stage's status the most telling of its jobs' ([`CiStatus::weight`]).
fn grouped(jobs: Vec<Job>, mut key: impl FnMut(&Job) -> (String, String)) -> Vec<Stage> {
    let mut stages: Vec<Stage> = Vec::new();
    let mut by_key: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for job in jobs {
        let (group, name) = key(&job);
        let at = *by_key.entry(group).or_insert_with(|| {
            stages.push(Stage {
                name,
                status: CiStatus::Success,
                jobs: Vec::new(),
            });
            stages.len().saturating_sub(1)
        });
        if let Some(stage) = stages.get_mut(at) {
            if stage.jobs.is_empty() || job.status.weight() > stage.status.weight() {
                stage.status = job.status;
            }
            stage.jobs.push(job);
        }
    }
    stages
}

impl PipelineDetails {
    /// `workflow`'s word on its jobs laid over them (a GitHub run's, read from its file at its
    /// commit): each job's needs (by the jobs' names as the run shows them, a matrix's every
    /// one), whether it may fail (`continue-on-error`), and its stage, the workflow's job it
    /// is one of (named as the workflow names it). A job the workflow doesn't name is left as
    /// it was.
    pub fn with_workflow(&mut self, workflow: &Workflow) {
        workflow::lay_over(self, workflow);
    }
}

/// What can be asked of a pipeline, or of one of its jobs (each by its number).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiAction {
    /// Run again the pipeline's jobs that failed or were cancelled (those after them run as
    /// it goes on).
    RetryPipeline(u64),
    /// Run every job of a pipeline again: GitHub's re-run of a whole workflow run, a new
    /// attempt (GitLab has none: a retry runs those that failed).
    RerunPipeline(u64),
    /// Cancel the pipeline's jobs under way or waiting.
    CancelPipeline(u64),
    /// Run a job again: a new run in its place.
    RetryJob(u64),
    /// Cancel a job under way or waiting (GitLab's alone: GitHub cancels a whole run).
    CancelJob(u64),
    /// Start a manual job (GitLab's).
    PlayJob(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_read_however_gitlab_writes_them() {
        assert_eq!(CiStatus::parse("RUNNING"), CiStatus::Running);
        assert_eq!(CiStatus::parse("running"), CiStatus::Running);
        assert_eq!(
            CiStatus::parse("waiting-for-resource"),
            CiStatus::WaitingForResource
        );
        assert_eq!(
            CiStatus::parse("WAITING_FOR_CALLBACK"),
            CiStatus::WaitingForCallback
        );
        assert_eq!(CiStatus::parse("SUCCESS"), CiStatus::Success);
        assert_eq!(CiStatus::parse("whatever-next"), CiStatus::Unknown);
    }

    #[test]
    fn github_s_statuses_and_conclusions_are_read_into_gitlab_s() {
        let read = |status, conclusion| CiStatus::from_github(Some(status), conclusion);
        assert_eq!(read("queued", None), CiStatus::Pending);
        assert_eq!(read("in_progress", None), CiStatus::Running);
        assert_eq!(read("waiting", None), CiStatus::Manual);
        assert_eq!(read("completed", Some("success")), CiStatus::Success);
        assert_eq!(read("completed", Some("timed_out")), CiStatus::Failed);
        assert_eq!(read("completed", Some("cancelled")), CiStatus::Canceled);
        assert_eq!(read("completed", Some("skipped")), CiStatus::Skipped);
        assert_eq!(read("completed", Some("whatever")), CiStatus::Unknown);
        // A failure says most of a commit; one that passed least.
        assert!(CiStatus::Failed.weight() > CiStatus::Running.weight());
        assert!(CiStatus::Canceled.weight() > CiStatus::Skipped.weight());
        assert!(CiStatus::Skipped.weight() > CiStatus::Success.weight());
    }

    #[test]
    fn each_status_shows_as_one_of_five() {
        assert_eq!(CiStatus::Success.group(), CiGroup::Success);
        assert_eq!(CiStatus::Failed.group(), CiGroup::Failed);
        for status in [CiStatus::Created, CiStatus::Pending, CiStatus::Running] {
            assert_eq!(status.group(), CiGroup::Running);
        }
        assert_eq!(CiStatus::Manual.group(), CiGroup::Paused);
        assert_eq!(CiStatus::Scheduled.group(), CiGroup::Paused);
        for status in [CiStatus::Canceled, CiStatus::Skipped, CiStatus::Unknown] {
            assert_eq!(status.group(), CiGroup::Canceled);
        }
        // Waiting for someone stays waiting; waiting for its time will move.
        assert!(!CiStatus::Manual.is_moving());
        assert!(CiStatus::Scheduled.is_moving());
        assert!(CiStatus::Pending.is_moving());
        assert!(!CiStatus::Success.is_moving());
    }
}
