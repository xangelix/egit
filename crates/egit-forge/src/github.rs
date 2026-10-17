//! GitHub's API: a repository's open pull requests (GraphQL, a page at a time), one pull
//! request's details, the repository's default branch and the one it was forked from, who is
//! signed in, and opening a pull request (REST).
//!
//! Every answer is checked: a rate limit used up (by its headers, or GraphQL's `RATE_LIMITED`)
//! says until when, a repository that isn't there (or isn't visible) says so, and anything else
//! the host refuses carries its own reason.

use std::time::{SystemTime, UNIX_EPOCH};

use egit_git::{CancelToken, ObjectId};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::account::Account;
use crate::error::ForgeError;
use crate::http::Answer;
use crate::repository::GitHubRepository;

/// Pull requests a page.
const PAGE: u32 = 100;

/// A pull request, as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// Its number in the repository.
    pub number: u32,
    /// Its title.
    pub title: String,
    /// Its author's login (`ghost` for a deleted account).
    pub author: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// The branch it brings in.
    pub head_branch: String,
    /// The repository that branch is in (`owner/name`), when known (a deleted fork isn't).
    pub head_repository: Option<String>,
    /// Whether that repository is another than the base's (a fork).
    pub cross_repository: bool,
    /// The branch it goes into.
    pub base_branch: String,
    /// Its head commit.
    pub head_commit: Option<ObjectId>,
    /// Where its reviews stand.
    pub review: ReviewDecision,
    /// Its head commit's checks, all together.
    pub checks: CheckState,
    /// When it last changed (seconds since the Unix epoch).
    pub updated: i64,
    /// Its web page.
    pub url: String,
}

/// Where a pull request's reviews stand, as the repository's rules see them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewDecision {
    /// No review is required, or none is known.
    #[default]
    None,
    /// A review is required and none approved yet.
    Required,
    /// Approved.
    Approved,
    /// A reviewer asked for changes.
    ChangesRequested,
}

/// A check's state (or a commit's, all its checks together).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckState {
    /// No checks.
    #[default]
    None,
    /// Skipped, neutral, or stale: neither passed nor failed.
    Neutral,
    /// Passed.
    Success,
    /// Queued or running.
    Pending,
    /// Failed (or errored, timed out, was cancelled, needs action).
    Failure,
}

/// A page of a repository's open pull requests.
#[derive(Clone, Debug, Default)]
pub struct PullRequestPage {
    /// The pull requests, the most recently updated first.
    pub pulls: Vec<PullRequest>,
    /// How many are open in all.
    pub total: u32,
    /// Where the next page starts, if there is one.
    pub next: Option<String>,
}

/// A pull request's details, read when it is looked at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequestDetails {
    /// Its description, as written (Markdown).
    pub body: String,
    /// Lines added and removed, and files changed.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// Files changed.
    pub changed_files: u32,
    /// Its commits.
    pub commits: u32,
    /// Whether it merges cleanly.
    pub mergeable: Mergeable,
    /// Its head commit's checks, one by one.
    pub checks: Vec<Check>,
    /// Each reviewer's latest review that approved or asked for changes.
    pub reviews: Vec<Review>,
    /// Its labels.
    pub labels: Vec<String>,
}

/// Whether a pull request merges cleanly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mergeable {
    /// Not known yet (GitHub works it out in the background).
    #[default]
    Unknown,
    /// It merges cleanly.
    Yes,
    /// It conflicts with its base.
    Conflicting,
}

/// One check of a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Its name.
    pub name: String,
    /// Its state.
    pub state: CheckState,
    /// Its page, if it has one.
    pub url: Option<String>,
}

/// A reviewer's latest opinion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// The reviewer's login.
    pub author: String,
    /// What they said.
    pub state: ReviewState,
}

/// What a review said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    /// Approved.
    Approved,
    /// Asked for changes.
    ChangesRequested,
    /// Commented.
    Commented,
    /// Was dismissed.
    Dismissed,
    /// Isn't submitted yet.
    Pending,
}

