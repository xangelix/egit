//! CI/CD pipelines: GitLab's, read through its GraphQL API. A project's latest pipelines (the
//! newest first, a page at a time), each with its commit, status, and page; and one pipeline
//! whole: its stages in order, each with its jobs (the latest run of each, retried ones left
//! out) and their pages. GitHub has none here: its Actions' runs are a pull request's checks.

use egit_git::{CancelToken, ObjectId};
use serde::Deserialize;
use serde_json::json;

use crate::account::Account;
use crate::answer::{cancelled, graphql, not_found, parse_time, read_answer};
use crate::error::ForgeError;
use crate::host::ForgeKind;
use crate::repository::ForgeRepository;

/// Pipelines a page: GitLab's largest.
const PAGE: u32 = 100;

/// A pipeline's, a stage's, or a job's status, as GitLab names it.
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

/// A pipeline, as a list of them shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    /// GitLab's number for it, which its page carries (`#123456`).
    pub id: u64,
    /// Its number within its project, which it is read again by.
    pub iid: u64,
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
    /// later). Of a commit's pipelines, the newest says how it stands.
    #[must_use]
    pub fn is_newer_than(&self, other: &Self) -> bool {
        (self.created, self.id) > (other.created, other.id)
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
    /// What made it run (`push`, `merge_request_event`, `schedule`, `web`, …).
    pub source: Option<String>,
    /// Its stages, in the order they run.
    pub stages: Vec<Stage>,
    /// Whether it has jobs to retry (failed, cancelled).
    pub retryable: bool,
    /// Whether it has jobs to cancel (under way, waiting).
    pub cancelable: bool,
    /// Whether the account may retry and cancel its jobs (its role on the project).
    pub can_update: bool,
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
    /// waits for every job of the stages before it.
    pub needs: Option<Vec<String>>,
    /// What can be done to it, as GitLab says.
    pub actions: JobActions,
    /// Whether the account may do those (its role on the project).
    pub can_update: bool,
    /// Its earlier runs in this place of the pipeline (retried since), the newest first.
    pub earlier: Vec<EarlierRun>,
}

/// What can be done to a job, as GitLab says.
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

/// Whether repositories on `kind` have pipelines egit reads.
#[must_use]
pub const fn has_pipelines(kind: ForgeKind) -> bool {
    matches!(kind, ForgeKind::GitLab)
}

/// Whether GitLab listed a project's pipelines to the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineListing {
    /// Listed (none, perhaps: the project has run none).
    Listed,
    /// Hidden: GitLab gives no list (CI/CD turned off for the project, or the account's role
    /// may not see its pipelines), or the forge has none (GitHub).
    Hidden,
}

/// Reads `repository`'s latest pipelines, and says whether GitLab listed them at all.
///
/// The newest first, a page at a time (each handed to `found` as it lands), up to `limit` of
/// them. Nothing on a forge without pipelines.
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
    if !has_pipelines(repository.host().kind()) {
        return Ok(PipelineListing::Hidden);
    }
    let mut after: Option<String> = None;
    let mut read = 0_u32;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let first = PAGE.min(limit.saturating_sub(read));
        if first == 0 {
            return Ok(PipelineListing::Listed);
        }
        let data: PipelinesData = graphql(
            account,
            PIPELINES,
            &json!({ "path": repository.path(), "first": first, "after": after }),
            repository.path(),
            cancel,
        )?;
        // A project whose pipelines this account can't see (or that has CI/CD off) has none.
        let Some(connection) = data
            .project
            .ok_or_else(|| not_found(account, repository.full_name()))?
            .pipelines
        else {
            tracing::debug!(
                project = %repository.full_name(),
                "GitLab lists no pipelines to this account"
            );
            return Ok(PipelineListing::Hidden);
        };
        let host = repository.host();
        let nodes: Vec<PipelineNode> = connection.nodes.into_iter().flatten().collect();
        let given = nodes.len();
        let page: Vec<Pipeline> = nodes
            .into_iter()
            .filter_map(|node| node.into_pipeline(host, repository))
            .collect();
        if page.len() < given {
            tracing::debug!(
                project = %repository.full_name(),
                unread = given.saturating_sub(page.len()),
                "pipelines without a number or a commit left out"
            );
        }
        read = read.saturating_add(u32::try_from(page.len()).unwrap_or(u32::MAX));
        let next = connection
            .page_info
            .has_next_page
            .then_some(connection.page_info.end_cursor)
            .flatten()
            .filter(|_| !page.is_empty());
        let more = next.is_some();
        after.clone_from(&next);
        found(page);
        if !more {
            return Ok(PipelineListing::Listed);
        }
    }
}

