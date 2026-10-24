//! GitLab's API: a project's open merge requests (GraphQL, a page at a time), one merge
//! request's details (GraphQL), the project's default branch and the one it was forked from
//! (REST), who is signed in, and opening a merge request (REST).
//!
//! Every answer is read as `answer.rs` reads them. GitLab's GraphQL says a project that isn't
//! there (or isn't visible) by giving none; a draft is marked by its title, as GitLab marks it.

use egit_git::{CancelToken, ObjectId};
use serde::Deserialize;
use serde_json::json;

use crate::account::Account;
use crate::answer::{cancelled, graphql, not_found, parse_time, read_answer};
use crate::error::ForgeError;
use crate::model::{
    Check, CheckState, Mergeable, NewPullRequest, Opened, PullRequest, PullRequestDetails,
    PullRequestPage, RepositoryInfo, Review, ReviewDecision, ReviewState,
};
use crate::repository::ForgeRepository;

/// Merge requests a page: GitLab's largest.
const PAGE: u32 = 100;

/// Reads `repository`'s open merge requests, the most recently updated first, a page at a
/// time (each handed to `found` as it lands), up to `limit` of them.
pub(crate) async fn merge_requests(
    account: &Account,
    repository: &ForgeRepository,
    limit: u32,
    cancel: &CancelToken,
    mut found: impl FnMut(PullRequestPage),
) -> Result<(), ForgeError> {
    let mut after: Option<String> = None;
    let mut read = 0_u32;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let first = PAGE.min(limit.saturating_sub(read));
        if first == 0 {
            return Ok(());
        }
        let data: MergeRequestsData = graphql(
            account,
            MERGE_REQUESTS,
            &json!({ "path": repository.path(), "first": first, "after": after }),
            repository.path(),
            cancel,
        )
        .await?;
        let connection = data
            .project
            .ok_or_else(|| not_found(account, repository.full_name()))?
            .merge_requests
            .unwrap_or_default();
        let pulls: Vec<PullRequest> = connection
            .nodes
            .into_iter()
            .flatten()
            .filter_map(|node| node.into_pull_request(repository))
            .collect();
        read = read.saturating_add(u32::try_from(pulls.len()).unwrap_or(u32::MAX));
        let next = connection
            .page_info
            .has_next_page
            .then_some(connection.page_info.end_cursor)
            .flatten()
            .filter(|_| !pulls.is_empty());
        let more = next.is_some();
        after.clone_from(&next);
        found(PullRequestPage {
            pulls,
            total: connection.count,
            next,
        });
        if !more {
            return Ok(());
        }
    }
}

/// Reads merge request `number`'s details.
pub(crate) async fn merge_request_details(
    account: &Account,
    repository: &ForgeRepository,
    number: u32,
    cancel: &CancelToken,
) -> Result<PullRequestDetails, ForgeError> {
    let what = format!("{}!{number}", repository.path());
    let data: DetailsData = graphql(
        account,
        MERGE_REQUEST,
        &json!({ "path": repository.path(), "iid": number.to_string() }),
        &what,
        cancel,
    )
    .await?;
    let request = data
        .project
        .and_then(|project| project.merge_request)
        .ok_or_else(|| not_found(account, what))?;
    Ok(request.into_details(repository))
}

/// Reads `repository`'s default branch and the project it was forked from.
pub(crate) async fn project_info(
    account: &Account,
    repository: &ForgeRepository,
    cancel: &CancelToken,
) -> Result<RepositoryInfo, ForgeError> {
    let project = project(account, repository, cancel).await?;
    Ok(RepositoryInfo {
        default_branch: project.default_branch,
        parent: project.parent.and_then(|parent| {
            Some((
                ForgeRepository::new(repository.host().clone(), &parent.path_with_namespace?)?,
                parent.default_branch,
            ))
        }),
    })
}

