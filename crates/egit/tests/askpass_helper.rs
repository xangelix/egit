//! The `egit` executable answering git as its askpass helper (started through the
//! `egit-askpass` link the server makes).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use egit_git::askpass::{AskpassServer, PromptKind};
use egit_git::testing::Sandbox;

#[test]
fn the_executable_answers_git_as_its_askpass_helper() {
    let sandbox = Sandbox::new().unwrap();
    let base = sandbox.path("askpass");
    fs::create_dir(&base).unwrap();
    egit_git::testing::set_mode(&base, 0o700).unwrap();
    let (server, prompts) =
        AskpassServer::start_in(&base, Path::new(env!("CARGO_BIN_EXE_egit")), || {}).unwrap();
    let git = sandbox.git_with(server.environment()).unwrap();
    let home = sandbox.home().to_owned();
    let fill = thread::spawn(move || {
        git.write("credential")
            .arg("fill")
            .stdin(b"protocol=https\nhost=example.invalid\n\n".as_slice())
            .current_dir(home)
            .output()
    });

    let wait = Duration::from_secs(30);
    let user = prompts.recv_timeout(wait).unwrap();
    assert_eq!(user.kind(), PromptKind::Visible);
    user.answer(b"me").unwrap();
    let password = prompts.recv_timeout(wait).unwrap();
    assert_eq!(password.kind(), PromptKind::Secret);
    password.answer(b"s3cret").unwrap();

    let filled = String::from_utf8(fill.join().unwrap().unwrap().stdout).unwrap();
    assert!(filled.contains("username=me\n"), "{filled}");
    assert!(filled.contains("password=s3cret\n"), "{filled}");
    drop(server);
}

/// Whether `program` can be run; a test without it is skipped, unless every signer is required.
fn available(program: &str) -> bool {
    let found = std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    assert!(
        found || std::env::var_os("EGIT_REQUIRE_SIGNERS").is_none(),
        "{program} is needed to test signing"
    );
    found
}

#[test]
fn a_signing_keys_passphrase_is_asked_through_the_window() {
    if !available("ssh-keygen") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let key = sandbox.home().join("id_ed25519");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "hunter2", "-C", "tests", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(made.success());
    let public = fs::read_to_string(key.with_extension("pub")).unwrap();
    let allowed = sandbox.home().join("allowed_signers");
    fs::write(
        &allowed,
        format!("tests@egit.invalid namespaces=\"git\" {}", public.trim()),
    )
    .unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    fs::write(repo.join("file"), "two\n").unwrap();
    sandbox.run(&repo, "add", &["file"]).unwrap();
    for setting in [
        ["gpg.format", "ssh"],
        ["user.signingKey", key.to_str().unwrap()],
        ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
    ] {
        sandbox.run(&repo, "config", &setting).unwrap();
    }

    let base = sandbox.path("askpass");
    fs::create_dir(&base).unwrap();
    egit_git::testing::set_mode(&base, 0o700).unwrap();
    let (server, prompts) =
        AskpassServer::start_in(&base, Path::new(env!("CARGO_BIN_EXE_egit")), || {}).unwrap();
    let git = sandbox.git_with(server.environment()).unwrap();
    let repository = egit_git::Repository::discover(&git, &repo).unwrap();
    let committing = {
        let (git, repository) = (git.clone(), repository.clone());
        thread::spawn(move || {
            let commit = egit_git::NewCommit {
                message: "Signed".to_owned(),
                sign: Some(true),
                ..egit_git::NewCommit::default()
            };
            egit_git::create_commit(&git, &repository, &commit, &egit_git::CancelToken::new())
        })
    };

    let asked = prompts.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(asked.kind(), PromptKind::Secret);
    assert_eq!(asked.context(), Some("Signing the commit"));
    assert!(asked.text().contains("passphrase"), "{}", asked.text());
    asked.answer(b"hunter2").unwrap();
    committing.join().unwrap().unwrap();

    let head = git
        .read("rev-parse")
        .arg("HEAD")
        .current_dir(&repo)
        .output()
        .unwrap();
    let head = egit_git::ObjectId::from_hex(head.stdout.trim_ascii()).unwrap();
    let checks =
        egit_git::check_signatures(&git, &repository, &[head], &egit_git::CancelToken::new())
            .unwrap();
    assert_eq!(checks[0].verdict, egit_git::Verdict::Good);
    drop(server);
}
