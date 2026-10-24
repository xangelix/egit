//! GitHub's API: a repository's open pull requests (GraphQL, a page at a time), one pull
//! request's details, the repository's default branch and the one it was forked from, who is
//! signed in, and opening a pull request (REST).
//!
//! Every answer is read as `answer.rs` reads them.

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

/// Pull requests a page.
const PAGE: u32 = 100;

/// Reads `repository`'s open pull requests, the most recently updated first, a page at a time
/// (each handed to `found` as it lands), up to `limit` of them.
///
/// # Errors
///
/// Any of [`ForgeError`]'s, from the page that failed (the pages before it were handed over).
pub(crate) async fn pull_requests(
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
        let data: PullRequestsData = graphql(
            account,
            PULL_REQUESTS,
            &json!({
                "owner": repository.owner(),
                "name": repository.name(),
                "first": first,
                "after": after,
            }),
            &repository.full_name(),
            cancel,
        )
        .await?;
        let connection = data
            .repository
            .ok_or_else(|| not_found(account, repository.full_name()))?
            .pull_requests;
        let pulls: Vec<PullRequest> = connection
            .nodes
            .into_iter()
            .flatten()
            .map(PullRequestNode::into_pull_request)
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
            total: connection.total_count,
            next,
        });
        if !more {
            return Ok(());
        }
    }
}

/// Reads pull request `number`'s details.
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub(crate) async fn pull_request_details(
    account: &Account,
    repository: &ForgeRepository,
    number: u32,
    cancel: &CancelToken,
) -> Result<PullRequestDetails, ForgeError> {
    let what = format!("{}#{number}", repository.full_name());
    let data: DetailsData = graphql(
        account,
        PULL_REQUEST,
        &json!({
            "owner": repository.owner(),
            "name": repository.name(),
            "number": number,
        }),
        &what,
        cancel,
    )
    .await?;
    let pull = data
        .repository
        .and_then(|repository| repository.pull_request)
        .ok_or_else(|| not_found(account, what))?;
    Ok(pull.into_details())
}

/// Reads `repository`'s default branch and the repository it was forked from.
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub(crate) async fn repository_info(
    account: &Account,
    repository: &ForgeRepository,
    cancel: &CancelToken,
) -> Result<RepositoryInfo, ForgeError> {
    let data: RepositoryData = graphql(
        account,
        REPOSITORY,
        &json!({ "owner": repository.owner(), "name": repository.name() }),
        &repository.full_name(),
        cancel,
    )
    .await?;
    let found = data
        .repository
        .ok_or_else(|| not_found(account, repository.full_name()))?;
    Ok(RepositoryInfo {
        default_branch: found.default_branch_ref.map(|branch| branch.name),
        parent: found.parent.and_then(|parent| {
            let path = format!("{}/{}", parent.owner.login, parent.name);
            Some((
                ForgeRepository::new(repository.host().clone(), &path)?,
                parent.default_branch_ref.map(|branch| branch.name),
            ))
        }),
    })
}

/// Opens a pull request into `repository`.
///
/// # Errors
///
/// Any of [`ForgeError`]'s; [`ForgeError::Status`] with GitHub's reason when it refuses (one is
/// already open for the branch, nothing to merge, …).
pub(crate) async fn create_pull_request(
    account: &Account,
    repository: &ForgeRepository,
    new: &NewPullRequest,
    cancel: &CancelToken,
) -> Result<Opened, ForgeError> {
    let head = new.head.as_ref().map_or_else(
        || new.head_branch.clone(),
        |fork| format!("{}:{}", fork.owner(), new.head_branch),
    );
    let body = json!({
        "title": new.title,
        "body": new.body,
        "head": head,
        "base": new.base_branch,
        "draft": new.draft,
    })
    .to_string();
    let url = account.host().rest_url(&format!(
        "/repos/{}/{}/pulls",
        repository.owner(),
        repository.name()
    ));
    let answer = account.send(&url, Some(body.as_bytes()), cancel).await?;
    let created: Created = read_answer(account, &answer, &repository.full_name())?;
    Ok(Opened {
        number: created.number,
        url: created.html_url,
    })
}

/// Who the credential is (and checks the host accepts it).
pub(crate) async fn viewer(account: &Account, cancel: &CancelToken) -> Result<String, ForgeError> {
    let data: ViewerData = graphql(account, VIEWER, &json!({}), "the account", cancel).await?;
    account.note_login(&data.viewer.login);
    Ok(data.viewer.login)
}

