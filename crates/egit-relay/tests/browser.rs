//! egit in a browser, as the relay serves it: headless Chromium driven over its developer tools
//! protocol (`EGIT_TEST_BROWSER` names it, `EGIT_TEST_WEB` the web build `scripts/build-web.sh`
//! made; skipped without them). The page starts, opens the repository its address names
//! (`?open=`), titled by it, and shows the history's statistics from the palette (each git
//! through the relay); nothing in it panics or says an error. Its screens are kept in
//! `target/web-screens`.

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
use tokio::net::TcpStream;
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
    /// What went wrong in it: console errors, exceptions, a panic's words.
    wrong: Vec<String>,
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
        page
    }

    /// Calls `method`: its result.
    async fn call(&mut self, method: &str, params: Value) -> Value {
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
            if said["id"] == id {
                assert!(said.get("error").is_none(), "{method}: {said}");
                return said["result"].clone();
            }
            self.note(&said);
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
            let title = self
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "document.title", "returnByValue": true }),
                )
                .await;
            if title["result"]["value"] == wanted {
                return true;
            }
            self.idle(Duration::from_millis(250)).await;
        }
        false
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

#[test]
fn egit_starts_in_a_browser_and_opens_a_repository_through_its_relay() {
    let Some((program, web)) = wanted() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    for (n, file) in ["one", "two", "three"].iter().enumerate() {
        sandbox
            .commit_file(&repo, file, &format!("{n}\n"), &format!("Add {file}"))
            .unwrap();
    }
    let relay = TestRelay::start_with(
        Path::new(env!("CARGO_BIN_EXE_egit-relay")),
        &["--web", web.to_str().unwrap()],
    )
    .unwrap();
    let browser = Browser::start(&program);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut page = Page::of(&browser).await;
        let address = format!(
            "http://{}/?open={}#{}",
            relay.address,
            encoded(&repo),
            relay.token
        );
        page.call("Page.navigate", json!({ "url": address })).await;
        // Started, the repository opened through the relay: the page says which is in front.
        let opened = page.titled("repo — egit", STARTING).await;
        page.idle(Duration::from_secs(3)).await;
        page.keep("browser-opened").await;
        assert!(opened, "never opened the repository: {:?}", page.wrong);
        // Out of the address bar: the token, and what was asked to be opened.
        let shown = page
            .call(
                "Runtime.evaluate",
                json!({ "expression": "location.href", "returnByValue": true }),
            )
            .await;
        assert_eq!(
            shown["result"]["value"],
            format!("http://{}/", relay.address)
        );

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