/// Reads pipeline `iid` of `repository` whole.
///
/// # Errors
///
/// Any of [`ForgeError`]'s; [`ForgeError::NotFound`] on a forge without pipelines.
pub fn pipeline_details(
    account: &Account,
    repository: &ForgeRepository,
    iid: u64,
    cancel: &CancelToken,
) -> Result<PipelineDetails, ForgeError> {
    let what = format!("{}'s pipeline {iid}", repository.path());
    if !has_pipelines(repository.host().kind()) {
        return Err(not_found(account, what));
    }
    let data: DetailsData = graphql(
        account,
        PIPELINE,
        &json!({ "path": repository.path(), "iid": iid.to_string() }),
        &what,
        cancel,
    )?;
    let node = data
        .project
        .and_then(|project| project.pipeline)
        .ok_or_else(|| not_found(account, what.clone()))?;
    node.into_details(repository)
        .ok_or_else(|| ForgeError::Malformed {
            host: account.host().name().to_owned(),
            detail: format!("{what} names no commit"),
        })
}

/// What can be asked of a pipeline, or of one of its jobs (each by its number).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiAction {
    /// Run again the pipeline's jobs that failed or were cancelled (those after them run as
    /// it goes on).
    RetryPipeline(u64),
    /// Cancel the pipeline's jobs under way or waiting.
    CancelPipeline(u64),
    /// Run a job again: a new run in its place.
    RetryJob(u64),
    /// Cancel a job under way or waiting.
    CancelJob(u64),
    /// Start a manual job.
    PlayJob(u64),
}

#[derive(Deserialize)]
struct Created {
    id: Option<u64>,
}

/// Asks `action` of `repository`'s GitLab (REST: `POST …/pipelines/:id/retry`, `…/cancel`;
/// `…/jobs/:id/retry`, `…/cancel`, `…/play`); returns the run a job's retry made, or the job
/// asked of.
///
/// # Errors
///
/// Any of [`ForgeError`]'s ([`ForgeError::Forbidden`] where the account may not: its role, or
/// a token without the `api` scope); [`ForgeError::NotFound`] on a forge without pipelines.
pub fn ci_act(
    account: &Account,
    repository: &ForgeRepository,
    action: CiAction,
    cancel: &CancelToken,
) -> Result<Option<u64>, ForgeError> {
    let what = format!("{}'s {action:?}", repository.path());
    if !has_pipelines(repository.host().kind()) {
        return Err(not_found(account, what));
    }
    let (kind, id, verb) = match action {
        CiAction::RetryPipeline(id) => ("pipelines", id, "retry"),
        CiAction::CancelPipeline(id) => ("pipelines", id, "cancel"),
        CiAction::RetryJob(id) => ("jobs", id, "retry"),
        CiAction::CancelJob(id) => ("jobs", id, "cancel"),
        CiAction::PlayJob(id) => ("jobs", id, "play"),
    };
    let url = account.host().rest_url(&format!(
        "/projects/{}/{kind}/{id}/{verb}",
        crate::gitlab::encoded(repository.path())
    ));
    let answer = account.send(&url, Some(b"{}"), cancel)?;
    let created: Created = read_answer(account, &answer, &what)?;
    Ok(created.id)
}

const PIPELINES: &str = "query Pipelines($path: ID!, $first: Int!, $after: String) { \
    project(fullPath: $path) { pipelines(first: $first, after: $after) { pageInfo { \
    hasNextPage endCursor } nodes { id iid sha ref status warnings createdAt path } } } }";

