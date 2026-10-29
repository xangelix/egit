//! A fake forge API on the loopback address, for tests: GitHub's or GitLab's.
//!
//! It answers the queries egit makes (a page of pull or merge requests, one's details, a
//! repository, who is signed in; GitLab's pipelines and GitHub Actions' workflow runs, one
//! whole, a job and its log) and opens requests, retries, re-runs, and cancels, from what the
//! test gave it; it checks the bearer token (but for a log's file, which GitHub serves from a
//! signed address), can be rate limited, and records every request.

// A test fixture: it indexes and counts small known things, and never ships; its state is a
// set of switches.
#![allow(
    clippy::missing_panics_doc,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::format_push_string,
    clippy::struct_excessive_bools,
    clippy::integer_division
)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use egit_git::sync::Mutex;
use serde_json::{Value, json};

use crate::host::ForgeKind;

/// A pull (or merge) request the fake serves.
#[derive(Clone, Debug, Default)]
pub struct FakePull {
    /// Its number.
    pub number: u32,
    /// Its title.
    pub title: String,
    /// Its author's login.
    pub author: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// The branch it brings in.
    pub head_branch: String,
    /// The repository that branch is in (its path); `None`: the base's own.
    pub fork: Option<String>,
    /// The branch it goes into.
    pub base_branch: String,
    /// Its head commit (hexadecimal).
    pub head_commit: String,
    /// Its review: GitHub's decision (`APPROVED`, `REVIEW_REQUIRED`, …); on GitLab, `APPROVED`
    /// is approved by the fake's user, `REVIEW_REQUIRED` not yet approved.
    pub review: Option<String>,
    /// Its checks' state: GitHub's rollup (`SUCCESS`, `FAILURE`, …) or GitLab's pipeline
    /// status (`SUCCESS`, `FAILED`, `RUNNING`, …).
    pub checks: Option<String>,
    /// Its checks, as (name, status, conclusion): GitHub's check runs; on GitLab, jobs, whose
    /// status is the second.
    pub check_runs: Vec<(String, String, Option<String>)>,
    /// When it last changed (`2024-05-01T10:20:30Z`).
    pub updated: String,
    /// Its description.
    pub body: String,
}

/// A GitLab pipeline the fake serves, or a GitHub workflow's run.
#[derive(Clone, Debug, Default)]
pub struct FakePipeline {
    /// GitLab's number for it; a GitHub run's id.
    pub id: u64,
    /// Its number within its project; a GitHub workflow's run number.
    pub iid: u64,
    /// A GitHub run's workflow: its name ("CI"), id, and file (`.github/workflows/ci.yml`).
    pub workflow: (String, u64, String),
    /// A GitHub run's attempt (0 is 1).
    pub attempt: u32,
    /// Its commit (hexadecimal).
    pub sha: String,
    /// Its branch or tag.
    pub git_ref: String,
    /// Its status, as GitLab's enum names it (`SUCCESS`, `RUNNING`, …).
    pub status: String,
    /// Whether it passed with warnings.
    pub warnings: bool,
    /// When it was made (`2024-05-01T10:20:30Z`).
    pub created: String,
    /// Its stages in order: each one's name, status (`success`, …), and jobs.
    pub stages: Vec<(String, String, Vec<FakeJob>)>,
    /// A GitHub run's artifacts.
    pub artifacts: Vec<FakeArtifact>,
}

/// A GitHub run's artifact the fake serves.
#[derive(Clone, Debug, Default)]
pub struct FakeArtifact {
    /// Its number.
    pub id: u64,
    /// Its name.
    pub name: String,
    /// What its zip holds, as the fake sends it.
    pub contents: String,
    /// The SHA-256 digest listed for it (`sha256:<hex>`; empty: the contents').
    pub digest: String,
    /// Whether it has expired (its download answers 410).
    pub expired: bool,
}

/// A job of a fake pipeline.
#[derive(Clone, Debug, Default)]
pub struct FakeJob {
    /// Its number (0: one made up from its pipeline's and its place).
    pub id: u64,
    /// Its log, as a runner wrote it.
    pub log: String,
    /// Its name.
    pub name: String,
    /// Its status, as GitLab's enum names it.
    pub status: String,
    /// Whether it may fail.
    pub allow_failure: bool,
    /// Whether someone starts it by hand.
    pub manual: bool,
    /// How long it ran, in seconds.
    pub duration: Option<u64>,
    /// The jobs it waits for by name (`needs`), where it is scheduled by them.
    pub needs: Option<Vec<String>>,
    /// Its earlier runs (retried since), the newest first: each with a number of its own.
    pub earlier: Vec<Self>,
    /// A GitHub job's steps: each one's name and status (as GitLab's enum names it).
    pub steps: Vec<(String, String)>,
    /// A GitHub job's steps' own logs, by their places: each served as it is (a running step's
    /// too, as far as it was written), none where empty.
    pub step_logs: Vec<String>,
    /// A GitLab job's artifacts: each one's file type (`ARCHIVE`, `JUNIT`, …), name, and
    /// contents. An archive is sent through a redirect (as gitlab.com's CDN), a report as it is.
    pub artifacts: Vec<(String, String, String)>,
}

/// A status GitLab retries (`FAILED`, `CANCELED`), may retry (one that ended), cancels (under
/// way or waiting), or starts (`MANUAL`).
fn retried_by_pipeline(status: &str) -> bool {
    matches!(status, "FAILED" | "CANCELED")
}

fn ended(status: &str) -> bool {
    matches!(status, "FAILED" | "CANCELED" | "SUCCESS" | "SKIPPED")
}

fn under_way(status: &str) -> bool {
    matches!(status, "RUNNING" | "PENDING" | "CREATED" | "PREPARING")
}

/// A repository (or GitLab project) the fake serves.
#[derive(Clone, Debug, Default)]
pub struct FakeRepository {
    /// Its path: `owner/name`, or `group/…/project`.
    pub path: String,
    /// Its number (GitLab's project id).
    pub id: u64,
    /// Its default branch.
    pub default_branch: String,
    /// The repository it was forked from (its path), if any.
    pub parent: Option<String>,
    /// Its open requests, the most recently updated first.
    pub pulls: Vec<FakePull>,
    /// Its pipelines (GitLab's), the newest first.
    pub pipelines: Vec<FakePipeline>,
}

/// A request the fake received.
#[derive(Clone, Debug, Default)]
pub struct Recorded {
    /// `POST`, say.
    pub method: String,
    /// Its path.
    pub path: String,
    /// Its `Authorization` header.
    pub authorization: Option<String>,
    /// Its body, as JSON (null when it isn't).
    pub body: Value,
    /// Its `Range` header.
    pub range: Option<String>,
}

/// What the fake knows and has seen.
#[derive(Debug, Default)]
pub struct FakeState {
    /// The token it accepts.
    pub token: String,
    /// Whose it is.
    pub login: String,
    /// Its repositories.
    pub repositories: Vec<FakeRepository>,
    /// Whether every request is refused for the rate limit (resetting at 1,700,000,000).
    pub rate_limited: bool,
    /// The error GitLab gives for a project's pipelines, when it gives one (CI/CD turned off,
    /// say).
    pub pipelines_refused: Option<String>,
    /// Whether GitLab lists no pipelines (`pipelines: null`), as for a project whose CI/CD is
    /// turned off.
    pub pipelines_hidden: bool,
    /// The name GitLab's ssh is on, where not the fake's own (its projects' ssh addresses are
    /// then `ssh://git@<it>:2222/…`).
    pub ssh_host: Option<String>,
    /// Whether a job's log is sent from where a `Range` asks (else whole, as GitLab does).
    pub ranges: bool,
    /// Whether the account may only read pipelines (a Reporter's role): retrying or cancelling
    /// is refused.
    pub read_only: bool,
    /// The number the next run made gets.
    pub next_job: u64,
    /// Its own address (`127.0.0.1:<port>`): GitHub's logs' files are there.
    pub address: String,
    /// The requests it received.
    pub requests: Vec<Recorded>,
}