/// A repository's default branch, and the repository it was forked from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryInfo {
    /// Its default branch (none in an empty repository).
    pub default_branch: Option<String>,
    /// The repository it was forked from (`owner/name`) and that one's default branch.
    pub parent: Option<(String, Option<String>)>,
}

/// A pull request to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPullRequest {
    /// Its title.
    pub title: String,
    /// Its description.
    pub body: String,
    /// The branch it brings in.
    pub head_branch: String,
    /// The repository that branch is in, when it isn't the base repository (a fork).
    pub head_owner: Option<String>,
    /// The branch it goes into.
    pub base_branch: String,
    /// Whether it is a draft.
    pub draft: bool,
}

/// A pull request opened: its number and page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// Its number.
    pub number: u32,
    /// Its web page.
    pub url: String,
}

/// Reads `repository`'s open pull requests, the most recently updated first, a page at a time
/// (each handed to `found` as it lands), up to `limit` of them.
///
/// # Errors
///
/// Any of [`ForgeError`]'s, from the page that failed (the pages before it were handed over).
pub fn pull_requests(
    account: &Account,
    repository: &GitHubRepository,
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
        )?;
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
pub fn pull_request_details(
    account: &Account,
    repository: &GitHubRepository,
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
    )?;
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
pub fn repository_info(
    account: &Account,
    repository: &GitHubRepository,
    cancel: &CancelToken,
) -> Result<RepositoryInfo, ForgeError> {
    let data: RepositoryData = graphql(
        account,
        REPOSITORY,
        &json!({ "owner": repository.owner(), "name": repository.name() }),
        &repository.full_name(),
        cancel,
    )?;
    let found = data
        .repository
        .ok_or_else(|| not_found(account, repository.full_name()))?;
    Ok(RepositoryInfo {
        default_branch: found.default_branch_ref.map(|branch| branch.name),
        parent: found.parent.map(|parent| {
            (
                format!("{}/{}", parent.owner.login, parent.name),
                parent.default_branch_ref.map(|branch| branch.name),
            )
        }),
    })
}

