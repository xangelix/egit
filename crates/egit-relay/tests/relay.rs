//! The real `egit-relay`, as a browser and egit reach it: its web build served to its own host
//! alone, other sites and wrong tokens refused, git run through it (its output as it comes,
//! no more than the client made room for), cancelled, its questions relayed and answered, and
//! a session's programs stopped with it; downloads delivered to the browser (whole, its digest
//! matched; cut short where it doesn't; taken once; an answer that isn't one left to egit).

#![cfg(unix)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egit_git::askpass::PromptKind;
use egit_git::testing::{Sandbox, TestRelay};
use egit_git::{CancelToken, GitError, Wait as _};

fn relay(options: &[&str]) -> TestRelay {
    TestRelay::start_with(Path::new(env!("CARGO_BIN_EXE_egit-relay")), options).unwrap()
}

/// A request's whole answer, as the relay sent it.
fn request(relay: &TestRelay, head: &str) -> String {
    let mut stream = TcpStream::connect(&relay.address).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    String::from_utf8_lossy(&answer).into_owned()
}

#[test]
fn serves_its_web_build_to_its_own_host_alone() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<!doctype html>egit").unwrap();
    std::fs::write(web.path().join("egit_bg.wasm"), b"\0asm").unwrap();
    std::fs::write(web.path().join("egit_bg.wasm.gz"), b"compressed").unwrap();
    std::fs::write(web.path().with_file_name("secret"), "no").unwrap();
    let relay = relay(&[
        "--web",
        web.path().to_str().unwrap(),
        "--host",
        "proxy.example",
    ]);
    let address = &relay.address;

    let index = request(
        &relay,
        &format!("GET / HTTP/1.1\r\nHost: {address}\r\n\r\n"),
    );
    assert!(index.starts_with("HTTP/1.1 200 OK"), "{index}");
    assert!(
        index.contains("Content-Security-Policy: default-src 'self'"),
        "{index}"
    );
    assert!(index.ends_with("<!doctype html>egit"), "{index}");
    // Compressed where the browser takes it, as its own type.
    let wasm = request(
        &relay,
        &format!(
            "GET /egit_bg.wasm HTTP/1.1\r\nHost: {address}\r\nAccept-Encoding: br, gzip\r\n\r\n"
        ),
    );
    assert!(wasm.contains("Content-Type: application/wasm"), "{wasm}");
    assert!(wasm.contains("Content-Encoding: gzip"), "{wasm}");
    assert!(wasm.ends_with("compressed"), "{wasm}");
    // Nothing outside it, and nothing for another host; a proxy's name, and loopback names on
    // any port (a tunnel's), go through.
    let outside = request(
        &relay,
        &format!("GET /../secret HTTP/1.1\r\nHost: {address}\r\n\r\n"),
    );
    assert!(outside.starts_with("HTTP/1.1 404"), "{outside}");
    let rebound = request(&relay, "GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n");
    assert!(rebound.starts_with("HTTP/1.1 421"), "{rebound}");
    for host in ["proxy.example", "localhost:9999", "127.0.0.1:1234"] {
        let through = request(&relay, &format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert!(through.starts_with("HTTP/1.1 200"), "{host}: {through}");
    }
}

