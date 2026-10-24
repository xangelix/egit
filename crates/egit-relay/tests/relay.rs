//! The real `egit-relay`, as a browser and egit reach it: its web build served to its own host
//! alone, other sites and wrong tokens refused, git run through it (its output as it comes,
//! no more than the client made room for), cancelled, its questions relayed and answered, and
//! a session's programs stopped with it.

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