/// A fake forge API, serving until dropped.
pub struct FakeForge {
    kind: ForgeKind,
    address: SocketAddr,
    state: Arc<Mutex<FakeState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeForge {
    /// Starts a fake of `kind`'s API accepting `token` as `login`'s.
    ///
    /// # Errors
    ///
    /// When the loopback address can't be listened on.
    pub fn start(kind: ForgeKind, token: &str, login: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let state = Arc::new(Mutex::new(FakeState {
            token: token.to_owned(),
            login: login.to_owned(),
            address: address.to_string(),
            ..FakeState::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (state, stop) = (Arc::clone(&state), Arc::clone(&stop));
            std::thread::Builder::new()
                .name(format!("fake {}", kind.name()))
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if let Ok(stream) = stream {
                            serve(kind, stream, &state);
                        }
                    }
                })?
        };
        Ok(Self {
            kind,
            address,
            state,
            stop,
            thread: Some(thread),
        })
    }

    /// Which forge it fakes.
    #[must_use]
    pub const fn kind(&self) -> ForgeKind {
        self.kind
    }

    /// Its host, as a listed host's entry: `http://127.0.0.1:<port>`.
    #[must_use]
    pub fn host_entry(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Its `host:port`.
    #[must_use]
    pub fn address(&self) -> String {
        self.address.to_string()
    }

    /// What it knows and has seen, to change or check.
    pub fn state(&self) -> egit_git::sync::MutexGuard<'_, FakeState> {
        self.state.lock()
    }
}

impl Drop for FakeForge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wakes the accepting thread.
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reads one request, answers it, and closes the connection.
fn serve(kind: ForgeKind, stream: TcpStream, state: &Mutex<FakeState>) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let Some(request) = read_request(&mut reader) else {
        return;
    };
    let (status, headers, body) = answer(kind, &request, &mut state.lock());
    state.lock().requests.push(request);
    let mut writer = writer;
    let mut head = format!(
        "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = writer.write_all(head.as_bytes());
    let _ = writer.write_all(body.as_bytes());
    let _ = writer.flush();
    let _ = writer.shutdown(Shutdown::Both);
}

/// One HTTP/1.1 request: its line, headers, and body (by length, or chunked).
fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Recorded> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut length = 0_usize;
    let mut chunked = false;
    let mut authorization = None;
    let mut range = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().ok()?,
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "authorization" => authorization = Some(value.to_owned()),
            "range" => range = Some(value.to_owned()),
            _ => {}
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim(), 16).ok()?;
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).ok()?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else {
        body.resize(length, 0);
        reader.read_exact(&mut body).ok()?;
    }
    Some(Recorded {
        method,
        path,
        authorization,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
        range,
    })
}

type Answer = (u16, Vec<(&'static str, String)>, String);

/// The fake's answer to `request`.
fn answer(kind: ForgeKind, request: &Recorded, state: &mut FakeState) -> Answer {
    // A GitHub log's file (a job's, or a step's), at the address a redirect signed: no token.
    if let Some(place) = request.path.strip_prefix("/blob/job/") {
        return github_log_file(place, state);
    }
    if let Some(place) = request.path.strip_prefix("/blob/artifact/") {
        return artifact_file(place, state);
    }
    let bearer = request
        .authorization
        .as_deref()
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token);
    if bearer != Some(state.token.as_str()) {
        let refusal = match kind {
            ForgeKind::GitHub => json!({ "message": "Bad credentials" }),
            ForgeKind::GitLab => json!({ "message": "401 Unauthorized" }),
        };
        return (401, Vec::new(), refusal.to_string());
    }
    if state.rate_limited {
        return match kind {
            ForgeKind::GitHub => (
                403,
                vec![
                    ("x-ratelimit-remaining", "0".to_owned()),
                    ("x-ratelimit-reset", "1700000000".to_owned()),
                ],
                json!({ "message": "API rate limit exceeded" }).to_string(),
            ),
            ForgeKind::GitLab => (
                429,
                vec![
                    ("ratelimit-remaining", "0".to_owned()),
                    ("ratelimit-reset", "1700000000".to_owned()),
                    ("retry-after", "60".to_owned()),
                ],
                "Retry later".to_owned(),
            ),
        };
    }
    if request.method == "POST" && request.path == "/api/graphql" {
        let reply = match kind {
            ForgeKind::GitHub => github_graphql(request, state),
            ForgeKind::GitLab => gitlab_graphql(request, state),
        };
        return (200, Vec::new(), reply.to_string());
    }
    match kind {
        ForgeKind::GitHub => github_rest(request, state),
        ForgeKind::GitLab => gitlab_rest(request, state),
    }
}

fn find<'a>(state: &'a mut FakeState, path: &str) -> Option<&'a mut FakeRepository> {
    state
        .repositories
        .iter_mut()
        .find(|repository| repository.path.eq_ignore_ascii_case(path))
}

/// A page of `pulls` from `variables`' cursor: (start, end).
fn page(pulls: usize, variables: &Value) -> (usize, usize) {
    let first = usize::try_from(variables["first"].as_u64().unwrap_or(100)).unwrap_or(100);
    let start: usize = variables["after"]
        .as_str()
        .and_then(|cursor| cursor.parse().ok())
        .unwrap_or(0);
    let end = (start + first).min(pulls);
    (start.min(end), end)
}

/// A request opened in `repository` by `login`: its number.
fn open(repository: &mut FakeRepository, login: String, pull: FakePull) -> u32 {
    let number = repository
        .pulls
        .iter()
        .map(|pull| pull.number)
        .max()
        .unwrap_or(0)
        + 1;
    repository.pulls.insert(
        0,
        FakePull {
            number,
            author: login,
            ..pull
        },
    );
    number
}

fn github_graphql(request: &Recorded, state: &mut FakeState) -> Value {
    let query = request.body["query"].as_str().unwrap_or_default();
    let variables = &request.body["variables"];
    if query.starts_with("query Viewer") {
        return json!({ "data": { "viewer": { "login": state.login } } });
    }
    let path = format!(
        "{}/{}",
        variables["owner"].as_str().unwrap_or_default(),
        variables["name"].as_str().unwrap_or_default()
    );
    let login = state.login.clone();
    let Some(repository) = find(state, &path) else {
        return json!({
            "data": { "repository": null },
            "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a Repository." }],
        });
    };
    if query.starts_with("query PullRequests") {
        let (start, end) = page(repository.pulls.len(), variables);
        let nodes: Vec<Value> = repository.pulls[start..end]
            .iter()
            .map(|pull| github_node(repository, pull))
            .collect();
        return json!({ "data": { "repository": { "pullRequests": {
            "totalCount": repository.pulls.len(),
            "pageInfo": { "hasNextPage": end < repository.pulls.len(), "endCursor": end.to_string() },
            "nodes": nodes,
        } } } });
    }
    if query.starts_with("query PullRequest(") {
        let number = variables["number"].as_u64().unwrap_or(0);
        let Some(pull) = repository
            .pulls
            .iter()
            .find(|pull| u64::from(pull.number) == number)
        else {
            return json!({
                "data": { "repository": { "pullRequest": null } },
                "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a PullRequest." }],
            });
        };
        let contexts: Vec<Value> = pull
            .check_runs
            .iter()
            .map(|(name, status, conclusion)| {
                json!({ "__typename": "CheckRun", "name": name, "status": status,
                        "conclusion": conclusion, "detailsUrl": format!("https://ci.example/{name}") })
            })
            .collect();
        return json!({ "data": { "repository": { "pullRequest": {
            "body": pull.body, "additions": 10, "deletions": 2, "changedFiles": 3,
            "mergeable": "MERGEABLE",
            "commits": { "totalCount": 2, "nodes": [{ "commit": { "statusCheckRollup":
                { "contexts": { "nodes": contexts } } } }] },
            "latestOpinionatedReviews": { "nodes": [{ "author": { "login": login }, "state": "APPROVED" }] },
            "labels": { "nodes": [{ "name": "bug" }] },
        } } } });
    }
    if query.starts_with("query Repository") {
        let parent = repository.parent.as_ref().and_then(|parent| {
            let (owner, name) = parent.split_once('/')?;
            Some(json!({ "name": name, "owner": { "login": owner },
                         "defaultBranchRef": { "name": "main" } }))
        });
        return json!({ "data": { "repository": {
            "defaultBranchRef": { "name": repository.default_branch },
            "parent": parent,
        } } });
    }
    json!({ "errors": [{ "message": "unknown query" }] })
}

