//! egit in a browser, as the relay serves it: headless Chromium driven over its developer tools
//! protocol (`EGIT_TEST_BROWSER` names it, `EGIT_TEST_WEB` the web build `scripts/build-web.sh`
//! made; skipped without them). The page starts, opens the repository its address names
//! (`?open=`), titled by it, and shows the history's statistics from the palette (each git
//! through the relay); it is threaded (cross-origin isolated, its workers started), and nothing
//! in it or its workers panics or says an error. And served by a proxy under a path of its own
//! (as code-server's `/proxy/<port>`), opened without its last slash, it does the same. Its
//! screens are kept in `target/web-screens`.

#![cfg(unix)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use egit_git::testing::{Sandbox, TestRelay};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

/// How long the page may take to start and open a repository: its WebAssembly is compiled
/// first, and a software renderer draws it.
const STARTING: Duration = Duration::from_secs(90);

/// The browser and the web build, when both are named.
fn wanted() -> Option<(PathBuf, PathBuf)> {
    let browser = std::env::var_os("EGIT_TEST_BROWSER").filter(|value| !value.is_empty())?;
    let web = std::env::var_os("EGIT_TEST_WEB").filter(|value| !value.is_empty())?;
    Some((browser.into(), web.into()))
}

/// Chromium, headless, its developer tools on a port of its own; stopped when dropped.
struct Browser {
    child: Child,
    /// Its developer tools' WebSocket.
    devtools: String,
    _profile: tempfile::TempDir,
}