/// Opens a merge request into `repository`: from the branch's own project (a fork's, when it
/// is one), a draft marked as GitLab marks one.
pub(crate) async fn create_merge_request(
    account: &Account,
    repository: &ForgeRepository,
    new: &NewPullRequest,
    cancel: &CancelToken,
) -> Result<Opened, ForgeError> {
    let mut title = new.title.clone();
    if new.draft && !is_marked_draft(&title) {
        title = format!("Draft: {title}");
    }
    let mut body = json!({
        "source_branch": new.head_branch,
        "target_branch": new.base_branch,
        "title": title,
        "description": new.body,
    });
    // From a fork, the request is made in the fork, aimed at the project's own id.
    let source = match &new.head {
        Some(fork) if !fork.is(repository) => {
            let target = project(account, repository, cancel).await?;
            if let Some(fields) = body.as_object_mut() {
                fields.insert("target_project_id".to_owned(), json!(target.id));
            }
            fork
        }
        _ => repository,
    };
    let url = account.host().rest_url(&format!(
        "/projects/{}/merge_requests",
        encoded(source.path())
    ));
    let answer = account
        .send(&url, Some(body.to_string().as_bytes()), cancel)
        .await?;
    let created: Created = read_answer(account, &answer, source.path())?;
    Ok(Opened {
        number: created.iid,
        url: created.web_url,
    })
}

/// Who the credential is (and checks the host accepts it).
pub(crate) async fn current_user(
    account: &Account,
    cancel: &CancelToken,
) -> Result<String, ForgeError> {
    let data: CurrentUserData =
        graphql(account, CURRENT_USER, &json!({}), "the account", cancel).await?;
    let user = data.current_user.ok_or_else(|| ForgeError::Refused {
        host: account.host().name().to_owned(),
    })?;
    account.note_login(&user.username);
    Ok(user.username)
}

/// The name the GitLab's ssh is on, as its projects' ssh addresses give it (one of the
/// account's own projects, else any it may see): its `gitlab_shell_ssh_host`, which may not be
/// its web pages' name. `None` when it shows the account no project.
pub(crate) async fn ssh_host(
    account: &Account,
    cancel: &CancelToken,
) -> Result<Option<String>, ForgeError> {
    let data: SshHostData = graphql(account, SSH_HOST, &json!({}), "a project", cancel).await?;
    let address = [data.mine, data.any]
        .into_iter()
        .flatten()
        .flat_map(|projects| projects.nodes)
        .flatten()
        .find_map(|project| project.ssh_url_to_repo);
    Ok(address.and_then(|address| crate::repository::ssh_host_of(&address)))
}

/// A project, read through REST.
async fn project(
    account: &Account,
    repository: &ForgeRepository,
    cancel: &CancelToken,
) -> Result<Project, ForgeError> {
    let url = account
        .host()
        .rest_url(&format!("/projects/{}", encoded(repository.path())));
    let answer = account.send(&url, None, cancel).await?;
    read_answer(account, &answer, repository.path())
}

/// A project's path as one REST path segment: its slashes encoded (its parts hold nothing else
/// that needs it).
pub(crate) fn encoded(path: &str) -> String {
    path.replace('/', "%2F")
}

/// Whether a title marks its merge request a draft, as GitLab reads it.
fn is_marked_draft(title: &str) -> bool {
    let lower = title.trim_start().to_ascii_lowercase();
    ["draft:", "[draft]", "(draft)"]
        .iter()
        .any(|mark| lower.starts_with(mark))
}

/// A pipeline's status, as a check state.
fn pipeline_state(status: &str) -> CheckState {
    match status {
        "SUCCESS" => CheckState::Success,
        "FAILED" => CheckState::Failure,
        "CANCELED" | "CANCELING" | "SKIPPED" | "MANUAL" => CheckState::Neutral,
        _ => CheckState::Pending,
    }
}

/// A job's status, as a check state: a failure it is allowed is neither pass nor fail.
fn job_state(status: Option<&str>, allow_failure: bool) -> CheckState {
    match status {
        Some("FAILED") if allow_failure => CheckState::Neutral,
        Some(status) => pipeline_state(status),
        None => CheckState::Neutral,
    }
}

