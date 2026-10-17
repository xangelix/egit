//! A fake GitHub API on the loopback address, for tests.
//!
//! It answers the GraphQL queries egit makes (a page of pull requests, one's details, a
//! repository, the viewer) and opens pull requests (REST) from what the test gave it, checks
//! the bearer token, can be rate limited, and records every request.

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

/// A pull request the fake serves.
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
    /// The repository that branch is in (`owner/name`); `None`: the base's own.
    pub fork: Option<String>,
    /// The branch it goes into.
    pub base_branch: String,
    /// Its head commit (hexadecimal).
    pub head_commit: String,
    /// GitHub's review decision (`APPROVED`, …).
    pub review: Option<String>,
    /// Its checks' rollup state (`SUCCESS`, …).
    pub checks: Option<String>,
    /// Its checks, as (name, status, conclusion).
    pub check_runs: Vec<(String, String, Option<String>)>,
    /// When it last changed (`2024-05-01T10:20:30Z`).
    pub updated: String,
    /// Its description.
    pub body: String,
}

/// A repository the fake serves.
#[derive(Clone, Debug, Default)]
pub struct FakeRepository {
    /// Its owner.
    pub owner: String,
    /// Its name.
    pub name: String,
    /// Its default branch.
    pub default_branch: String,
    /// The repository it was forked from (`owner/name`), if any.
    pub parent: Option<String>,
    /// Its open pull requests, the most recently updated first.
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

/// A fake GitHub API, serving until dropped.
pub struct FakeGitHub {
    address: SocketAddr,
    state: Arc<Mutex<FakeState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeGitHub {
    /// Starts one accepting `token` as `login`'s.
    ///
    /// # Errors
    ///
    /// When the loopback address can't be listened on.
    pub fn start(token: &str, login: &str) -> std::io::Result<Self> {
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
                .name("fake GitHub".to_owned())
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if let Ok(stream) = stream {
                            serve(stream, &state);
                        }
                    }
                })?
        };
        Ok(Self {
            address,
            state,
            stop,
            thread: Some(thread),
        })
    }

    /// Its host, as an Enterprise host's entry: `http://127.0.0.1:<port>`.
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

impl Drop for FakeGitHub {
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
fn serve(stream: TcpStream, state: &Mutex<FakeState>) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let Some(request) = read_request(&mut reader) else {
        return;
    };
    let (status, headers, body) = answer(&request, &mut state.lock());
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
fn answer(request: &Recorded, state: &mut FakeState) -> Answer {
    if request.authorization.as_deref() != Some(&format!("bearer {}", state.token)) {
        return (
            401,
            Vec::new(),
            json!({ "message": "Bad credentials" }).to_string(),
        );
    }
    if state.rate_limited {
        return (
            403,
            vec![
                ("x-ratelimit-remaining", "0".to_owned()),
                ("x-ratelimit-reset", "1700000000".to_owned()),
            ],
            json!({ "message": "API rate limit exceeded" }).to_string(),
        );
    }
    if request.method == "POST" && request.path == "/api/graphql" {
        return (200, Vec::new(), graphql(request, state).to_string());
    }
    let created = request
        .path
        .strip_prefix("/api/v3/repos/")
        .and_then(|rest| rest.strip_suffix("/pulls"))
        .and_then(|rest| rest.split_once('/'));
    if let (Some((owner, name)), "POST") = (created, request.method.as_str()) {
        return create(owner, name, &request.body, state);
    }
    (
        404,
        Vec::new(),
        json!({ "message": "Not Found" }).to_string(),
    )
}

fn find<'a>(state: &'a mut FakeState, owner: &str, name: &str) -> Option<&'a mut FakeRepository> {
    state.repositories.iter_mut().find(|repository| {
        repository.owner.eq_ignore_ascii_case(owner) && repository.name.eq_ignore_ascii_case(name)
    })
}

fn not_found() -> Value {
    json!({
        "data": { "repository": null },
        "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a Repository." }],
    })
}

fn graphql(request: &Recorded, state: &mut FakeState) -> Value {
    let query = request.body["query"].as_str().unwrap_or_default();
    let variables = &request.body["variables"];
    let owner = variables["owner"].as_str().unwrap_or_default();
    let name = variables["name"].as_str().unwrap_or_default();
    if query.starts_with("query Viewer") {
        return json!({ "data": { "viewer": { "login": state.login } } });
    }
    let login = state.login.clone();
    let Some(repository) = find(state, owner, name) else {
        return not_found();
    };
    if query.starts_with("query PullRequests") {
        let first = usize::try_from(variables["first"].as_u64().unwrap_or(100)).unwrap_or(100);
        let start: usize = variables["after"]
            .as_str()
            .and_then(|cursor| cursor.parse().ok())
            .unwrap_or(0);
        let end = (start + first).min(repository.pulls.len());
        let nodes: Vec<Value> = repository.pulls[start.min(end)..end]
            .iter()
            .map(|pull| pull_node(repository, pull))
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

fn pull_node(repository: &FakeRepository, pull: &FakePull) -> Value {
    let rollup = pull.checks.as_ref().map(|state| json!({ "state": state }));
    json!({
        "number": pull.number,
        "title": pull.title,
        "url": format!("https://github.example/{}/{}/pull/{}", repository.owner, repository.name, pull.number),
        "isDraft": pull.draft,
        "updatedAt": if pull.updated.is_empty() { "2024-05-01T10:20:30Z" } else { &pull.updated },
        "isCrossRepository": pull.fork.is_some(),
        "author": { "login": pull.author },
        "baseRefName": pull.base_branch,
        "headRefName": pull.head_branch,
        "headRefOid": pull.head_commit,
        "headRepository": { "nameWithOwner": pull.fork.clone().unwrap_or_else(|| format!("{}/{}", repository.owner, repository.name)) },
        "reviewDecision": pull.review,
        "commits": { "nodes": [{ "commit": { "statusCheckRollup": rollup } }] },
    })
}

fn create(owner: &str, name: &str, body: &Value, state: &mut FakeState) -> Answer {
    let login = state.login.clone();
    let Some(repository) = find(state, owner, name) else {
        return (
            404,
            Vec::new(),
            json!({ "message": "Not Found" }).to_string(),
        );
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
            title: body["title"].as_str().unwrap_or_default().to_owned(),
            author: login,
            draft: body["draft"].as_bool().unwrap_or(false),
            head_branch: branch.to_owned(),
            fork: fork.map(|owner| format!("{owner}/{}", repository.name)),
            base_branch: body["base"].as_str().unwrap_or_default().to_owned(),
            body: body["body"].as_str().unwrap_or_default().to_owned(),
            ..FakePull::default()
        },
    );
    (
        201,
        Vec::new(),
        json!({ "number": number, "html_url": format!(
            "https://github.example/{}/{}/pull/{number}", repository.owner, repository.name) })
        .to_string(),
    )
}