#[test]
fn refuses_another_site_s_page_and_a_wrong_token() {
    let relay = relay(&[]);
    let address = &relay.address;
    let other_site = request(
        &relay,
        &format!(
            "GET /relay HTTP/1.1\r\nHost: {address}\r\nOrigin: https://evil.example\r\n\
             Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        ),
    );
    assert!(other_site.starts_with("HTTP/1.1 403"), "{other_site}");

    let sandbox = Sandbox::new().unwrap();
    let wrong = relay.connect("not the token").unwrap();
    let refused = sandbox.git_through(&wrong, Vec::<(&str, &str)>::new());
    assert!(
        matches!(&refused, Err(error) if error.to_string().contains("token")),
        "{refused:?}"
    );
}

#[test]
fn runs_git_its_output_as_it_comes_and_cancelled() {
    let relay = relay(&[]);
    let sandbox = Sandbox::new().unwrap();
    let connection = relay.connect(&relay.token).unwrap();
    let git = sandbox
        .git_through(&connection, Vec::<(&str, &str)>::new())
        .unwrap();
    assert!(git.version().is_supported());
    assert_eq!(
        git.home_there(),
        std::env::var_os("HOME").as_deref().map(Path::new)
    );

    // Larger than what the relay sends before it is told it was taken in, many times over.
    let repo = sandbox.init("repo").unwrap();
    let big = vec![b'x'; 9 << 20];
    std::fs::write(repo.join("big"), &big).unwrap();
    let read = git.files().read(&repo.join("big")).wait().unwrap();
    assert_eq!(read.len(), big.len());
    // Standard input, too.
    let hashed = git
        .read("hash-object")
        .arg("--stdin")
        .stdin(big)
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_eq!(hashed.stdout.trim_ascii().len(), 40);
    // A folder that isn't one says so, as here.
    let nowhere = git
        .read("status")
        .current_dir(sandbox.path("nowhere"))
        .output()
        .wait()
        .unwrap_err();
    assert!(
        nowhere.to_string().contains("isn't a directory"),
        "{nowhere}"
    );

    // Cancelled, it stops: git and what it started.
    let pid_file = sandbox.path("sleeper.pid");
    let token = CancelToken::new();
    let canceller = {
        let (token, pid_file) = (token.clone(), pid_file.clone());
        std::thread::spawn(move || {
            while !pid_file.exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            token.cancel();
        })
    };
    let started = Instant::now();
    let error = git
        .read("sleeper")
        .config("alias.sleeper", "!echo $$ > \"$PID_FILE\" && exec sleep 60")
        .env("PID_FILE", &pid_file)
        .cancel_token(&token)
        .output()
        .wait()
        .unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(error, GitError::Cancelled { .. }), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(gone(&pid_file));
}

#[test]
fn relays_git_s_questions_and_their_answers() {
    let relay = relay(&[]);
    let sandbox = Sandbox::new().unwrap();
    let connection = relay.connect(&relay.token).unwrap();
    let (sender, prompts) = mpsc::channel();
    connection.send_prompts_to(sender, || {});
    let git = sandbox
        .git_through(&connection, Vec::<(&str, &str)>::new())
        .unwrap();
    let home = sandbox.home().to_owned();
    let filling = std::thread::spawn(move || {
        git.write("credential")
            .arg("fill")
            .stdin(b"protocol=https\nhost=example.invalid\n\n".as_slice())
            .prompt_context("Fetching origin")
            .current_dir(home)
            .output()
            .wait()
    });
    let wait = Duration::from_secs(30);
    let user = prompts.recv_timeout(wait).unwrap();
    assert_eq!(user.kind(), PromptKind::Visible);
    assert_eq!(user.context(), Some("Fetching origin"));
    user.answer(b"ada").unwrap();
    let password = prompts.recv_timeout(wait).unwrap();
    assert_eq!(password.kind(), PromptKind::Secret);
    password.answer(b"hunter2").unwrap();
    let filled = String::from_utf8(filling.join().unwrap().unwrap().stdout).unwrap();
    assert!(filled.contains("username=ada\n"), "{filled}");
    assert!(filled.contains("password=hunter2\n"), "{filled}");
}

#[test]
fn a_session_s_programs_stop_with_it() {
    let relay = relay(&[]);
    let sandbox = Sandbox::new().unwrap();
    let connection = relay.connect(&relay.token).unwrap();
    let git = sandbox
        .git_through(&connection, Vec::<(&str, &str)>::new())
        .unwrap();
    let pid_file = sandbox.path("sleeper.pid");
    let running = {
        let pid_file = pid_file.clone();
        std::thread::spawn(move || {
            git.read("sleeper")
                .config("alias.sleeper", "!echo $$ > \"$PID_FILE\" && exec sleep 60")
                .env("PID_FILE", &pid_file)
                .output()
                .wait()
        })
    };
    while !pid_file.exists() {
        std::thread::sleep(Duration::from_millis(10));
    }
    connection.close();
    let ended = running.join().unwrap();
    assert!(ended.is_err(), "{ended:?}");
    assert!(gone(&pid_file));
}

/// Whether the process whose id `pid_file` holds is gone (within a few seconds).
fn gone(pid_file: &Path) -> bool {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if !alive {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn makes_requests_for_its_browser_their_answers_as_they_come() {
    use egit_git::relay::{Fetch, FetchMode, Method};
    use zeroize::Zeroizing;

    // A host answering one request with a body larger than what the relay sends ahead.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let body = vec![b'y'; 5 << 20];
    let answering = {
        let body = body.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-RateLimit-Remaining: 41\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            String::from_utf8(head).unwrap()
        })
    };
    let relay = relay(&[]);
    let connection = relay.connect(&relay.token).unwrap();
    connection.welcome().wait().unwrap();
    let mut fetching = connection.fetch(Fetch {
        method: Method::Get,
        url: format!("http://{address}/api/v4/projects"),
        headers: vec![(
            "Authorization".to_owned(),
            Zeroizing::new(b"Bearer t0ken".to_vec()),
        )],
        body: None,
        mode: FetchMode::Api,
        deliver: None,
    });
    let head = fetching.head().wait().unwrap();
    assert_eq!(head.status, 200);
    assert!(
        head.headers
            .contains(&("x-ratelimit-remaining".to_owned(), "41".to_owned())),
        "{:?}",
        head.headers
    );
    let mut read = Vec::new();
    let mut buffer = vec![0; 100_000];
    loop {
        let got = fetching.read(&mut buffer).wait().unwrap();
        if got == 0 {
            break;
        }
        read.extend_from_slice(&buffer[..got]);
    }
    fetching.end().wait().unwrap();
    assert_eq!(read, body);
    let asked = answering.join().unwrap();
    assert!(
        asked.starts_with("GET /api/v4/projects HTTP/1.1"),
        "{asked}"
    );
    assert!(
        asked.contains("authorization: Bearer t0ken")
            || asked.contains("Authorization: Bearer t0ken"),
        "{asked}"
    );

    // A host that isn't there says so, in words.
    let mut nowhere = connection.fetch(Fetch {
        method: Method::Get,
        url: "http://127.0.0.1:1/".to_owned(),
        headers: Vec::new(),
        body: None,
        mode: FetchMode::Api,
        deliver: None,
    });
    let unreached = nowhere.head().wait().unwrap_err();
    assert!(unreached.contains("refused"), "{unreached}");
}

/// A host answering one request with `head` then `body`, on the loopback address; what was
/// asked comes from the thread.
fn answer_once(head: String, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap().to_string();
    let answering = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().unwrap();
        let mut asked = Vec::new();
        let mut byte = [0_u8; 1];
        while !asked.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            asked.push(byte[0]);
        }
        stream.write_all(head.as_bytes()).unwrap();
        // A browser may stop taking it: what it didn't take doesn't matter here.
        let _ = stream.write_all(&body);
        String::from_utf8(asked).unwrap()
    });
    (address, answering)
}

