//! A fake forge API on the loopback address, for tests: GitHub's or GitLab's.
//!
//! It answers the queries egit makes (a page of pull or merge requests, one's details, a
//! repository, who is signed in) and opens requests, from what the test gave it; it checks the
//! bearer token, can be rate limited, and records every request.

// A test fixture: it indexes and counts small known things, and never ships.
#![allow(
    clippy::missing_panics_doc,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::format_push_string
)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use parking_lot::Mutex;
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
    pub fn state(&self) -> parking_lot::MutexGuard<'_, FakeState> {
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
    })
}

type Answer = (u16, Vec<(&'static str, String)>, String);

/// The fake's answer to `request`.
fn answer(kind: ForgeKind, request: &Recorded, state: &mut FakeState) -> Answer {
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
    let path = variables["path"].as_str().unwrap_or_default().to_owned();
    let login = state.login.clone();
    // GitLab says a project it can't show by giving none.
    let Some(repository) = find(state, &path) else {
        return json!({ "data": { "project": null } });
    };
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