fn github_node(repository: &FakeRepository, pull: &FakePull) -> Value {
    let rollup = pull.checks.as_ref().map(|state| json!({ "state": state }));
    json!({
        "number": pull.number,
        "title": pull.title,
        "url": format!("https://github.example/{}/pull/{}", repository.path, pull.number),
        "isDraft": pull.draft,
        "updatedAt": if pull.updated.is_empty() { "2024-05-01T10:20:30Z" } else { &pull.updated },
        "isCrossRepository": pull.fork.is_some(),
        "author": { "login": pull.author },
        "baseRefName": pull.base_branch,
        "headRefName": pull.head_branch,
        "headRefOid": pull.head_commit,
        "headRepository": { "nameWithOwner": pull.fork.clone().unwrap_or_else(|| repository.path.clone()) },
        "reviewDecision": pull.review,
        "commits": { "nodes": [{ "commit": { "statusCheckRollup": rollup } }] },
    })
}

fn github_rest(request: &Recorded, state: &mut FakeState) -> Answer {
    let not_found = || {
        (
            404,
            Vec::new(),
            json!({ "message": "Not Found" }).to_string(),
        )
    };
    if let Some(answer) = github_actions(request, state) {
        return answer;
    }
    let opening = request
        .path
        .strip_prefix("/api/v3/repos/")
        .and_then(|rest| rest.strip_suffix("/pulls"));
    let (Some(path), "POST") = (opening, request.method.as_str()) else {
        return not_found();
    };
    let body = &request.body;
    let login = state.login.clone();
    let Some(repository) = find(state, path) else {
        return not_found();
    };
    let head = body["head"].as_str().unwrap_or_default();
    let (fork, branch) = head
        .split_once(':')
        .map_or((None, head), |(owner, branch)| (Some(owner), branch));
    let exists = repository.pulls.iter().any(|pull| {
        pull.head_branch == branch
            && pull.fork.as_deref().and_then(|fork| fork.split('/').next()) == fork
    });
    if exists {
        return (
            422,
            Vec::new(),
            json!({ "message": "Validation Failed", "errors": [{ "resource": "PullRequest",
                "code": "custom", "message": format!("A pull request already exists for {head}.") }] })
            .to_string(),
        );
    }
    let name = repository
        .path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let number = open(
        repository,
        login,
        FakePull {
            title: body["title"].as_str().unwrap_or_default().to_owned(),
            draft: body["draft"].as_bool().unwrap_or(false),
            head_branch: branch.to_owned(),
            fork: fork.map(|owner| format!("{owner}/{name}")),
            base_branch: body["base"].as_str().unwrap_or_default().to_owned(),
            body: body["body"].as_str().unwrap_or_default().to_owned(),
            ..FakePull::default()
        },
    );
    (
        201,
        Vec::new(),
        json!({ "number": number, "html_url": format!(
            "https://github.example/{}/pull/{number}", repository.path) })
        .to_string(),
    )
}

fn gitlab_graphql(request: &Recorded, state: &mut FakeState) -> Value {
    let query = request.body["query"].as_str().unwrap_or_default();
    let variables = &request.body["variables"];
    if query.starts_with("query CurrentUser") {
        return json!({ "data": { "currentUser": { "username": state.login } } });
    }
    if query.starts_with("query SshHost") {
        let address = |path: &str| {
            state.ssh_host.as_ref().map_or_else(
                || format!("git@127.0.0.1:{path}.git"),
                |ssh| format!("ssh://git@{ssh}:2222/{path}.git"),
            )
        };
        let nodes: Vec<Value> = state
            .repositories
            .first()
            .map(|repository| json!({ "sshUrlToRepo": address(&repository.path) }))
            .into_iter()
            .collect();
        return json!({ "data": { "mine": { "nodes": nodes }, "any": { "nodes": nodes } } });
    }
    let path = variables["path"].as_str().unwrap_or_default().to_owned();
    let login = state.login.clone();
    let can_update = !state.read_only;
    if query.starts_with("query Pipeline")
        && let Some(message) = &state.pipelines_refused
    {
        return json!({ "errors": [{ "message": message }], "data": { "project": null } });
    }
    if query.starts_with("query Pipelines(") && state.pipelines_hidden {
        return json!({ "data": { "project": { "pipelines": null } } });
    }
    // GitLab says a project it can't show by giving none.
    let Some(repository) = find(state, &path) else {
        return json!({ "data": { "project": null } });
    };
    if query.starts_with("query Pipeline") {
        return gitlab_pipelines(query, variables, repository, (&login, can_update));
    }
    if query.starts_with("query MergeRequests") {
        let (start, end) = page(repository.pulls.len(), variables);
        let nodes: Vec<Value> = repository.pulls[start..end]
            .iter()
            .map(|pull| gitlab_node(repository, pull, &login))
            .collect();
        return json!({ "data": { "project": { "mergeRequests": {
            "count": repository.pulls.len(),
            "pageInfo": { "hasNextPage": end < repository.pulls.len(), "endCursor": end.to_string() },
            "nodes": nodes,
        } } } });
    }
    if query.starts_with("query MergeRequest(") {
        let iid = variables["iid"].as_str().unwrap_or_default();
        let Some(pull) = repository
            .pulls
            .iter()
            .find(|pull| pull.number.to_string() == iid)
        else {
            return json!({ "data": { "project": { "mergeRequest": null } } });
        };
        let jobs: Vec<Value> = pull
            .check_runs
            .iter()
            .map(|(name, status, _)| {
                json!({ "name": name, "status": status, "allowFailure": false,
                        "webPath": format!("/{}/-/jobs/{name}", repository.path) })
            })
            .collect();
        let approved_by: Vec<Value> = if pull.review.as_deref() == Some("APPROVED") {
            vec![json!({ "username": login })]
        } else {
            Vec::new()
        };
        return json!({ "data": { "project": { "mergeRequest": {
            "description": pull.body, "commitCount": 2, "conflicts": false,
            "diffStatsSummary": { "additions": 10, "deletions": 2, "fileCount": 3 },
            "labels": { "nodes": [{ "title": "bug" }] },
            "approvedBy": { "nodes": approved_by },
            "headPipeline": { "jobs": { "nodes": jobs } },
        } } } });
    }
    json!({ "errors": [{ "message": "unknown query" }] })
}