/// A download asked of `url` for the browser as `name`, `sha256` its digest.
fn delivered(url: String, name: &str, sha256: Option<[u8; 32]>) -> egit_git::relay::Fetch {
    egit_git::relay::Fetch {
        method: egit_git::relay::Method::Get,
        url,
        headers: Vec::new(),
        body: None,
        mode: egit_git::relay::FetchMode::Download,
        deliver: Some(egit_git::relay::Delivery {
            name: name.to_owned(),
            sha256,
        }),
    }
}

/// The browser opening `path` of the relay's: its whole answer, as bytes.
fn browser_takes(relay: &TestRelay, path: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(&relay.address).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}\r\n\r\n",
        relay.address
    )
    .unwrap();
    let mut answer = Vec::new();
    // Cut short, the connection ends as it is reset.
    let _ = stream.read_to_end(&mut answer);
    answer
}

/// Where an answer's body starts.
fn body_of(answer: &[u8]) -> &[u8] {
    let at = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    &answer[at + 4..]
}

#[test]
fn delivers_a_download_to_the_browser_whole_once() {
    use sha2::{Digest as _, Sha256};

    let body: Vec<u8> = (0..3_000_000_u32)
        .map(|n| u8::try_from(n % 251).unwrap())
        .collect();
    let digest: [u8; 32] = Sha256::digest(&body).into();
    let (address, answering) = answer_once(
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()),
        body.clone(),
    );
    let relay = relay(&[]);
    let connection = relay.connect(&relay.token).unwrap();
    connection.welcome().wait().unwrap();
    let mut fetching = connection.fetch(delivered(
        format!("http://{address}/artifacts.zip"),
        "artifacts – build.zip",
        Some(digest),
    ));
    assert_eq!(fetching.head().wait().unwrap().status, 200);
    let path = fetching.delivering().wait().unwrap();
    assert!(path.starts_with("/delivery/"), "{path}");
    let answer = browser_takes(&relay, &path);
    let head = String::from_utf8_lossy(&answer[..answer.len() - body.len()]).into_owned();
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(
        head.contains(&format!("Content-Length: {}\r\n", body.len())),
        "{head}"
    );
    assert!(
        head.contains(
            "Content-Disposition: attachment; filename=\"artifacts _ build.zip\"; \
             filename*=UTF-8''artifacts%20%E2%80%93%20build.zip"
        ),
        "{head}"
    );
    assert_eq!(body_of(&answer), body);
    // How much went, told as it went; its end, whole.
    let mut went = 0;
    while let Some(bytes) = fetching.delivered_past(went).wait() {
        went = bytes;
    }
    assert_eq!(went, u64::try_from(body.len()).unwrap());
    fetching.end().wait().unwrap();
    answering.join().unwrap();
    // Taken once.
    let again = browser_takes(&relay, &path);
    assert!(
        again.starts_with(b"HTTP/1.1 404"),
        "{}",
        String::from_utf8_lossy(&again)
    );
}