/// Opens a pull request into `repository`.
///
/// # Errors
///
/// Any of [`ForgeError`]'s; [`ForgeError::Status`] with GitHub's reason when it refuses (one is
/// already open for the branch, nothing to merge, …).
pub fn create_pull_request(
    account: &Account,
    repository: &GitHubRepository,
    new: &NewPullRequest,
    cancel: &CancelToken,
) -> Result<Opened, ForgeError> {
    let head = new.head_owner.as_ref().map_or_else(
        || new.head_branch.clone(),
        |owner| format!("{owner}:{}", new.head_branch),
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
    let answer = account.post(&url, body.as_bytes(), cancel)?;
    let created: Created = read_answer(account, &answer, &repository.full_name())?;
    Ok(Opened {
        number: created.number,
        url: created.html_url,
    })
}

/// Who the credential is (and checks the host accepts it).
pub(crate) fn viewer(account: &Account, cancel: &CancelToken) -> Result<String, ForgeError> {
    let data: ViewerData = graphql(account, VIEWER, &json!({}), "the account", cancel)?;
    account.note_login(&data.viewer.login);
    Ok(data.viewer.login)
}

/// Runs a GraphQL query, reading its data or its errors.
fn graphql<T: DeserializeOwned>(
    account: &Account,
    query: &str,
    variables: &serde_json::Value,
    what: &str,
    cancel: &CancelToken,
) -> Result<T, ForgeError> {
    let body = json!({ "query": query, "variables": variables }).to_string();
    let url = account.host().graphql_url();
    let answer = account.post(&url, body.as_bytes(), cancel)?;
    let (remaining, reset) = (answer.remaining, answer.reset);
    let reply: GraphQlReply<T> = read_answer(account, &answer, what)?;
    if let Some(error) = reply.errors.first() {
        let host = account.host().name().to_owned();
        return Err(match error.kind.as_deref() {
            Some("RATE_LIMITED") => ForgeError::RateLimited {
                host,
                reset: reset.filter(|_| remaining.is_none_or(|left| left == 0)),
            },
            Some("NOT_FOUND") => ForgeError::NotFound {
                host,
                what: what.to_owned(),
            },
            Some("FORBIDDEN" | "INSUFFICIENT_SCOPES") => ForgeError::Forbidden {
                host,
                message: error.message.clone(),
            },
            _ => ForgeError::Status {
                host,
                status: 200,
                message: error.message.clone(),
            },
        });
    }
    reply.data.ok_or_else(|| ForgeError::Malformed {
        host: account.host().name().to_owned(),
        detail: "an answer without data".to_owned(),
    })
}

/// Reads a successful answer's JSON, or turns a refusal into its error.
fn read_answer<T: DeserializeOwned>(
    account: &Account,
    answer: &Answer,
    what: &str,
) -> Result<T, ForgeError> {
    let host = account.host().name().to_owned();
    match answer.status {
        200..=299 => serde_json::from_slice(&answer.body).map_err(|error| ForgeError::Malformed {
            host,
            detail: error.to_string(),
        }),
        403 | 429 if answer.remaining == Some(0) || answer.retry_after.is_some() => {
            let reset = answer.reset.or_else(|| {
                answer
                    .retry_after
                    .map(|after| now().saturating_add(after.max(0)))
            });
            Err(ForgeError::RateLimited { host, reset })
        }
        403 => Err(ForgeError::Forbidden {
            host,
            message: reason(&answer.body),
        }),
        404 => Err(ForgeError::NotFound {
            host,
            what: what.to_owned(),
        }),
        status => Err(ForgeError::Status {
            host,
            status,
            message: reason(&answer.body),
        }),
    }
}

/// A refusal's reason: GitHub's `message`, with a validation failure's own messages.
fn reason(body: &[u8]) -> String {
    let refusal: Refusal = serde_json::from_slice(body).unwrap_or_default();
    let details: Vec<String> = refusal
        .errors
        .into_iter()
        .filter_map(|error| error.message)
        .collect();
    if !details.is_empty() {
        return details.join(" ");
    }
    refusal.message.unwrap_or_else(|| {
        let text = String::from_utf8_lossy(body);
        text.chars().take(200).collect()
    })
}

fn not_found(account: &Account, what: String) -> ForgeError {
    ForgeError::NotFound {
        host: account.host().name().to_owned(),
        what,
    }
}

const fn cancelled() -> ForgeError {
    ForgeError::Cancelled
}

/// Seconds since the Unix epoch.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// An ISO 8601 time as GitHub writes it (`2024-05-01T10:20:30Z`), in seconds since the Unix
/// epoch.
pub(crate) fn parse_time(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    };
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if !separators
        .iter()
        .all(|&(at, byte)| text.as_bytes().get(at) == Some(&byte))
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm), in years starting in March.
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let month_index = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let of_year = 153_i64
        .checked_mul(month_index)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let of_cycle = of_era
        .checked_mul(365)?
        .checked_add(of_era.checked_div(4)?)?
        .checked_sub(of_era.checked_div(100)?)?
        .checked_add(of_year)?;
    let days = era
        .checked_mul(146_097)?
        .checked_add(of_cycle)?
        .checked_sub(719_468)?;
    days.checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)
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
struct GraphQlReply<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    message: String,
}

#[derive(Default, Deserialize)]
struct Refusal {
    message: Option<String>,
    #[serde(default)]
    errors: Vec<RefusalDetail>,
}

#[derive(Deserialize)]
struct RefusalDetail {
    message: Option<String>,
}

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
    fn github_times_are_read() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2024-02-29T12:34:56Z"), Some(1_709_210_096));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_time("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn a_refusal_s_own_reasons_are_given() {
        let body = br#"{"message":"Validation Failed","errors":[{"resource":"PullRequest",
            "code":"custom","message":"A pull request already exists for ada:topic."}]}"#;
        assert_eq!(reason(body), "A pull request already exists for ada:topic.");
        assert_eq!(reason(br#"{"message":"Not Found"}"#), "Not Found");
        assert_eq!(reason(b"<html>"), "<html>");
    }

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