/// GitLab's pipelines of `repository`: a page of them, or one whole.
fn gitlab_pipelines(
    query: &str,
    variables: &Value,
    repository: &FakeRepository,
    (login, can_update): (&str, bool),
) -> Value {
    if query.starts_with("query Pipelines") {
        let (start, end) = page(repository.pipelines.len(), variables);
        let nodes: Vec<Value> = repository.pipelines[start..end]
            .iter()
            .map(|pipeline| gitlab_listed(repository, pipeline, login))
            .collect();
        return json!({ "data": { "project": { "pipelines": {
            "pageInfo": { "hasNextPage": end < repository.pipelines.len(), "endCursor": end.to_string() },
            "nodes": nodes,
        } } } });
    }
    if query.starts_with("query Pipeline(") {
        let iid = variables["iid"].as_str().unwrap_or_default();
        let Some(pipeline) = repository
            .pipelines
            .iter()
            .find(|pipeline| pipeline.iid.to_string() == iid)
        else {
            return json!({ "data": { "project": { "pipeline": null } } });
        };
        let mut node = gitlab_pipeline(repository, pipeline);
        let stages: Vec<Value> = pipeline
            .stages
            .iter()
            .map(|(name, status, jobs)| {
                let jobs: Vec<Value> = jobs
                    .iter()
                    .map(|job| {
                        let id = job_id(pipeline, job);
                        let needs: Vec<Value> = job
                            .needs
                            .iter()
                            .flatten()
                            .map(|need| json!({ "name": need }))
                            .collect();
                        json!({ "id": format!("gid://gitlab/Ci::Build/{id}"), "name": job.name,
                                "status": job.status,
                                "allowFailure": job.allow_failure, "manualJob": job.manual,
                                "webPath": format!("/{}/-/jobs/{id}", repository.path),
                                "duration": job.duration, "startedAt": "2024-05-01T10:21:00Z",
                                "schedulingType": if job.needs.is_some() { "dag" } else { "stage" },
                                "needs": { "nodes": needs },
                                "retryable": ended(&job.status),
                                "cancelable": under_way(&job.status),
                                "playable": job.status == "MANUAL",
                                "userPermissions": { "updateBuild": can_update },
                                "artifacts": { "nodes": gitlab_artifact_nodes(job) },
                                "downstreamPipeline": null })
                    })
                    .collect();
                json!({ "name": name, "status": status, "jobs": { "nodes": jobs } })
            })
            .collect();
        let retried: Vec<Value> = pipeline
            .stages
            .iter()
            .flat_map(|(stage, _, jobs)| {
                jobs.iter().flat_map(move |job| {
                    job.earlier.iter().map(move |run| {
                        json!({ "id": format!("gid://gitlab/Ci::Build/{}", run.id),
                                "name": job.name, "status": run.status,
                                "duration": run.duration, "startedAt": "2024-05-01T10:15:00Z",
                                "webPath": format!("/{}/-/jobs/{}", repository.path, run.id),
                                "stage": { "name": stage } })
                    })
                })
            })
            .collect();
        let jobs = || pipeline.stages.iter().flat_map(|(_, _, jobs)| jobs);
        if let Some(fields) = node.as_object_mut() {
            fields.insert("startedAt".to_owned(), json!("2024-05-01T10:21:00Z"));
            fields.insert("finishedAt".to_owned(), Value::Null);
            fields.insert("duration".to_owned(), json!(95));
            fields.insert("source".to_owned(), json!("push"));
            fields.insert("user".to_owned(), json!({ "username": login }));
            fields.insert("stages".to_owned(), json!({ "nodes": stages }));
            fields.insert("retried".to_owned(), json!({ "nodes": retried }));
            fields.insert(
                "retryable".to_owned(),
                json!(jobs().any(|job| retried_by_pipeline(&job.status))),
            );
            fields.insert(
                "cancelable".to_owned(),
                json!(jobs().any(|job| under_way(&job.status))),
            );
            fields.insert(
                "userPermissions".to_owned(),
                json!({ "updatePipeline": can_update }),
            );
        }
        return json!({ "data": { "project": { "pipeline": node } } });
    }
    json!({ "errors": [{ "message": "unknown query" }] })
}

/// A pipeline's or a job's action (`<project>/pipelines/<id>/retry`, `…/jobs/<id>/cancel`, …):
/// done to the fake's pipelines as GitLab would (a job retried keeps its run as an earlier one,
/// and waits under a new number), or refused where the account may only read.
fn gitlab_ci_action(
    decoded: &dyn Fn(&str) -> String,
    rest: &str,
    state: &mut FakeState,
) -> Option<Answer> {
    let (rest, verb) = rest.rsplit_once('/')?;
    let (project, kind, id) = if let Some((project, id)) = rest.split_once("/pipelines/") {
        (project, "pipelines", id)
    } else {
        let (project, id) = rest.split_once("/jobs/")?;
        (project, "jobs", id)
    };
    let id: u64 = id.parse().ok()?;
    if state.read_only {
        return Some((
            403,
            Vec::new(),
            json!({ "message": "403 Forbidden" }).to_string(),
        ));
    }
    let project = decoded(project);
    if state.next_job == 0 {
        state.next_job = 990_000;
    }
    let mut next = state.next_job;
    let repository = find(state, &project)?;
    let mut answered = None;
    for pipeline in &mut repository.pipelines {
        // Each job's number fixed first: its place in the pipeline is about to change.
        let numbers: Vec<Vec<u64>> = pipeline
            .stages
            .iter()
            .map(|(_, _, jobs)| jobs.iter().map(|job| job_id(pipeline, job)).collect())
            .collect();
        let whole = kind == "pipelines" && pipeline.id == id;
        for ((_, _, jobs), numbers) in pipeline.stages.iter_mut().zip(&numbers) {
            for (job, number) in jobs.iter_mut().zip(numbers) {
                job.id = *number;
                let asked = kind == "jobs" && *number == id;
                let retry = verb == "retry"
                    && ((whole && retried_by_pipeline(&job.status))
                        || (asked && ended(&job.status)));
                if retry {
                    let mut earlier = job.clone();
                    earlier.earlier.clear();
                    job.earlier.insert(0, earlier);
                    next += 1;
                    job.id = next;
                    "PENDING".clone_into(&mut job.status);
                    job.log.clear();
                    job.duration = None;
                    answered = Some(next);
                } else if verb == "cancel" && (whole || asked) && under_way(&job.status) {
                    "CANCELED".clone_into(&mut job.status);
                    answered = Some(*number);
                } else if verb == "play" && asked && job.status == "MANUAL" {
                    "PENDING".clone_into(&mut job.status);
                    answered = Some(*number);
                } else if asked {
                    answered = answered.or(Some(*number));
                }
            }
        }
        if whole {
            let status = if verb == "retry" {
                "RUNNING"
            } else {
                "CANCELED"
            };
            status.clone_into(&mut pipeline.status);
            answered = Some(id);
        }
    }
    state.next_job = next;
    let id = answered?;
    Some((
        201,
        Vec::new(),
        json!({ "id": id, "status": "pending" }).to_string(),
    ))
}

/// A job's number: its own, or one made up from its pipeline's and its place in it.
fn job_id(pipeline: &FakePipeline, job: &FakeJob) -> u64 {
    if job.id != 0 {
        return job.id;
    }
    let place = pipeline
        .stages
        .iter()
        .flat_map(|(_, _, jobs)| jobs)
        .position(|other| std::ptr::eq(other, job))
        .unwrap_or(0);
    pipeline.id * 100 + u64::try_from(place).unwrap_or(0)
}

/// A job of `project` (`<id>`) as GitLab's REST API gives it, or its log (`<id>/trace`): whole,
/// or from where a `Range` asks when the fake answers ranges.
fn gitlab_job(project: &str, asked: &str, request: &Recorded, state: &FakeState) -> Option<Answer> {
    let (id, trace) = asked
        .strip_suffix("/trace")
        .map_or((asked, false), |id| (id, true));
    let id: u64 = id.parse().ok()?;
    let repository = state
        .repositories
        .iter()
        .find(|repository| repository.path.eq_ignore_ascii_case(project))?;
    // A job's latest run, or one of its earlier ones (each numbered).
    let (pipeline, stage_name, job) = repository.pipelines.iter().find_map(|pipeline| {
        pipeline.stages.iter().find_map(|(name, _, jobs)| {
            jobs.iter()
                .find(|job| job_id(pipeline, job) == id)
                .or_else(|| {
                    jobs.iter()
                        .flat_map(|job| &job.earlier)
                        .find(|earlier| earlier.id == id)
                })
                .map(|job| (pipeline, name, job))
        })
    })?;
    if !trace {
        let status = job.status.to_ascii_lowercase();
        let finished = !matches!(
            status.as_str(),
            "created" | "pending" | "running" | "manual" | "scheduled" | "preparing"
        );
        let job = json!({
            "id": id, "name": job.name, "stage": stage_name, "status": status,
            "started_at": "2024-05-01T10:21:00Z",
            "finished_at": finished.then_some("2024-05-01T10:22:35Z"),
            "duration": job.duration,
            "failure_reason": (status == "failed").then_some("script_failure"),
            "web_url": format!("http://127.0.0.1/{}/-/jobs/{id}", repository.path),
            "pipeline": { "id": pipeline.id },
        });
        return Some((200, Vec::new(), job.to_string()));
    }
    if job.log.is_empty() {
        return Some((
            404,
            Vec::new(),
            json!({ "message": "404 Not Found" }).to_string(),
        ));
    }
    let from = request
        .range
        .as_deref()
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|from| from.parse::<usize>().ok())
        .filter(|_| state.ranges);
    let log = job.log.as_bytes();
    Some(match from {
        Some(from) if from >= log.len() => (416, Vec::new(), String::new()),
        Some(from) => (
            206,
            vec![(
                "content-range",
                format!("bytes {from}-{}/{}", log.len() - 1, log.len()),
            )],
            String::from_utf8_lossy(&log[from..]).into_owned(),
        ),
        None => (200, Vec::new(), job.log.clone()),
    })
}

