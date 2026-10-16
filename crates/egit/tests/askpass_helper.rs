//! The `egit` executable answering git as its askpass helper (started through the
//! `egit-askpass` link the server makes).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
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
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
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
