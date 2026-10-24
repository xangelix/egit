//! GitLab's pipelines, through its GraphQL API: a project's latest (the newest first, a page at
//! a time), each with its commit, status, and page; and one whole: its stages in order, each
//! with its jobs (the latest run of each, the runs retried before it beside), what can be done
//! to them, and whether the account may. Asking something of one is REST.

use egit_git::{CancelToken, ObjectId};
use serde::Deserialize;
use serde_json::json;

use super::{
    CiAction, CiStatus, EarlierRun, Job, JobActions, PAGE, Pipeline, PipelineActions,
    PipelineDetails, PipelineListing, Stage,
};
use crate::account::Account;
use crate::answer::{cancelled, graphql, not_found, parse_time, read_answer};
use crate::artifacts::{Artifact, gitlab_artifact};
use crate::error::ForgeError;
use crate::repository::ForgeRepository;

/// Reads `repository`'s latest pipelines, and says whether GitLab listed them at all.
///
/// The newest first, a page at a time (each handed to `found` as it lands), up to `limit` of
/// them.
pub(super) async fn pipelines(
    account: &Account,
    repository: &ForgeRepository,
    limit: u32,
    cancel: &CancelToken,
    found: &mut (dyn FnMut(Vec<Pipeline>) + Send),
) -> Result<PipelineListing, ForgeError> {
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
        )
        .await?;
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
pub(super) async fn pipeline_details(
    account: &Account,
    repository: &ForgeRepository,
    iid: u64,
    cancel: &CancelToken,
) -> Result<PipelineDetails, ForgeError> {
    let what = format!("{}'s pipeline {iid}", repository.path());
    let data: DetailsData = graphql(
        account,
        PIPELINE,
        &json!({ "path": repository.path(), "iid": iid.to_string() }),
        &what,
        cancel,
    )
    .await?;
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

#[derive(Deserialize)]
struct Created {
    id: Option<u64>,
}

/// Asks `action` of `repository`'s GitLab (REST: `POST …/pipelines/:id/retry`, `…/cancel`;
/// `…/jobs/:id/retry`, `…/cancel`, `…/play`); returns the run a job's retry made, or the job
/// asked of. Running every job of a pipeline again is GitHub's alone: GitLab's retry runs
/// those that failed.
pub(super) async fn ci_act(
    account: &Account,
    repository: &ForgeRepository,
    action: CiAction,
    cancel: &CancelToken,
) -> Result<Option<u64>, ForgeError> {
    let what = format!("{}'s {action:?}", repository.path());
    let (kind, id, verb) = match action {
        CiAction::RetryPipeline(id) | CiAction::RerunPipeline(id) => ("pipelines", id, "retry"),
        CiAction::CancelPipeline(id) => ("pipelines", id, "cancel"),
        CiAction::RetryJob(id) => ("jobs", id, "retry"),
        CiAction::CancelJob(id) => ("jobs", id, "cancel"),
        CiAction::PlayJob(id) => ("jobs", id, "play"),
    };
    let url = account.host().rest_url(&format!(
        "/projects/{}/{kind}/{id}/{verb}",
        crate::gitlab::encoded(repository.path())
    ));
    let answer = account.send(&url, Some(b"{}"), cancel).await?;
    let created: Created = read_answer(account, &answer, &what)?;
    Ok(created.id)
}

/// A pipeline GitLab made: its number within its project.
#[derive(Deserialize)]
struct Made {
    iid: Option<u64>,
}

/// Starts a pipeline of `repository` on `reference` (REST: `POST /projects/:id/pipeline`);
/// returns its number within the project.
pub(super) async fn start(
    account: &Account,
    repository: &ForgeRepository,
    reference: &str,
    cancel: &CancelToken,
) -> Result<Option<u64>, ForgeError> {
    let what = format!("{}'s new pipeline on {reference}", repository.path());
    let url = account.host().rest_url(&format!(
        "/projects/{}/pipeline",
        crate::gitlab::encoded(repository.path())
    ));
    let body = serde_json::json!({ "ref": reference }).to_string();
    let answer = account.send(&url, Some(body.as_bytes()), cancel).await?;
    let made: Made = read_answer(account, &answer, &what)?;
    Ok(made.iid)
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
    artifacts { nodes { name fileType size expireAt } } \
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
            name: None,
            number: id,
            series: 0,
            attempt: 1,
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
    artifacts: Option<Nodes<ArtifactNode>>,
}

/// A job's artifact, as GitLab's GraphQL API lists it (its size a big integer, written as text).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactNode {
    name: Option<String>,
    file_type: Option<String>,
    size: Option<serde_json::Value>,
    expire_at: Option<String>,
}

impl ArtifactNode {
    /// The artifact of job `job`, unless it is its log or the archive's metadata.
    fn artifact(self, job: u64) -> Option<Artifact> {
        let size = match &self.size {
            Some(serde_json::Value::Number(number)) => number.as_u64(),
            Some(serde_json::Value::String(text)) => text.parse().ok(),
            _ => None,
        };
        gitlab_artifact(
            job,
            (
                self.file_type.as_deref()?,
                self.name.as_deref().unwrap_or_default(),
                size,
                self.expire_at.as_deref().and_then(parse_time),
            ),
        )
    }
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
            actions: PipelineActions {
                retry: self.retryable,
                cancel: self.cancelable,
                rerun: false,
            },
            can_update: self
                .user_permissions
                .is_some_and(|permissions| permissions.update_pipeline),
            ordered_stages: true,
            workflow: None,
            artifacts: Vec::new(),
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
        let id = build_id(self.id.as_deref());
        let artifacts = id.map_or_else(Vec::new, |id| {
            self.artifacts
                .unwrap_or_default()
                .nodes
                .into_iter()
                .flatten()
                .filter_map(|artifact| artifact.artifact(id))
                .collect()
        });
        Job {
            id,
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
            artifacts,
        }
    }
}