/// A pipeline as GitLab lists them: who made it run, how, and for how long, beside the rest.
fn gitlab_listed(repository: &FakeRepository, pipeline: &FakePipeline, login: &str) -> Value {
    let mut node = gitlab_pipeline(repository, pipeline);
    if let Some(fields) = node.as_object_mut() {
        let finished = !matches!(
            pipeline.status.as_str(),
            "RUNNING" | "PENDING" | "CREATED" | "MANUAL"
        );
        let (ended, took) = if finished {
            (json!("2024-05-01T10:22:35Z"), json!(95))
        } else {
            (Value::Null, Value::Null)
        };
        fields.insert("startedAt".to_owned(), json!("2024-05-01T10:21:00Z"));
        fields.insert("finishedAt".to_owned(), ended);
        fields.insert("duration".to_owned(), took);
        fields.insert("source".to_owned(), json!("push"));
        fields.insert("user".to_owned(), json!({ "username": login }));
    }
    node
}

fn gitlab_pipeline(repository: &FakeRepository, pipeline: &FakePipeline) -> Value {
    json!({
        "id": format!("gid://gitlab/Ci::Pipeline/{}", pipeline.id),
        "iid": pipeline.iid.to_string(),
        "sha": pipeline.sha,
        "ref": pipeline.git_ref,
        "status": pipeline.status,
        "warnings": pipeline.warnings,
        "createdAt": if pipeline.created.is_empty() { "2024-05-01T10:20:30Z" } else { &pipeline.created },
        "path": format!("/{}/-/pipelines/{}", repository.path, pipeline.id),
    })
}

fn gitlab_node(repository: &FakeRepository, pull: &FakePull, login: &str) -> Value {
    let (approved, approved_by) = match pull.review.as_deref() {
        Some("APPROVED") => (true, vec![json!({ "username": login })]),
        Some("REVIEW_REQUIRED") => (false, Vec::new()),
        _ => (true, Vec::new()),
    };
    json!({
        "iid": pull.number.to_string(),
        "title": pull.title,
        "draft": pull.draft,
        "webUrl": format!("https://gitlab.example/{}/-/merge_requests/{}", repository.path, pull.number),
        "updatedAt": if pull.updated.is_empty() { "2024-05-01T10:20:30Z" } else { &pull.updated },
        "sourceBranch": pull.head_branch,
        "targetBranch": pull.base_branch,
        "diffHeadSha": pull.head_commit,
        "approved": approved,
        "author": { "username": pull.author },
        "sourceProject": { "fullPath": pull.fork.clone().unwrap_or_else(|| repository.path.clone()) },
        "headPipeline": pull.checks.as_ref().map(|status| json!({ "status": status })),
        "approvedBy": { "nodes": approved_by },
    })
}

