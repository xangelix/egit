//! Running the real git: output, failures, sessions, cancellation, streaming, and locks.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::ops::ControlFlow;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use egit_git::testing::Sandbox;
use egit_git::{CancelToken, GitError, GitVersion, Wait as _};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

#[test]
fn reports_a_supported_version() {
    let sandbox = sandbox();
    assert!(sandbox.git().version() >= GitVersion::MINIMUM);
}

#[test]
fn collects_standard_output_and_feeds_standard_input() {
    let sandbox = sandbox();
    let completed = sandbox
        .git()
        .read("hash-object")
        .arg("--stdin")
        .stdin(b"hello\n".as_slice())
        .output()
        .wait()
        .unwrap();
    assert_eq!(
        completed.stdout,
        b"ce013625030ba8dba906f756967f9e9ca394464a\n"
    );
}

#[test]
fn a_failure_carries_gits_own_message() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    let error = sandbox
        .git()
        .read("rev-parse")
        .args(["--verify", "no-such-revision"])
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap_err();
    let failure = error.failure().expect("git ran and failed");
    assert_eq!(failure.code(), Some(128));
    assert_eq!(failure.stderr.message(), "Needed a single revision");
    assert!(!error.is_lock_contention());
}

#[test]
fn unchecked_output_returns_any_exit() {
    let sandbox = sandbox();
    let completed = sandbox
        .git()
        .read("config")
        .args(["--get", "no.such-key"])
        .output_unchecked()
        .wait()
        .unwrap();
    assert_eq!(completed.status.code(), Some(1));
    assert_eq!(completed.stdout, b"");
}

#[test]
fn a_missing_directory_is_named() {
    let sandbox = sandbox();
    let missing = sandbox.path("missing");
    let error = sandbox
        .git()
        .read("status")
        .current_dir(&missing)
        .output()
        .wait()
        .unwrap_err();
    assert!(matches!(&error, GitError::Io { .. }), "{error:?}");
    assert!(error.to_string().contains("isn't a directory"), "{error}");
}

#[cfg(target_os = "linux")]
#[test]
fn git_runs_in_a_session_of_its_own_without_a_terminal() {
    let sandbox = sandbox();
    let completed = sandbox
        .git()
        .read("stat")
        .config("alias.stat", "!cat /proc/self/stat")
        .output()
        .wait()
        .unwrap();
    let stat = String::from_utf8(completed.stdout).unwrap();
    // `pid (comm) state ppid pgrp session tty_nr …`; `comm` may contain spaces.
    let fields: Vec<&str> = stat[stat.rfind(')').unwrap() + 1..]
        .split_whitespace()
        .collect();
    let (group, session, terminal) = (fields[2], fields[3], fields[4]);
    let ours = rustix::process::getsid(None)
        .unwrap()
        .as_raw_pid()
        .to_string();
    assert_ne!(session, ours, "git shares egit's session");
    assert_eq!(group, session, "git doesn't lead its process group");
    assert_eq!(terminal, "0", "git has a controlling terminal");
}

#[test]
fn never_prompts_on_a_terminal() {
    let sandbox = sandbox();
    let started = Instant::now();
    let error = sandbox
        .git()
        .write("credential")
        .arg("fill")
        .stdin(b"protocol=https\nhost=example.invalid\n\n".as_slice())
        .output()
        .wait()
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    let message = error.failure().expect("git failed").stderr.message();
    assert!(message.contains("terminal prompts disabled"), "{message}");
}