const PIPELINE: &str = "query Pipeline($path: ID!, $iid: ID!) { project(fullPath: $path) { \
    pipeline(iid: $iid) { id iid sha ref status warnings createdAt startedAt finishedAt \
    duration path source retryable cancelable userPermissions { updatePipeline } \
    user { username } retried: jobs(retried: true) { nodes { id name status duration startedAt \
    webPath stage { name } } } stages { nodes { name status jobs { nodes { id name status \
    allowFailure manualJob webPath duration startedAt schedulingType retryable cancelable \
    playable userPermissions { updateBuild } needs { nodes { name } } downstreamPipeline { path } \
    } } } } } } }";

#[derive(Deserialize)]
struct Nodes<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<T>>,
}

impl<T> Default for Nodes<T> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

#[derive(Deserialize)]
struct PipelinesData {
    project: Option<PipelinesProject>,
}

#[derive(Deserialize)]
struct PipelinesProject {
    pipelines: Option<PipelineConnection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineConnection {
    #[serde(default)]
    page_info: PageInfo,
    #[serde(default)]
    nodes: Vec<Option<PipelineNode>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineNode {
    id: Option<String>,
    iid: String,
    sha: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    status: String,
    #[serde(default)]
    warnings: bool,
    created_at: Option<String>,
    path: Option<String>,
}

impl PipelineNode {
    /// The pipeline, unless its numbers or commit aren't ones.
    fn into_pipeline(
        self,
        host: &crate::host::ForgeHost,
        repository: &ForgeRepository,
    ) -> Option<Pipeline> {
        let iid: u64 = self.iid.parse().ok()?;
        // A global id, `gid://gitlab/Ci::Pipeline/123456`.
        let id = self
            .id
            .as_deref()
            .and_then(|id| id.rsplit('/').next())
            .and_then(|id| id.parse().ok())
            .unwrap_or(iid);
        let commit = ObjectId::from_hex(self.sha?.as_bytes())?;
        let url = self.path.map_or_else(
            || format!("{}/-/pipelines/{id}", repository.web_url()),
            |path| host.web_url(&path),
        );
        Some(Pipeline {
            id,
            iid,
            commit,
            status: CiStatus::parse(&self.status),
            warnings: self.warnings,
            git_ref: self.git_ref,
            created: self.created_at.as_deref().and_then(parse_time).unwrap_or(0),
            url,
        })
    }
}

#[derive(Deserialize)]
struct DetailsData {
    project: Option<DetailsProject>,
}

#[derive(Deserialize)]
struct DetailsProject {
    pipeline: Option<DetailsNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsNode {
    #[serde(flatten)]
    pipeline: PipelineNode,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration: Option<u64>,
    source: Option<String>,
    user: Option<Username>,
    stages: Option<Nodes<StageNode>>,
    #[serde(default)]
    retryable: bool,
    #[serde(default)]
    cancelable: bool,
    user_permissions: Option<Permissions>,
    retried: Option<Nodes<RetriedNode>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Permissions {
    #[serde(default)]
    update_pipeline: bool,
    #[serde(default)]
    update_build: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetriedNode {
    id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    duration: Option<u64>,
    started_at: Option<String>,
    web_path: Option<String>,
    stage: Option<NameOnly>,
}

#[derive(Deserialize)]
struct NameOnly {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Username {
    username: String,
}

#[derive(Deserialize)]
struct StageNode {
    name: Option<String>,
    status: Option<String>,
    jobs: Option<Nodes<JobNode>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobNode {
    id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    #[serde(default)]
    allow_failure: bool,
    #[serde(default)]
    manual_job: Option<bool>,
    web_path: Option<String>,
    duration: Option<u64>,
    started_at: Option<String>,
    downstream_pipeline: Option<PathOnly>,
    scheduling_type: Option<String>,
    #[serde(flatten)]
    actions: JobActions,
    user_permissions: Option<Permissions>,
    needs: Option<Nodes<NameOnly>>,
}

/// A job's number, from its global id (`gid://gitlab/Ci::Build/123`; a trigger's is a
/// `Ci::Bridge`, without a log).
fn build_id(id: Option<&str>) -> Option<u64> {
    id?.strip_prefix("gid://gitlab/Ci::Build/")?.parse().ok()
}

#[derive(Deserialize)]
struct PathOnly {
    path: Option<String>,
}

impl DetailsNode {
    /// The details, unless its numbers or commit aren't ones.
    fn into_details(self, repository: &ForgeRepository) -> Option<PipelineDetails> {
        let host = repository.host();
        let pipeline = self.pipeline.into_pipeline(host, repository)?;
        let retried = earlier_runs(self.retried, host);
        let stages = self
            .stages
            .unwrap_or_default()
            .nodes
            .into_iter()
            .flatten()
            .map(|stage| {
                let name = stage.name.unwrap_or_default();
                Stage {
                    status: stage
                        .status
                        .as_deref()
                        .map_or(CiStatus::Unknown, CiStatus::parse),
                    jobs: stage
                        .jobs
                        .unwrap_or_default()
                        .nodes
                        .into_iter()
                        .flatten()
                        .map(|job| job.into_job(host, (Some(name.as_str()), &retried)))
                        .collect(),
                    name,
                }
            })
            .collect();
        Some(PipelineDetails {
            pipeline,
            started: self.started_at.as_deref().and_then(parse_time),
            finished: self.finished_at.as_deref().and_then(parse_time),
            duration: self.duration,
            user: self.user.map(|user| user.username),
            source: self.source,
            stages,
            retryable: self.retryable,
            cancelable: self.cancelable,
            can_update: self
                .user_permissions
                .is_some_and(|permissions| permissions.update_pipeline),
        })
    }
}

/// A pipeline's retried runs: each with its stage's and job's names, the newest first.
fn earlier_runs(
    retried: Option<Nodes<RetriedNode>>,
    host: &crate::host::ForgeHost,
) -> Vec<(String, String, EarlierRun)> {
    let mut runs: Vec<(String, String, EarlierRun)> = retried
        .unwrap_or_default()
        .nodes
        .into_iter()
        .flatten()
        .filter_map(|run| {
            let id = build_id(run.id.as_deref())?;
            Some((
                run.stage.and_then(|stage| stage.name).unwrap_or_default(),
                run.name.unwrap_or_default(),
                EarlierRun {
                    id,
                    status: run
                        .status
                        .as_deref()
                        .map_or(CiStatus::Unknown, CiStatus::parse),
                    duration: run.duration,
                    started: run.started_at.as_deref().and_then(parse_time),
                    url: run.web_path.map(|path| host.web_url(&path)),
                },
            ))
        })
        .collect();
    runs.sort_by_key(|(_, _, run)| std::cmp::Reverse(run.id));
    runs
}

impl JobNode {
    /// The job, of stage `stage`, with its earlier runs among `retried`.
    fn into_job(
        self,
        host: &crate::host::ForgeHost,
        (stage, retried): (Option<&str>, &[(String, String, EarlierRun)]),
    ) -> Job {
        let name = self.name.unwrap_or_default();
        let earlier = retried
            .iter()
            .filter(|(stage_of, named, _)| *named == name && stage == Some(stage_of.as_str()))
            .map(|(_, _, run)| run.clone())
            .collect();
        // Scheduled by its needs (`dag`), or by the stages before it.
        let needs = (self.scheduling_type.as_deref() == Some("dag")).then(|| {
            self.needs
                .unwrap_or_default()
                .nodes
                .into_iter()
                .flatten()
                .filter_map(|need| need.name)
                .collect()
        });
        Job {
            id: build_id(self.id.as_deref()),
            name,
            status: self
                .status
                .as_deref()
                .map_or(CiStatus::Unknown, CiStatus::parse),
            allow_failure: self.allow_failure,
            manual: self.manual_job.unwrap_or(false),
            duration: self.duration,
            started: self.started_at.as_deref().and_then(parse_time),
            // A trigger job's page is its downstream pipeline's.
            url: self
                .web_path
                .or_else(|| self.downstream_pipeline.and_then(|down| down.path))
                .map(|path| host.web_url(&path)),
            needs,
            actions: self.actions,
            can_update: self
                .user_permissions
                .is_some_and(|permissions| permissions.update_build),
            earlier,
        }
    }
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