const PULL_REQUESTS: &str = "query PullRequests($owner: String!, $name: String!, $first: Int!, \
    $after: String) { repository(owner: $owner, name: $name) { pullRequests(states: OPEN, \
    first: $first, after: $after, orderBy: {field: UPDATED_AT, direction: DESC}) { totalCount \
    pageInfo { hasNextPage endCursor } nodes { number title url isDraft updatedAt \
    isCrossRepository author { login } baseRefName headRefName headRefOid headRepository { \
    nameWithOwner } reviewDecision commits(last: 1) { nodes { commit { statusCheckRollup { \
    state } } } } } } } }";

const PULL_REQUEST: &str = "query PullRequest($owner: String!, $name: String!, $number: Int!) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { body additions \
    deletions changedFiles mergeable commits(last: 1) { totalCount nodes { commit { \
    statusCheckRollup { contexts(first: 100) { nodes { __typename ... on CheckRun { name \
    conclusion status detailsUrl } ... on StatusContext { context state targetUrl } } } } } } } \
    latestOpinionatedReviews(first: 50) { nodes { author { login } state } } labels(first: 50) \
    { nodes { name } } } } }";

const REPOSITORY: &str = "query Repository($owner: String!, $name: String!) { \
    repository(owner: $owner, name: $name) { defaultBranchRef { name } parent { name owner { \
    login } defaultBranchRef { name } } } }";

const VIEWER: &str = "query Viewer { viewer { login } }";

#[derive(Deserialize)]
struct Created {
    number: u32,
    html_url: String,
}

#[derive(Deserialize)]
struct ViewerData {
    viewer: Login,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct Nodes<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<T>>,
}

#[derive(Deserialize)]
struct BranchRef {
    name: String,
}