#[test]
fn cancelling_stops_git_and_everything_it_started() {
    let sandbox = sandbox();
    let pid_file = sandbox.path("sleeper.pid");
    let token = CancelToken::new();
    let canceller = {
        let token = token.clone();
        let pid_file = pid_file.clone();
        thread::spawn(move || {
            wait_for(|| pid_file.exists());
            token.cancel();
        })
    };
    let started = Instant::now();
    let error = sandbox
        .git()
        .read("sleeper")
        .config("alias.sleeper", "!echo $$ > \"$PID_FILE\" && exec sleep 60")
        .env("PID_FILE", &pid_file)
        .cancel_token(&token)
        .output()
        .wait()
        .unwrap_err();
    canceller.join().unwrap();
    assert!(
        matches!(
            error,
            GitError::Cancelled {
                subcommand: "sleeper"
            }
        ),
        "{error:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(30));

    let sleeper = fs::read_to_string(&pid_file).unwrap();
    let proc_dir = format!("/proc/{}", sleeper.trim());
    wait_for(|| !is_running(Path::new(&proc_dir)));
}

#[test]
fn a_cancelled_token_starts_nothing() {
    let sandbox = sandbox();
    let token = CancelToken::new();
    token.cancel();
    let error = sandbox
        .git()
        .read("version")
        .cancel_token(&token)
        .output()
        .wait()
        .unwrap_err();
    assert!(matches!(
        error,
        GitError::Cancelled {
            subcommand: "version"
        }
    ));
}

#[test]
fn streams_everything_git_prints() {
    let sandbox = sandbox();
    let expected = (1..=200_000).fold(String::new(), |mut text, n| {
        writeln!(text, "{n}").unwrap();
        text
    });
    let mut received = Vec::new();
    let streamed = sandbox
        .git()
        .read("numbers")
        .config("alias.numbers", "!seq 1 200000")
        .stream(|chunk| {
            received.extend_from_slice(chunk);
            ControlFlow::Continue(())
        })
        .wait()
        .unwrap();
    assert!(!streamed.stopped);
    assert!(streamed.status.success());
    assert_eq!(received, expected.as_bytes());
}

#[test]
fn a_consumer_can_stop_an_endless_stream() {
    let sandbox = sandbox();
    let mut received = 0_usize;
    let streamed = sandbox
        .git()
        .read("forever")
        .config("alias.forever", "!yes")
        .stream(|chunk| {
            received += chunk.len();
            if received > 1024 * 1024 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .wait()
        .unwrap();
    assert!(streamed.stopped);
}

#[test]
fn collected_output_is_limited() {
    let sandbox = sandbox();
    let error = sandbox
        .git()
        .read("forever")
        .config("alias.forever", "!yes")
        .stdout_limit(1024)
        .output()
        .wait()
        .unwrap_err();
    assert!(
        matches!(error, GitError::OutputTooLarge { limit: 1024, .. }),
        "{error:?}"
    );
}

#[test]
fn a_write_waits_for_another_gits_lock() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    fs::write(repo.join("file"), "contents\n").unwrap();
    let lock = repo.join(".git/index.lock");
    fs::write(&lock, "").unwrap();
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        fs::remove_file(lock).unwrap();
    });
    sandbox
        .git()
        .write("add")
        .arg("file")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    releaser.join().unwrap();
    let staged = sandbox
        .git()
        .read("diff")
        .args(["--cached", "--name-only"])
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_eq!(staged.stdout, b"file\n");
}

#[test]
fn a_write_gives_up_on_a_lock_nobody_releases() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    fs::write(repo.join("file"), "contents\n").unwrap();
    fs::write(repo.join(".git/index.lock"), "").unwrap();
    let error = sandbox
        .git()
        .write("add")
        .arg("file")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap_err();
    assert!(error.is_lock_contention(), "{error:?}");
    let lock = error.failure().unwrap().held_lock().unwrap();
    assert!(lock.ends_with(b".git/index.lock"), "{lock}");
}

#[test]
fn a_read_never_takes_the_index_lock() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "contents\n", "first")
        .unwrap();
    let index = repo.join(".git/index");

    // Make the index's stat data stale, so a status that may write would refresh it.
    let file = fs::File::options()
        .write(true)
        .open(repo.join("file"))
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    let before = fs::metadata(&index).unwrap().modified().unwrap();
    thread::sleep(Duration::from_millis(20));

    sandbox
        .git()
        .read("status")
        .arg("--porcelain")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), before);

    // The same status allowed to write refreshes the index: the read above really held back.
    sandbox
        .git()
        .write("status")
        .arg("--porcelain")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_ne!(fs::metadata(&index).unwrap().modified().unwrap(), before);
}

/// Polls `condition` for up to 20 seconds.
fn wait_for(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the process behind a `/proc/<pid>` directory is alive (not gone, not a zombie).
fn is_running(proc_dir: &Path) -> bool {
    match fs::read_to_string(proc_dir.join("stat")) {
        Ok(stat) => {
            let state = stat[stat.rfind(')').unwrap() + 1..]
                .split_whitespace()
                .next();
            state != Some("Z")
        }
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => panic!("reading {}: {error}", proc_dir.display()),
    }
}