#[test]
fn a_download_not_matching_its_digest_is_cut_short() {
    // Its length unsaid: chunked to the browser, which never sees its end.
    let body = vec![b'z'; 700_000];
    let (address, answering) = answer_once(
        "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_owned(),
        body.clone(),
    );
    let relay = relay(&[]);
    let connection = relay.connect(&relay.token).unwrap();
    connection.welcome().wait().unwrap();
    let mut fetching = connection.fetch(delivered(
        format!("http://{address}/report.xml"),
        "report.xml",
        Some([0; 32]),
    ));
    fetching.head().wait().unwrap();
    let path = fetching.delivering().wait().unwrap();
    let answer = browser_takes(&relay, &path);
    let text = String::from_utf8_lossy(&answer);
    assert!(text.contains("Transfer-Encoding: chunked\r\n"), "{text}");
    assert!(!answer.ends_with(b"0\r\n\r\n"), "the browser saw it end");
    assert!(body_of(&answer).len() < body.len());
    let why = fetching.end().wait().unwrap_err();
    assert!(why.contains("SHA-256"), "{why}");
    drop(answering);
}

#[test]
fn an_answer_that_isnt_a_download_comes_to_egit() {
    // A redirect: its place for egit to follow, its words egit's.
    let (address, answering) = answer_once(
        "HTTP/1.1 302 Found\r\nLocation: https://objects.example/a.zip\r\n\
         Content-Length: 5\r\n\r\n"
            .to_owned(),
        b"moved".to_vec(),
    );
    let relay = relay(&[]);
    let connection = relay.connect(&relay.token).unwrap();
    connection.welcome().wait().unwrap();
    let mut fetching =
        connection.fetch(delivered(format!("http://{address}/a.zip"), "a.zip", None));
    let head = fetching.head().wait().unwrap();
    assert_eq!(head.status, 302);
    let mut buffer = vec![0; 64];
    let read = fetching.read(&mut buffer).wait().unwrap();
    assert_eq!(&buffer[..read], b"moved");
    fetching.end().wait().unwrap();
    assert!(fetching.delivering().wait().is_err());
    answering.join().unwrap();
}