#[derive(Deserialize)]
struct PullRequestsData {
    repository: Option<PullRequestsRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestsRepository {
    pull_requests: PullRequestConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestConnection {
    total_count: u32,
    page_info: PageInfo,
    #[serde(default)]
    nodes: Vec<Option<PullRequestNode>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestNode {
    number: u32,
    title: String,
    url: String,
    is_draft: bool,
    updated_at: String,
    is_cross_repository: bool,
    author: Option<Login>,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: Option<String>,
    head_repository: Option<NameWithOwner>,
    review_decision: Option<String>,
    commits: Nodes<CommitNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: RollupCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollupCommit {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    state: String,
}

impl PullRequestNode {
    fn into_pull_request(self) -> PullRequest {
        let checks = self
            .commits
            .nodes
            .into_iter()
            .flatten()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .map_or(CheckState::None, |rollup| status_state(&rollup.state));
        PullRequest {
            number: self.number,
            title: self.title,
            author: self
                .author
                .map_or_else(|| "ghost".to_owned(), |author| author.login),
            draft: self.is_draft,
            head_branch: self.head_ref_name,
            head_repository: self.head_repository.map(|head| head.name_with_owner),
            cross_repository: self.is_cross_repository,
            base_branch: self.base_ref_name,
            head_commit: self
                .head_ref_oid
                .and_then(|oid| ObjectId::from_hex(oid.as_bytes())),
            review: match self.review_decision.as_deref() {
                Some("APPROVED") => ReviewDecision::Approved,
                Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
                Some("REVIEW_REQUIRED") => ReviewDecision::Required,
                _ => ReviewDecision::None,
            },
            checks,
            updated: parse_time(&self.updated_at).unwrap_or(0),
            url: self.url,
        }
    }
}

/// A status's (or a whole commit's) state.
fn status_state(state: &str) -> CheckState {
    match state {
        "SUCCESS" => CheckState::Success,
        "FAILURE" | "ERROR" => CheckState::Failure,
        "PENDING" | "EXPECTED" => CheckState::Pending,
        _ => CheckState::Neutral,
    }
}

#[derive(Deserialize)]
struct DetailsData {
    repository: Option<DetailsRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsRepository {
    pull_request: Option<DetailsNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsNode {
    #[serde(default)]
    body: String,
    additions: u32,
    deletions: u32,
    changed_files: u32,
    mergeable: Option<String>,
    commits: DetailsCommits,
    latest_opinionated_reviews: Nodes<ReviewNode>,
    labels: Nodes<BranchRef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsCommits {
    total_count: u32,
    #[serde(default)]
    nodes: Vec<Option<ContextsCommitNode>>,
}

#[derive(Deserialize)]
struct ContextsCommitNode {
    commit: ContextsCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextsCommit {
    status_check_rollup: Option<ContextsRollup>,
}

#[derive(Deserialize)]
struct ContextsRollup {
    contexts: Nodes<ContextNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextNode {
    #[serde(rename = "__typename")]
    kind: String,
    // A check run's.
    name: Option<String>,
    conclusion: Option<String>,
    status: Option<String>,
    details_url: Option<String>,
    // A status's.
    context: Option<String>,
    state: Option<String>,
    target_url: Option<String>,
}

#[derive(Deserialize)]
struct ReviewNode {
    author: Option<Login>,
    state: String,
}

impl DetailsNode {
    fn into_details(self) -> PullRequestDetails {
        let checks = self
            .commits
            .nodes
            .into_iter()
            .flatten()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .map(|rollup| {
                rollup
                    .contexts
                    .nodes
                    .into_iter()
                    .flatten()
                    .map(ContextNode::into_check)
                    .collect()
            })
            .unwrap_or_default();
        let reviews = self
            .latest_opinionated_reviews
            .nodes
            .into_iter()
            .flatten()
            .filter_map(|review| {
                let state = match review.state.as_str() {
                    "APPROVED" => ReviewState::Approved,
                    "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
                    "COMMENTED" => ReviewState::Commented,
                    "DISMISSED" => ReviewState::Dismissed,
                    "PENDING" => ReviewState::Pending,
                    _ => return None,
                };
                Some(Review {
                    author: review
                        .author
                        .map_or_else(|| "ghost".to_owned(), |author| author.login),
                    state,
                })
            })
            .collect();
        PullRequestDetails {
            body: self.body,
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            commits: self.commits.total_count,
            mergeable: match self.mergeable.as_deref() {
                Some("MERGEABLE") => Mergeable::Yes,
                Some("CONFLICTING") => Mergeable::Conflicting,
                _ => Mergeable::Unknown,
            },
            checks,
            reviews,
            labels: self
                .labels
                .nodes
                .into_iter()
                .flatten()
                .map(|label| label.name)
                .collect(),
        }
    }
}

impl ContextNode {
    fn into_check(self) -> Check {
        if self.kind == "CheckRun" {
            let state = match (self.status.as_deref(), self.conclusion.as_deref()) {
                (Some("COMPLETED"), Some("SUCCESS")) => CheckState::Success,
                (
                    Some("COMPLETED"),
                    Some(
                        "FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
                        | "STARTUP_FAILURE",
                    ),
                ) => CheckState::Failure,
                (Some("COMPLETED"), _) => CheckState::Neutral,
                _ => CheckState::Pending,
            };
            Check {
                name: self.name.unwrap_or_default(),
                state,
                url: self.details_url,
            }
        } else {
            Check {
                name: self.context.unwrap_or_default(),
                state: self
                    .state
                    .as_deref()
                    .map_or(CheckState::Neutral, status_state),
                url: self.target_url,
            }
        }
    }
}

#[derive(Deserialize)]
struct RepositoryData {
    repository: Option<RepositoryNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryNode {
    default_branch_ref: Option<BranchRef>,
    parent: Option<ParentNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParentNode {
    name: String,
    owner: Login,
    default_branch_ref: Option<BranchRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_runs_and_statuses_have_states() {
        let run = |status: &str, conclusion: Option<&str>| {
            ContextNode {
                kind: "CheckRun".to_owned(),
                name: Some("build".to_owned()),
                conclusion: conclusion.map(str::to_owned),
                status: Some(status.to_owned()),
                details_url: None,
                context: None,
                state: None,
                target_url: None,
            }
            .into_check()
            .state
        };
        assert_eq!(run("COMPLETED", Some("SUCCESS")), CheckState::Success);
        assert_eq!(run("COMPLETED", Some("TIMED_OUT")), CheckState::Failure);
        assert_eq!(run("COMPLETED", Some("SKIPPED")), CheckState::Neutral);
        assert_eq!(run("IN_PROGRESS", None), CheckState::Pending);
        assert_eq!(status_state("EXPECTED"), CheckState::Pending);
        assert_eq!(status_state("ERROR"), CheckState::Failure);
    }
}