impl Browser {
    fn start(program: &Path) -> Self {
        let profile = tempfile::tempdir().unwrap();
        let mut child = Command::new(program)
            .args([
                "--headless=new",
                // A page of the loopback address's own: no sandbox needed (and CI's has none).
                "--no-sandbox",
                "--no-first-run",
                "--no-default-browser-check",
                "--use-angle=swiftshader",
                "--enable-unsafe-swiftshader",
                // WebGL2, whose canvas a screenshot reads (WebGPU's shows blank, headless).
                "--disable-features=WebGPU",
                "--window-size=1400,900",
                "--remote-debugging-port=0",
            ])
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("about:blank")
            .env("TZ", "UTC")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let devtools = lines
            .by_ref()
            .map_while(Result::ok)
            .find_map(|line| {
                line.strip_prefix("DevTools listening on ")
                    .map(str::to_owned)
            })
            .expect("the browser said nowhere for its DevTools");
        // The rest of what it says, read so it never waits on a full pipe.
        std::thread::spawn(move || lines.for_each(drop));
        Self {
            child,
            devtools,
            _profile: profile,
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A page of the browser's, driven through its developer tools: what it says noted as it comes.
struct Page {
    socket: WebSocketStream<TcpStream>,
    session: String,
    next: u64,
    /// What went wrong in it or its workers: console errors, exceptions, a panic's words.
    wrong: Vec<String>,
    /// Its workers' sessions, attached to as they start.
    workers: Vec<String>,
    /// Those whose console isn't followed yet.
    unfollowed: Vec<String>,
}

impl Page {
    /// The browser's first page, attached to.
    async fn of(browser: &Browser) -> Self {
        let request = browser.devtools.as_str().into_client_request().unwrap();
        let host = request.uri().authority().unwrap().as_str().to_owned();
        let stream = TcpStream::connect(host).await.unwrap();
        let (socket, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .unwrap();
        let mut page = Self {
            socket,
            session: String::new(),
            next: 0,
            wrong: Vec::new(),
            workers: Vec::new(),
            unfollowed: Vec::new(),
        };
        let targets = page.call("Target.getTargets", json!({})).await;
        let target = targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["type"] == "page")
            .unwrap()["targetId"]
            .clone();
        let attached = page
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .await;
        page.session = attached["sessionId"].as_str().unwrap().to_owned();
        for domain in ["Runtime.enable", "Page.enable", "Log.enable"] {
            page.call(domain, json!({})).await;
        }
        // Its workers attached to as they start, so what they say is heard too.
        page.call(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        )
        .await;
        page
    }

    /// Follows the console of the workers attached to since (their answers aren't waited for).
    async fn follow_workers(&mut self) {
        for session in std::mem::take(&mut self.unfollowed) {
            self.next += 1;
            let message =
                json!({ "id": self.next, "method": "Runtime.enable", "sessionId": session });
            self.socket
                .send(Message::Text(message.to_string().into()))
                .await
                .unwrap();
        }
    }

    /// Calls `method`: its result.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        match self.answer(method, params).await {
            Ok(result) => result,
            Err(said) => panic!("{method}: {said}"),
        }
    }

    /// Calls `method`: its result, or what the browser said went wrong.
    async fn answer(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if !self.session.is_empty() {
            message["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .unwrap();
        loop {
            let Some(Ok(Message::Text(text))) = self.socket.next().await else {
                panic!("the browser went away");
            };
            let said: Value = serde_json::from_str(&text).unwrap();
            if said["id"] == id && said.get("sessionId") == message.get("sessionId") {
                if let Some(error) = said.get("error") {
                    return Err(error.clone());
                }
                return Ok(said["result"].clone());
            }
            self.note(&said);
            self.follow_workers().await;
        }
    }

    /// Notes what an event says went wrong.
    fn note(&mut self, event: &Value) {
        let params = &event["params"];
        match event["method"].as_str() {
            Some("Runtime.consoleAPICalled") if params["type"] == "error" => {
                let words = params["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|arg| {
                        arg["value"]
                            .as_str()
                            .or_else(|| arg["description"].as_str())
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.wrong.push(words);
            }
            Some("Runtime.exceptionThrown") => self.wrong.push(params.to_string()),
            Some("Target.attachedToTarget") if params["targetInfo"]["type"] == "worker" => {
                let session = params["sessionId"].as_str().unwrap_or_default().to_owned();
                self.workers.push(session.clone());
                self.unfollowed.push(session);
            }
            Some("Log.entryAdded") if params["entry"]["level"] == "error" => self.wrong.push(
                params["entry"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ),
            _ => {}
        }
    }

    /// Lets the page run for `time`, noting what it says.
    async fn idle(&mut self, time: Duration) {
        let until = Instant::now() + time;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.socket.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    let said: Value = serde_json::from_str(&text).unwrap();
                    self.note(&said);
                    self.follow_workers().await;
                }
                Ok(Some(Ok(_))) => {}
                _ => return,
            }
        }
    }

    /// Waits up to `limit` for the page's title to be `wanted`.
    async fn titled(&mut self, wanted: &str, limit: Duration) -> bool {
        let since = Instant::now();
        while since.elapsed() < limit {
            if self.evaluate("document.title").await == wanted {
                return true;
            }
            self.idle(Duration::from_millis(250)).await;
        }
        false
    }

    /// What `expression` comes to in the page; nothing while it was navigating elsewhere (the
    /// page adding its address's last slash, say).
    async fn evaluate(&mut self, expression: &str) -> Value {
        self.answer(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .await
        .map_or(Value::Null, |said| said["result"]["value"].clone())
    }

    /// Opens `address` (a sandbox's repository named `repo` in it to open): started, threaded,
    /// the repository opened through the relay (the page titled by it), and the address bar
    /// showing `shown`. Its screen kept as `screen`.
    async fn opens(&mut self, address: &str, shown: &str, screen: &str) {
        self.call("Page.navigate", json!({ "url": address })).await;
        // Started, the repository opened through the relay: the page says which is in front.
        let opened = self.titled("repo — egit", STARTING).await;
        self.idle(Duration::from_secs(3)).await;
        self.keep(screen).await;
        assert!(opened, "never opened the repository: {:?}", self.wrong);
        // Out of the address bar: the token, and what was asked to be opened.
        assert_eq!(self.evaluate("location.href").await, shown);
        // Threaded: the relay's headers isolate it (memory shared with its workers), and its
        // workers started, each running egit's WebAssembly.
        assert_eq!(
            self.evaluate("self.crossOriginIsolated").await,
            true,
            "not cross-origin isolated"
        );
        assert!(
            self.workers.len() >= 2,
            "{} workers started",
            self.workers.len()
        );
    }

    /// Clicks at `x`, `y`.
    async fn click(&mut self, x: u32, y: u32) {
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
            )
            .await;
        }
    }

    /// Presses `key` (its `code` and Windows' number for it), with `modifiers` (2: Ctrl).
    async fn press(&mut self, (key, code, number): (&str, &str, u32), modifiers: u32) {
        for kind in ["rawKeyDown", "keyUp"] {
            self.call(
                "Input.dispatchKeyEvent",
                json!({
                    "type": kind,
                    "key": key,
                    "code": code,
                    "windowsVirtualKeyCode": number,
                    "modifiers": modifiers,
                }),
            )
            .await;
        }
    }

    /// Types `text` where the keys go.
    async fn type_text(&mut self, text: &str) {
        self.call("Input.insertText", json!({ "text": text })).await;
    }

    /// Keeps the page's screen as `target/web-screens/<name>.png`.
    async fn keep(&mut self, name: &str) {
        let shot = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await;
        let png = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap())
            .unwrap();
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/web-screens");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{name}.png")), png).unwrap();
    }
}