fn gitlab_rest(request: &Recorded, state: &mut FakeState) -> Answer {
    let not_found = || {
        (
            404,
            Vec::new(),
            json!({ "message": "404 Project Not Found" }).to_string(),
        )
    };
    let Some(rest) = request.path.strip_prefix("/api/v4/projects/") else {
        return not_found();
    };
    let decoded = |encoded: &str| encoded.replace("%2F", "/").replace("%2f", "/");
    if request.method == "POST"
        && let Some(project) = rest.strip_suffix("/pipeline")
    {
        let reference = request.body["ref"].as_str().unwrap_or_default();
        return start_pipeline(&decoded(project), reference, None, state)
            .map_or_else(not_found, |made| created_pipeline(&made));
    }
    if request.method == "POST"
        && let Some(answer) = gitlab_ci_action(&decoded, rest, state)
    {
        return answer;
    }
    if request.method == "GET"
        && let Some((project, job)) = rest.split_once("/jobs/")
    {
        if let Some(answer) = gitlab_artifact(&decoded(project), job, state) {
            return answer;
        }
        return gitlab_job(&decoded(project), job, request, state).unwrap_or_else(not_found);
    }
    if request.method == "GET" {
        return gitlab_project(&decoded(rest), state).unwrap_or_else(not_found);
    }
    let Some(source) = rest.strip_suffix("/merge_requests").map(decoded) else {
        return not_found();
    };
    let body = &request.body;
    let source_project = state
        .repositories
        .iter()
        .find(|repository| repository.path.eq_ignore_ascii_case(&source))
        .map(|repository| (repository.id, repository.path.clone()));
    let Some((source_id, source_path)) = source_project else {
        return not_found();
    };
    let target_id = body["target_project_id"].as_u64().unwrap_or(source_id);
    let login = state.login.clone();
    let Some(target) = state
        .repositories
        .iter_mut()
        .find(|repository| repository.id == target_id)
    else {
        return not_found();
    };
    let branch = body["source_branch"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let fork = (target.path != source_path).then_some(source_path);
    let exists = target
        .pulls
        .iter()
        .find(|pull| pull.head_branch == branch && pull.fork == fork)
        .map(|pull| pull.number);
    if let Some(number) = exists {
        return (
            409,
            Vec::new(),
            json!({ "message": [format!(
                "Another open merge request already exists for this source branch: !{number}")] })
            .to_string(),
        );
    }
    let title = body["title"].as_str().unwrap_or_default().to_owned();
    let draft = title.to_ascii_lowercase().starts_with("draft:");
    let number = open(
        target,
        login,
        FakePull {
            title,
            draft,
            head_branch: branch,
            fork,
            base_branch: body["target_branch"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            body: body["description"].as_str().unwrap_or_default().to_owned(),
            ..FakePull::default()
        },
    );
    (
        201,
        Vec::new(),
        json!({ "iid": number, "web_url": format!(
            "https://gitlab.example/{}/-/merge_requests/{number}", target.path) })
        .to_string(),
    )
}

/// A project as GitLab's REST API gives it: its id, default branch, and the one it was forked
/// from; `None` when there is none.
fn gitlab_project(path: &str, state: &mut FakeState) -> Option<Answer> {
    let parent = state
        .repositories
        .iter()
        .find(|repository| repository.path.eq_ignore_ascii_case(path))
        .and_then(|repository| repository.parent.clone());
    let parent = parent.map(|parent| {
        let found = state
            .repositories
            .iter()
            .find(|repository| repository.path.eq_ignore_ascii_case(&parent));
        json!({
            "id": found.map_or(0, |repository| repository.id),
            "path_with_namespace": parent,
            "default_branch": found.map(|repository| repository.default_branch.clone()),
        })
    });
    let repository = find(state, path)?;
    Some((
        200,
        Vec::new(),
        json!({ "id": repository.id, "path_with_namespace": repository.path,
                "default_branch": repository.default_branch, "forked_from_project": parent })
        .to_string(),
    ))
}

/// A fake status (GitLab's enum's word) as GitHub writes it: a status, and once completed a
/// conclusion.
fn github_status(status: &str) -> (&'static str, Option<&'static str>) {
    match status {
        "SUCCESS" => ("completed", Some("success")),
        "FAILED" => ("completed", Some("failure")),
        "CANCELED" => ("completed", Some("cancelled")),
        "SKIPPED" => ("completed", Some("skipped")),
        "RUNNING" => ("in_progress", None),
        "MANUAL" => ("waiting", None),
        _ => ("queued", None),
    }
}

/// A time `seconds` after the fake's jobs start (2024-05-01T10:21:00Z).
fn github_time(seconds: u64) -> String {
    let minutes = (21 + seconds.checked_div(60).unwrap_or(0)).min(59);
    format!("2024-05-01T10:{minutes:02}:{:02}Z", seconds % 60)
}

/// A minute and a half ago, as GitHub writes a time: when a job (or step) under way started.
fn github_lately() -> String {
    let now = crate::answer::now().saturating_sub(90);
    let (days, of_day) = (now.div_euclid(86_400), now.rem_euclid(86_400));
    // The civil date of a day since 1970 (Howard Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted - era * 146_097;
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3_600,
        of_day / 60 % 60,
        of_day % 60
    )
}

fn github_run(repository: &FakeRepository, run: &FakePipeline, state: &FakeState) -> Value {
    let (status, conclusion) = github_status(&run.status);
    let created = if run.created.is_empty() {
        "2024-05-01T10:20:30Z"
    } else {
        &run.created
    };
    json!({
        "id": run.id, "name": run.workflow.0, "workflow_id": run.workflow.1,
        "path": run.workflow.2, "head_branch": run.git_ref, "head_sha": run.sha,
        "run_number": run.iid, "run_attempt": run.attempt.max(1), "event": "push",
        "status": status, "conclusion": conclusion, "created_at": created,
        "updated_at": "2024-05-01T10:25:00Z", "run_started_at": created,
        "html_url": format!("http://{}/{}/actions/runs/{}", state.address, repository.path, run.id),
        "actor": { "login": state.login }, "triggering_actor": { "login": state.login },
    })
}

fn github_job(
    repository: &FakeRepository,
    run: &FakePipeline,
    (job, attempt): (&FakeJob, u32),
    state: &FakeState,
) -> Value {
    let id = job_id(run, job);
    let (status, conclusion) = github_status(&job.status);
    let steps: Vec<Value> = job
        .steps
        .iter()
        .enumerate()
        .map(|(at, (name, step))| {
            let (status, conclusion) = github_status(step);
            // Each step a second after the one before.
            let second = u64::try_from(at).unwrap_or(0);
            let started = if status == "in_progress" {
                github_lately()
            } else {
                github_time(second)
            };
            json!({ "name": name, "number": at + 1, "status": status,
                    "conclusion": conclusion, "started_at": started,
                    "completed_at": (status == "completed").then(|| github_time(second + 1)) })
        })
        .collect();
    let started = if status == "in_progress" {
        github_lately()
    } else {
        github_time(0)
    };
    json!({
        "id": id, "run_id": run.id, "run_attempt": attempt, "name": job.name,
        "status": status, "conclusion": conclusion, "started_at": started,
        "completed_at": (status == "completed").then(|| github_time(job.duration.unwrap_or(0))),
        "html_url": format!("http://{}/{}/actions/runs/{}/job/{id}", state.address, repository.path, run.id),
        "workflow_name": run.workflow.0, "steps": steps,
    })
}

/// A run's jobs as GitHub lists them: each one's latest (attempt by its earlier runs), and with
/// `all`, every earlier one too.
fn github_jobs(
    repository: &FakeRepository,
    run: &FakePipeline,
    all: bool,
    state: &FakeState,
) -> Vec<Value> {
    let mut jobs = Vec::new();
    for (_, _, stage) in &run.stages {
        for job in stage {
            let latest = u32::try_from(job.earlier.len()).unwrap_or(0) + 1;
            jobs.push(github_job(
                repository,
                run,
                (job, latest.max(run.attempt)),
                state,
            ));
            if all {
                for (back, earlier) in job.earlier.iter().enumerate() {
                    let attempt = latest - 1 - u32::try_from(back).unwrap_or(0);
                    jobs.push(github_job(
                        repository,
                        run,
                        (earlier, attempt.max(1)),
                        state,
                    ));
                }
            }
        }
    }
    jobs
}

/// The job (latest or earlier) numbered `id` among `runs`: its run, it, and its attempt.
fn github_find_job<'a>(
    runs: &'a [FakePipeline],
    id: &str,
) -> Option<(&'a FakePipeline, &'a FakeJob, u32)> {
    runs.iter().find_map(|run| {
        run.stages
            .iter()
            .flat_map(|(_, _, jobs)| jobs)
            .find_map(|job| {
                if job_id(run, job).to_string() == id {
                    return Some((run, job, u32::try_from(job.earlier.len()).unwrap_or(0) + 1));
                }
                job.earlier
                    .iter()
                    .find(|earlier| earlier.id.to_string() == id)
                    .map(|earlier| (run, earlier, 1))
            })
    })
}

/// A pipeline GitLab made, as its REST API answers.
fn created_pipeline(made: &FakePipeline) -> Answer {
    let made = json!({ "id": made.id, "iid": made.iid, "ref": made.git_ref, "status": "created" });
    (201, Vec::new(), made.to_string())
}

/// GitHub's workflows: listed (those its runs are of, each once), one's runs (on a branch), and
/// a run of one started (`workflow_dispatch`); none for another request.
fn github_workflows(
    request: &Recorded,
    (path, resource): (&str, &str),
    state: &mut FakeState,
) -> Option<Answer> {
    let parts: Vec<&str> = resource.split('/').collect();
    if request.method == "POST" {
        let ["actions", "workflows", workflow, "dispatches"] = parts.as_slice() else {
            return None;
        };
        let reference = request.body["ref"].as_str().unwrap_or_default();
        let read_only = state.read_only;
        let made = workflow
            .parse()
            .ok()
            .and_then(|workflow| start_pipeline(path, reference, Some(workflow), state));
        return Some(match made {
            Some(_) => (204, Vec::new(), String::new()),
            None if read_only => (
                403,
                Vec::new(),
                json!({ "message": "Must have admin rights to Repository." }).to_string(),
            ),
            None => (
                422,
                Vec::new(),
                json!({ "message": "Workflow does not have 'workflow_dispatch' trigger" })
                    .to_string(),
            ),
        });
    }
    let repository = state
        .repositories
        .iter()
        .find(|repository| repository.path.eq_ignore_ascii_case(path))?;
    let runs = &repository.pipelines;
    let answer = match parts.as_slice() {
        ["actions", "workflows"] => {
            let mut seen: Vec<&(String, u64, String)> = Vec::new();
            for run in runs {
                if !seen.iter().any(|known| known.1 == run.workflow.1) {
                    seen.push(&run.workflow);
                }
            }
            let listed: Vec<Value> = seen
                .iter()
                .map(|(name, id, file)| {
                    json!({ "id": id, "name": name, "path": file, "state": "active" })
                })
                .collect();
            json!({ "total_count": listed.len(), "workflows": listed })
        }
        ["actions", "workflows", workflow, "runs"] => {
            let branch = query(&request.path, "branch");
            let listed: Vec<Value> = runs
                .iter()
                .filter(|run| run.workflow.1.to_string() == *workflow)
                .filter(|run| branch.is_none_or(|branch| run.git_ref == branch))
                .map(|run| github_run(repository, run, state))
                .collect();
            json!({ "total_count": listed.len(), "workflow_runs": listed })
        }
        _ => return None,
    };
    Some((200, Vec::new(), answer.to_string()))
}