const MERGE_REQUESTS: &str = "query MergeRequests($path: ID!, $first: Int!, $after: String) { \
    project(fullPath: $path) { mergeRequests(state: opened, sort: UPDATED_DESC, first: $first, \
    after: $after) { count pageInfo { hasNextPage endCursor } nodes { iid title draft webUrl \
    updatedAt sourceBranch targetBranch diffHeadSha approved author { username } sourceProject \
    { fullPath } headPipeline { status } approvedBy { nodes { username } } } } } }";

const MERGE_REQUEST: &str = "query MergeRequest($path: ID!, $iid: String!) { project(fullPath: \
    $path) { mergeRequest(iid: $iid) { description commitCount conflicts diffStatsSummary { \
    additions deletions fileCount } labels(first: 50) { nodes { title } } approvedBy { nodes { \
    username } } headPipeline { jobs(first: 100, retried: false) { nodes { name status \
    allowFailure webPath } } } } } }";

const CURRENT_USER: &str = "query CurrentUser { currentUser { username } }";

const SSH_HOST: &str = "query SshHost { mine: projects(first: 1, membership: true) { nodes { \
    sshUrlToRepo } } any: projects(first: 1) { nodes { sshUrlToRepo } } }";

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
struct Username {
    username: String,
}

#[derive(Deserialize)]
struct MergeRequestsData {
    project: Option<MergeRequestsProject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeRequestsProject {
    merge_requests: Option<MergeRequestConnection>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeRequestConnection {
    #[serde(default)]
    count: u32,
    #[serde(default)]
    page_info: PageInfo,
    #[serde(default)]
    nodes: Vec<Option<MergeRequestNode>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeRequestNode {
    iid: String,
    title: String,
    draft: bool,
    web_url: Option<String>,
    updated_at: String,
    source_branch: String,
    target_branch: String,
    diff_head_sha: Option<String>,
    approved: bool,
    author: Option<Username>,
    source_project: Option<FullPath>,
    head_pipeline: Option<PipelineStatus>,
    #[serde(default)]
    approved_by: Option<Nodes<Username>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FullPath {
    full_path: String,
}

#[derive(Deserialize)]
struct PipelineStatus {
    status: String,
}

impl MergeRequestNode {
    /// The merge request, unless its number isn't one.
    fn into_pull_request(self, repository: &ForgeRepository) -> Option<PullRequest> {
        let number: u32 = self.iid.parse().ok()?;
        let source = self.source_project.map(|project| project.full_path);
        // A deleted fork has no project left: it was someone else's all the same.
        let cross_repository = source
            .as_deref()
            .is_none_or(|source| !repository.is_named(source));
        let approvals = self
            .approved_by
            .map_or(0, |approved| approved.nodes.into_iter().flatten().count());
        let review = match (self.approved, approvals) {
            (false, _) => ReviewDecision::Required,
            (true, 0) => ReviewDecision::None,
            (true, _) => ReviewDecision::Approved,
        };
        Some(PullRequest {
            number,
            title: self.title,
            author: self
                .author
                .map_or_else(|| "ghost".to_owned(), |author| author.username),
            draft: self.draft,
            head_branch: self.source_branch,
            head_repository: source,
            cross_repository,
            base_branch: self.target_branch,
            head_commit: self
                .diff_head_sha
                .and_then(|sha| ObjectId::from_hex(sha.as_bytes())),
            review,
            checks: self.head_pipeline.map_or(CheckState::None, |pipeline| {
                pipeline_state(&pipeline.status)
            }),
            updated: parse_time(&self.updated_at).unwrap_or(0),
            url: self
                .web_url
                .unwrap_or_else(|| repository.pull_request_url(number)),
        })
    }
}

#[derive(Deserialize)]
struct DetailsData {
    project: Option<DetailsProject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsProject {
    merge_request: Option<DetailsNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsNode {
    description: Option<String>,
    commit_count: Option<u32>,
    conflicts: bool,
    diff_stats_summary: Option<DiffStats>,
    #[serde(default)]
    labels: Option<Nodes<Title>>,
    #[serde(default)]
    approved_by: Option<Nodes<Username>>,
    head_pipeline: Option<PipelineJobs>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffStats {
    additions: u32,
    deletions: u32,
    file_count: u32,
}

#[derive(Deserialize)]
struct Title {
    title: String,
}

#[derive(Deserialize)]
struct PipelineJobs {
    #[serde(default)]
    jobs: Option<Nodes<JobNode>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobNode {
    name: Option<String>,
    status: Option<String>,
    #[serde(default)]
    allow_failure: bool,
    web_path: Option<String>,
}

impl DetailsNode {
    fn into_details(self, repository: &ForgeRepository) -> PullRequestDetails {
        let host = repository.host();
        let stats = self.diff_stats_summary;
        PullRequestDetails {
            body: self.description.unwrap_or_default(),
            additions: stats.as_ref().map_or(0, |stats| stats.additions),
            deletions: stats.as_ref().map_or(0, |stats| stats.deletions),
            changed_files: stats.as_ref().map_or(0, |stats| stats.file_count),
            commits: self.commit_count.unwrap_or(0),
            mergeable: if self.conflicts {
                Mergeable::Conflicting
            } else {
                Mergeable::Yes
            },
            checks: self
                .head_pipeline
                .and_then(|pipeline| pipeline.jobs)
                .unwrap_or_default()
                .nodes
                .into_iter()
                .flatten()
                .map(|job| Check {
                    name: job.name.unwrap_or_default(),
                    state: job_state(job.status.as_deref(), job.allow_failure),
                    url: job.web_path.map(|path| host.web_url(&path)),
                })
                .collect(),
            reviews: self
                .approved_by
                .unwrap_or_default()
                .nodes
                .into_iter()
                .flatten()
                .map(|user| Review {
                    author: user.username,
                    state: ReviewState::Approved,
                })
                .collect(),
            labels: self
                .labels
                .unwrap_or_default()
                .nodes
                .into_iter()
                .flatten()
                .map(|label| label.title)
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct SshHostData {
    mine: Option<Nodes<SshAddress>>,
    any: Option<Nodes<SshAddress>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshAddress {
    ssh_url_to_repo: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentUserData {
    current_user: Option<Username>,
}

#[derive(Deserialize)]
struct Project {
    id: u64,
    default_branch: Option<String>,
    #[serde(rename = "forked_from_project")]
    parent: Option<ParentProject>,
}

#[derive(Deserialize)]
struct ParentProject {
    path_with_namespace: Option<String>,
    default_branch: Option<String>,
}

#[derive(Deserialize)]
struct Created {
    iid: u32,
    web_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipelines_and_jobs_have_states() {
        assert_eq!(pipeline_state("SUCCESS"), CheckState::Success);
        assert_eq!(pipeline_state("FAILED"), CheckState::Failure);
        assert_eq!(pipeline_state("RUNNING"), CheckState::Pending);
        assert_eq!(pipeline_state("WAITING_FOR_RESOURCE"), CheckState::Pending);
        assert_eq!(pipeline_state("SKIPPED"), CheckState::Neutral);
        assert_eq!(job_state(Some("FAILED"), true), CheckState::Neutral);
        assert_eq!(job_state(Some("FAILED"), false), CheckState::Failure);
    }

    #[test]
    fn drafts_are_marked_as_gitlab_marks_them() {
        assert!(is_marked_draft("Draft: something"));
        assert!(is_marked_draft("[draft] something"));
        assert!(is_marked_draft("  (Draft) something"));
        assert!(!is_marked_draft("Drafting the plan"));
    }

    #[test]
    fn a_project_path_is_one_segment() {
        assert_eq!(encoded("group/sub/project"), "group%2Fsub%2Fproject");
    }
}