/// `path` as an address's query takes it.
fn encoded(path: &Path) -> String {
    path.to_str()
        .unwrap()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"/._-~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// A sandbox's repository of three commits, named `repo`.
fn repository(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    for (n, file) in ["one", "two", "three"].iter().enumerate() {
        sandbox
            .commit_file(&repo, file, &format!("{n}\n"), &format!("Add {file}"))
            .unwrap();
    }
    repo
}

/// The relay serving `web`, the build.
fn relay_of(web: &Path) -> TestRelay {
    TestRelay::start_with(
        Path::new(env!("CARGO_BIN_EXE_egit-relay")),
        &["--web", web.to_str().unwrap()],
    )
    .unwrap()
}

/// A proxy serving `relay` under `prefix`, as code-server's `/proxy/<port>` does: each request's
/// path has the prefix taken off (`/` where nothing is left), everything else passes through
/// both ways, a WebSocket's frames too. Where it listens.
async fn proxy_under(prefix: &'static str, relay: String) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut browser, _)) = listener.accept().await {
            let relay = relay.clone();
            tokio::spawn(async move {
                // The request line, a byte at a time: what follows passes through as it is.
                let mut line = Vec::new();
                while !line.ends_with(b"\r\n") {
                    let Ok(byte) = browser.read_u8().await else {
                        return;
                    };
                    line.push(byte);
                }
                let line = String::from_utf8_lossy(&line).into_owned();
                let mut parts = line.splitn(3, ' ');
                let (method, path, version) = (
                    parts.next().unwrap_or_default(),
                    parts.next().unwrap_or_default(),
                    parts.next().unwrap_or_default(),
                );
                let Some(rest) = path.strip_prefix(prefix) else {
                    let _ = browser
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                };
                let rest = if rest.starts_with('/') {
                    rest.to_owned()
                } else {
                    format!("/{rest}")
                };
                let Ok(mut upstream) = TcpStream::connect(&relay).await else {
                    return;
                };
                if upstream
                    .write_all(format!("{method} {rest} {version}").as_bytes())
                    .await
                    .is_ok()
                {
                    let _ = tokio::io::copy_bidirectional(&mut browser, &mut upstream).await;
                }
            });
        }
    });
    address
}

/// The browser's runtime: one thread, the tests' proxies on it too.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn egit_starts_in_a_browser_and_opens_a_repository_through_its_relay() {
    let Some((program, web)) = wanted() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let repo = repository(&sandbox);
    let relay = relay_of(&web);
    let browser = Browser::start(&program);
    runtime().block_on(async {
        let mut page = Page::of(&browser).await;
        let address = format!(
            "http://{}/?open={}#{}",
            relay.address,
            encoded(&repo),
            relay.token
        );
        page.opens(
            &address,
            &format!("http://{}/", relay.address),
            "browser-opened",
        )
        .await;

        // The palette, from the keys (taken from the browser), runs the Stats page.
        page.click(700, 600).await;
        page.press(("k", "KeyK", 75), 2).await;
        page.idle(Duration::from_millis(500)).await;
        page.type_text("statistics").await;
        page.idle(Duration::from_millis(500)).await;
        page.press(("Enter", "Enter", 13), 0).await;
        page.idle(Duration::from_secs(5)).await;
        page.keep("browser-stats").await;
        assert!(
            page.wrong.is_empty(),
            "the page went wrong: {:?}",
            page.wrong
        );
    });
}

#[test]
fn egit_opens_under_a_proxy_s_path_given_without_its_last_slash() {
    let Some((program, web)) = wanted() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let repo = repository(&sandbox);
    let relay = relay_of(&web);
    let browser = Browser::start(&program);
    runtime().block_on(async {
        let proxy = proxy_under("/proxy/7171", relay.address.clone()).await;
        let mut page = Page::of(&browser).await;
        // As code-server's address is often given: its last slash left off. The page adds it,
        // then finds its files, its WebSocket, and its repository below that path.
        let address = format!(
            "http://{proxy}/proxy/7171?open={}#{}",
            encoded(&repo),
            relay.token
        );
        page.opens(
            &address,
            &format!("http://{proxy}/proxy/7171/"),
            "browser-proxied",
        )
        .await;
        assert!(
            page.wrong.is_empty(),
            "the page went wrong: {:?}",
            page.wrong
        );
    });
}