/// A new pipeline of the repository at `path` on `reference` (a GitHub run of `workflow`, the
/// one its newest run is of), waiting to start, listed first: GitLab's REST `POST …/pipeline`
/// and GitHub's dispatch (`read_only` refuses both, as `None`).
fn start_pipeline(
    path: &str,
    reference: &str,
    workflow: Option<u64>,
    state: &mut FakeState,
) -> Option<FakePipeline> {
    if state.read_only || reference.is_empty() {
        return None;
    }
    let repository = find(state, path)?;
    let runs = &repository.pipelines;
    let of = workflow.map_or_else(
        || ((String::new(), 0, String::new()), runs.first()),
        |workflow| {
            let newest = runs.iter().find(|run| run.workflow.1 == workflow);
            (newest.map_or_default(|run| run.workflow.clone()), newest)
        },
    );
    let ((name, id, file), newest) = of;
    if workflow.is_some() && newest.is_none() {
        return None;
    }
    let made = FakePipeline {
        id: runs.iter().map(|run| run.id).max().unwrap_or(9_000) + 1,
        iid: runs.iter().map(|run| run.iid).max().unwrap_or(0) + 1,
        workflow: (name, id, file),
        // The commit its ref's newest pipeline ran on, else the newest's (the fake knows no
        // branches).
        sha: runs
            .iter()
            .find(|run| run.git_ref == reference)
            .or_else(|| runs.first())
            .map_or_default(|run| run.sha.clone()),
        git_ref: reference.to_owned(),
        status: "CREATED".to_owned(),
        created: "2024-05-02T10:00:00Z".to_owned(),
        ..FakePipeline::default()
    };
    repository.pipelines.insert(0, made.clone());
    Some(made)
}

/// A query's parameter's value.
fn query<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    path.split_once('?')?
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

