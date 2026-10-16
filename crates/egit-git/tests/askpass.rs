//! Git asking through the askpass bridge, end to end.
//!
//! This test runs without libtest's harness: started under the helper's name (git runs the
//! server's `egit-askpass` link, which points here), the executable is the helper; otherwise it
//! runs the tests below.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use egit_git::askpass::{AskpassServer, HELPER_NAME, Prompt, PromptKind, run_helper};
use egit_git::testing::Sandbox;
use egit_git::{Completed, Git, GitError};

const WAIT: Duration = Duration::from_secs(20);
const CREDENTIAL_REQUEST: &[u8] = b"protocol=https\nhost=example.invalid\n\n";

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    if Path::new(&program).file_name() == Some(HELPER_NAME.as_ref()) {
        return run_helper(&args.next().unwrap_or_default());
    }

    let tests: [(&str, fn()); 3] = [
        ("git asks through the window", git_asks_through_the_window),
        ("a refusal fails git", a_refusal_fails_git),
        (
            "a non-interactive command never asks",
            a_non_interactive_command_never_asks,
        ),
    ];
    let mut stdout = std::io::stdout();
    for (name, test) in tests {
        test();
        writeln!(stdout, "test {name} ... ok").unwrap();
    }
    ExitCode::SUCCESS
}

/// A sandbox whose git asks through a server, and the server's prompts.
struct Bridge {
    _server: AskpassServer,
    prompts: Receiver<Prompt>,
    git: Git,
    sandbox: Sandbox,
}

impl Bridge {
    fn new() -> Self {
        let sandbox = Sandbox::new().unwrap();
        let base = private_dir(&sandbox.path("askpass"));
        let executable = std::env::current_exe().unwrap();
        let (server, prompts) = AskpassServer::start_in(&base, &executable, || {}).unwrap();
        let git = sandbox.git_with(server.environment()).unwrap();
        Self {
            _server: server,
            prompts,
            git,
            sandbox,
        }
    }

    /// Runs `git credential fill` on its own thread: with no helper configured, git asks for a
    /// user name and a password.
    fn fill(&self, non_interactive: bool) -> thread::JoinHandle<Result<Completed, GitError>> {
        let mut command = self
            .git
            .write("credential")
            .arg("fill")
            .stdin(CREDENTIAL_REQUEST)
            .current_dir(self.sandbox.home())
            .prompt_context("Testing the bridge");
        if non_interactive {
            command = command.non_interactive();
        }
        thread::spawn(move || command.output())
    }

    fn next_prompt(&self) -> Prompt {
        self.prompts.recv_timeout(WAIT).unwrap()
    }
}

fn git_asks_through_the_window() {
    let bridge = Bridge::new();
    let git = bridge.fill(false);

    let user = bridge.next_prompt();
    assert_eq!(user.kind(), PromptKind::Visible);
    assert_eq!(user.text(), "Username for 'https://example.invalid': ");
    assert_eq!(user.context(), Some("Testing the bridge"));
    user.answer(b"me").unwrap();

    let password = bridge.next_prompt();
    assert_eq!(password.kind(), PromptKind::Secret);
    assert_eq!(
        password.text(),
        "Password for 'https://me@example.invalid': "
    );
    password.answer(b"correct horse").unwrap();

    let filled = git.join().unwrap().unwrap();
    let filled = String::from_utf8(filled.stdout).unwrap();
    assert!(filled.contains("username=me\n"), "{filled}");
    assert!(filled.contains("password=correct horse\n"), "{filled}");
}

fn a_refusal_fails_git() {
    let bridge = Bridge::new();
    let git = bridge.fill(false);
    bridge.next_prompt().refuse().unwrap();
    let error = git.join().unwrap().unwrap_err();
    assert!(error.failure().is_some(), "{error:?}");
}

fn a_non_interactive_command_never_asks() {
    let bridge = Bridge::new();
    let git = bridge.fill(true);
    let error = git.join().unwrap().unwrap_err();
    assert!(error.failure().is_some(), "{error:?}");
    assert_eq!(
        bridge
            .prompts
            .recv_timeout(Duration::from_millis(100))
            .unwrap_err(),
        RecvTimeoutError::Timeout
    );
}

/// A directory only this user may enter.
fn private_dir(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_owned()
}