/// GitHub Actions' API: runs, a run, its jobs, a job, a job's log (a redirect to its file),
/// re-runs and cancelling; none for another request.
fn github_actions(request: &Recorded, state: &mut FakeState) -> Option<Answer> {
    let rest = request.path.strip_prefix("/api/v3/repos/")?;
    let (owner, rest) = rest.split_once('/')?;
    let (name, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let name = name.split('?').next().unwrap_or(name);
    let path = format!("{owner}/{name}");
    let resource = rest.split('?').next().unwrap_or(rest);
    let not_found = (
        404,
        Vec::new(),
        json!({ "message": "Not Found" }).to_string(),
    );
    let read_only = state.read_only;
    if let Some(answer) = github_workflows(request, (&path, resource), state) {
        return Some(answer);
    }
    if request.method == "POST" && resource.starts_with("actions/") {
        if read_only {
            return Some((
                403,
                Vec::new(),
                json!({ "message": "Must have admin rights to Repository." }).to_string(),
            ));
        }
        return Some(github_action(&path, resource, state).unwrap_or(not_found));
    }
    let snapshot = state
        .repositories
        .iter()
        .find(|repository| repository.path.eq_ignore_ascii_case(&path))?
        .clone();
    let runs = &snapshot.pipelines;
    let run_of = |id: &str| runs.iter().find(|run| run.id.to_string() == id);
    if let Some(answer) = github_artifacts(resource, runs, state) {
        return Some(answer);
    }
    let answer = match resource.split('/').collect::<Vec<_>>().as_slice() {
        [""] => json!({ "full_name": snapshot.path,
            "permissions": { "admin": false, "maintain": false, "push": !read_only, "pull": true } }),
        ["actions", "runs"] => {
            let per_page: usize = query(&request.path, "per_page")
                .and_then(|n| n.parse().ok())
                .unwrap_or(30);
            let page: usize = query(&request.path, "page")
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            let start = (page - 1) * per_page;
            let listed: Vec<Value> = runs
                .iter()
                .skip(start)
                .take(per_page)
                .map(|run| github_run(&snapshot, run, state))
                .collect();
            json!({ "total_count": runs.len(), "workflow_runs": listed })
        }
        ["actions", "runs", id] => match run_of(id) {
            Some(run) => github_run(&snapshot, run, state),
            None => return Some(not_found),
        },
        ["actions", "runs", id, "jobs"] => match run_of(id) {
            Some(run) => {
                let all = query(&request.path, "filter") == Some("all");
                let page: usize = query(&request.path, "page")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                let jobs = if page == 1 {
                    github_jobs(&snapshot, run, all, state)
                } else {
                    Vec::new()
                };
                json!({ "total_count": jobs.len(), "jobs": jobs })
            }
            None => return Some(not_found),
        },
        ["actions", "jobs", id, "steps", position, "logs"] => {
            return Some(github_step_log(runs, (id, position), state));
        }
        ["actions", "jobs", id] | ["actions", "jobs", id, "logs"] => {
            let found = github_find_job(runs, id);
            let Some((run, job, attempt)) = found else {
                return Some(not_found);
            };
            if resource.ends_with("/logs") {
                // GitHub's log is there once the job has ended.
                if !ended(&job.status) || job.log.is_empty() {
                    return Some(not_found);
                }
                let location = format!("http://{}/blob/job/{id}", state.address);
                return Some((302, vec![("location", location)], String::new()));
            }
            github_job(&snapshot, run, (job, attempt), state)
        }
        _ => return None,
    };
    Some((200, Vec::new(), answer.to_string()))
}

/// A run's artifacts listed, or an artifact's zip (a redirect to its file; 410 once expired);
/// none for another request.
fn github_artifacts(resource: &str, runs: &[FakePipeline], state: &FakeState) -> Option<Answer> {
    let not_found = (
        404,
        Vec::new(),
        json!({ "message": "Not Found" }).to_string(),
    );
    let answer = match resource.split('/').collect::<Vec<_>>().as_slice() {
        ["actions", "runs", id, "artifacts"] => {
            let Some(run) = runs.iter().find(|run| run.id.to_string() == *id) else {
                return Some(not_found);
            };
            let listed: Vec<Value> = run.artifacts.iter().map(github_artifact).collect();
            json!({ "total_count": listed.len(), "artifacts": listed })
        }
        ["actions", "artifacts", id, "zip"] => {
            let found = runs
                .iter()
                .flat_map(|run| &run.artifacts)
                .find(|artifact| artifact.id.to_string() == *id);
            return Some(match found {
                Some(artifact) if artifact.expired => (
                    410,
                    Vec::new(),
                    json!({ "message": "Artifact has expired" }).to_string(),
                ),
                Some(artifact) => (
                    302,
                    vec![(
                        "location",
                        format!("http://{}/blob/artifact/gh/{}", state.address, artifact.id),
                    )],
                    String::new(),
                ),
                None => not_found,
            });
        }
        _ => return None,
    };
    Some((200, Vec::new(), answer.to_string()))
}

/// A GitHub job's step's log (at `position` among its steps): a redirect to its file, once the
/// step has written one.
fn github_step_log(
    runs: &[FakePipeline],
    (id, position): (&str, &str),
    state: &FakeState,
) -> Answer {
    let written = github_find_job(runs, id).is_some_and(|(_, job, _)| {
        position
            .parse::<usize>()
            .ok()
            .and_then(|position| job.step_logs.get(position))
            .is_some_and(|log| !log.is_empty())
    });
    if !written {
        return (
            404,
            Vec::new(),
            json!({ "message": "Not Found" }).to_string(),
        );
    }
    let location = format!("http://{}/blob/job/{id}/step/{position}", state.address);
    (302, vec![("location", location)], String::new())
}

/// A GitHub log's file, whoever asks: a job's (`<id>`), or a step's (`<id>/step/<place>`).
fn github_log_file(place: &str, state: &FakeState) -> Answer {
    let (id, step) = place
        .split_once("/step/")
        .map_or((place, None), |(id, step)| (id, step.parse::<usize>().ok()));
    let log = state.repositories.iter().find_map(|repository| {
        github_find_job(&repository.pipelines, id).and_then(|(_, job, _)| {
            step.map_or_else(
                || Some(job.log.clone()),
                |step| job.step_logs.get(step).cloned(),
            )
        })
    });
    log.map_or_else(
        || {
            (
                404,
                Vec::new(),
                "The specified blob does not exist.".to_owned(),
            )
        },
        |log| (200, Vec::new(), log),
    )
}

/// A job run again in its place, as GitHub does: its run kept as an earlier one, the new one
/// numbered from `next`.
fn rerun(job: &mut FakeJob, number: u64, next: &mut u64) {
    job.id = number;
    let mut before = job.clone();
    before.earlier.clear();
    job.earlier.insert(0, before);
    *next += 1;
    job.id = *next;
    "PENDING".clone_into(&mut job.status);
    job.log.clear();
    job.step_logs.clear();
    job.duration = None;
    for (_, step) in &mut job.steps {
        "PENDING".clone_into(step);
    }
}

/// A run's re-run (of its failed jobs, or all), its cancelling, or a job's re-run.
fn github_action(path: &str, resource: &str, state: &mut FakeState) -> Option<Answer> {
    if state.next_job == 0 {
        state.next_job = 990_000;
    }
    let mut next = state.next_job;
    let repository = find(state, path)?;
    let parts: Vec<&str> = resource.split('/').collect();
    let answer = match parts.as_slice() {
        ["actions", "runs", id, verb] => {
            let run = repository
                .pipelines
                .iter_mut()
                .find(|run| run.id.to_string() == *id)?;
            let numbers: Vec<Vec<u64>> = run
                .stages
                .iter()
                .map(|(_, _, jobs)| jobs.iter().map(|job| job_id(run, job)).collect())
                .collect();
            match *verb {
                "cancel" => {
                    let mut any = false;
                    for (_, _, jobs) in &mut run.stages {
                        for job in jobs
                            .iter_mut()
                            .filter(|job| under_way(&job.status) || job.status == "MANUAL")
                        {
                            "CANCELED".clone_into(&mut job.status);
                            any = true;
                        }
                    }
                    if !any {
                        return Some((
                            409,
                            Vec::new(),
                            json!({ "message": "Cannot cancel a workflow run that is completed." })
                                .to_string(),
                        ));
                    }
                    "CANCELED".clone_into(&mut run.status);
                    (202, Vec::new(), String::new())
                }
                "rerun" | "rerun-failed-jobs" => {
                    let all = *verb == "rerun";
                    for ((_, _, jobs), numbers) in run.stages.iter_mut().zip(&numbers) {
                        for (job, number) in jobs.iter_mut().zip(numbers) {
                            if all || retried_by_pipeline(&job.status) {
                                rerun(job, *number, &mut next);
                            } else {
                                job.id = *number;
                            }
                        }
                    }
                    run.attempt = run.attempt.max(1) + 1;
                    "RUNNING".clone_into(&mut run.status);
                    (201, Vec::new(), String::new())
                }
                _ => return None,
            }
        }
        ["actions", "jobs", id, "rerun"] => {
            let mut done = false;
            for run in &mut repository.pipelines {
                let numbers: Vec<Vec<u64>> = run
                    .stages
                    .iter()
                    .map(|(_, _, jobs)| jobs.iter().map(|job| job_id(run, job)).collect())
                    .collect();
                for ((_, _, jobs), numbers) in run.stages.iter_mut().zip(&numbers) {
                    for (job, number) in jobs.iter_mut().zip(numbers) {
                        if number.to_string() == *id {
                            rerun(job, *number, &mut next);
                            done = true;
                        } else {
                            job.id = *number;
                        }
                    }
                }
                if done {
                    run.attempt = run.attempt.max(1) + 1;
                    "RUNNING".clone_into(&mut run.status);
                    break;
                }
            }
            if !done {
                return None;
            }
            (201, Vec::new(), String::new())
        }
        _ => return None,
    };
    state.next_job = next;
    Some(answer)
}

/// A GitLab job's artifacts as its GraphQL API lists them: its own, and its log's (a trace).
fn gitlab_artifact_nodes(job: &FakeJob) -> Vec<Value> {
    let mut nodes: Vec<Value> = job
        .artifacts
        .iter()
        .map(|(file_type, name, contents)| {
            json!({ "name": name, "fileType": file_type, "size": contents.len().to_string(),
                    "expireAt": null })
        })
        .collect();
    if !job.log.is_empty() {
        nodes.push(
            json!({ "name": "job.log", "fileType": "TRACE", "size": job.log.len().to_string(),
                           "expireAt": null }),
        );
    }
    nodes
}

/// A GitHub artifact as GitHub lists it, its digest the contents' unless given.
fn github_artifact(artifact: &FakeArtifact) -> Value {
    use sha2::{Digest as _, Sha256};
    let digest = if artifact.digest.is_empty() {
        let hash = Sha256::digest(artifact.contents.as_bytes());
        let hex = hash.iter().fold(String::new(), |mut hex, byte| {
            hex.push_str(&format!("{byte:02x}"));
            hex
        });
        format!("sha256:{hex}")
    } else {
        artifact.digest.clone()
    };
    json!({ "id": artifact.id, "name": artifact.name, "size_in_bytes": artifact.contents.len(),
            "expired": artifact.expired, "digest": digest,
            "archive_download_url": format!("/actions/artifacts/{}/zip", artifact.id) })
}

/// A GitLab job's artifact (`<project>/jobs/<id>/artifacts?file_type=<type>`): an archive a
/// redirect to its file, a report its file; none for another request.
fn gitlab_artifact(project: &str, job: &str, state: &FakeState) -> Option<Answer> {
    let (id, query) = job.split_once("/artifacts")?;
    let file_type = query
        .strip_prefix("?file_type=")
        .unwrap_or("archive")
        .to_ascii_uppercase();
    let repository = state
        .repositories
        .iter()
        .find(|repository| repository.path.eq_ignore_ascii_case(project))?;
    let contents = repository.pipelines.iter().find_map(|pipeline| {
        pipeline
            .stages
            .iter()
            .flat_map(|(_, _, jobs)| jobs)
            .find(|job| job_id(pipeline, job).to_string() == id)
            .and_then(|job| job.artifacts.iter().find(|(kind, _, _)| *kind == file_type))
            .map(|(_, _, contents)| contents.clone())
    });
    Some(match contents {
        None => (
            404,
            Vec::new(),
            json!({ "message": "404 Not Found" }).to_string(),
        ),
        Some(_) if file_type == "ARCHIVE" => (
            302,
            vec![(
                "location",
                format!("http://{}/blob/artifact/gl/{id}/{file_type}", state.address),
            )],
            String::new(),
        ),
        Some(contents) => (200, Vec::new(), contents),
    })
}

/// An artifact's file, at the address a redirect signed (`gl/<job>/<type>`, `gh/<id>`):
/// whoever asks.
fn artifact_file(place: &str, state: &FakeState) -> Answer {
    let pipelines = || {
        state
            .repositories
            .iter()
            .flat_map(|repository| &repository.pipelines)
    };
    let found = match place.split_once('/') {
        Some(("gl", rest)) => rest.split_once('/').and_then(|(id, file_type)| {
            pipelines().find_map(|pipeline| {
                pipeline
                    .stages
                    .iter()
                    .flat_map(|(_, _, jobs)| jobs)
                    .find(|job| job_id(pipeline, job).to_string() == id)
                    .and_then(|job| job.artifacts.iter().find(|(kind, _, _)| kind == file_type))
                    .map(|(_, _, contents)| contents.clone())
            })
        }),
        Some(("gh", id)) => pipelines()
            .flat_map(|pipeline| &pipeline.artifacts)
            .find(|artifact| artifact.id.to_string() == id)
            .map(|artifact| artifact.contents.clone()),
        _ => None,
    };
    found.map_or_else(
        || {
            (
                404,
                Vec::new(),
                "The specified blob does not exist.".to_owned(),
            )
        },
        |contents| (200, Vec::new(), contents),
    )
}
