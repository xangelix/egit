//! Hooks with the real git: which a repository has (its hooks folder, `core.hooksPath`), and
//! what a commit's hooks print, as it comes, into a transcript: their colours, a refusal, skipped
//! with `--no-verify`, and cancelled part-way.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egit_git::testing::{Sandbox, set_mode};
use egit_git::{
    CancelToken, Color, GitError, LineKind, NewCommit, Repository, Transcript, create_commit,
    installed_hooks,
};

/// Writes `script` as the hook `name` in `folder`, runnable unless `runnable` is false.
fn hook(folder: &Path, name: &str, script: &str, runnable: bool) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    set_mode(&path, if runnable { 0o755 } else { 0o644 }).unwrap();
    path
}

/// A repository with one commit and a change staged.
fn staged(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "one\n", "First").unwrap();
    std::fs::write(repo.join("a"), "two\n").unwrap();
    sandbox.run(&repo, "add", &["a"]).unwrap();
    repo
}

/// Every line's kind and text.
fn lines(transcript: &Transcript) -> Vec<(LineKind, String)> {
    transcript.read(|log| {
        (0..log.len())
            .filter_map(|at| log.line(at))
            .map(|line| (line.kind, line.text.clone()))
            .collect()
    })
}

fn commit(message: &str) -> NewCommit {
    NewCommit {
        message: message.to_owned(),
        sign: Some(false),
        ..NewCommit::default()
    }
}

#[test]
fn the_hooks_a_repository_has_are_the_runnable_ones_where_git_looks() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    let hooks_folder = repo.join(".git/hooks");
    hook(&hooks_folder, "pre-commit", "exit 0", true);
    hook(&hooks_folder, "commit-msg", "exit 0", false);
    hook(&hooks_folder, "pre-push.sample", "exit 0", true);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let hooks = installed_hooks(sandbox.git(), &repository).unwrap();
    assert_eq!(
        hooks.names,
        ["pre-commit"],
        "not the unrunnable, nor a sample"
    );
    assert!(!hooks.configured);
    assert!(hooks.folder.ends_with(".git/hooks"));
    assert_eq!(
        hooks.of(&["pre-commit", "commit-msg", "post-commit"]),
        ["pre-commit"]
    );

    // A team's folder, by `core.hooksPath`.
    hook(&repo.join(".husky"), "pre-push", "exit 0", true);
    sandbox
        .run(&repo, "config", &["core.hooksPath", ".husky"])
        .unwrap();
    let hooks = installed_hooks(sandbox.git(), &repository).unwrap();
    assert_eq!(hooks.names, ["pre-push"]);
    assert!(hooks.configured && hooks.folder.ends_with(".husky"));
}

#[test]
fn a_commit_s_hooks_print_into_its_transcript() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    let hooks = repo.join(".git/hooks");
    hook(
        &hooks,
        "pre-commit",
        r#"printf '\033[32mchecking\033[0m style\n'; echo "colour: $PRE_COMMIT_COLOR""#,
        true,
    );
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let transcript = Transcript::new();
    let git = sandbox.git().with_transcript(&transcript);
    create_commit(&git, &repository, &commit("Checked"), &CancelToken::new()).unwrap();
    let said = lines(&transcript);
    assert_eq!(said[0].0, LineKind::Command);
    assert!(said[0].1.starts_with("git commit --file=-"), "{said:?}");
    assert!(
        said.contains(&(LineKind::Err, "checking style".to_owned())),
        "a hook's output goes to standard error: {said:?}"
    );
    assert!(said.contains(&(LineKind::Err, "colour: always".to_owned())));
    assert!(
        said.iter()
            .any(|(kind, text)| *kind == LineKind::Out && text.contains("Checked")),
        "git's own summary: {said:?}"
    );
    transcript.read(|log| {
        let checking = (0..log.len())
            .filter_map(|at| log.line(at))
            .find(|line| line.text == "checking style")
            .unwrap();
        assert_eq!(checking.spans[0].style.color, Some(Color::Indexed(2)));
    });
    // A reading command prints nothing into it; nor does a git without a transcript.
    let before = transcript.updates();
    sandbox
        .git()
        .write("status")
        .current_dir(&repo)
        .output()
        .unwrap();
    git.read("status").current_dir(&repo).output().unwrap();
    assert_eq!(transcript.updates(), before);
}

#[test]
fn a_hook_s_refusal_and_skipping_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    hook(
        &repo.join(".git/hooks"),
        "pre-commit",
        "echo 'lint: 2 problems' >&2; exit 1",
        true,
    );
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let transcript = Transcript::new();
    let git = sandbox.git().with_transcript(&transcript);
    let refused = create_commit(&git, &repository, &commit("Refused"), &CancelToken::new());
    assert!(matches!(refused, Err(GitError::Failed(_))));
    let said = lines(&transcript);
    assert!(said.contains(&(LineKind::Err, "lint: 2 problems".to_owned())));
    assert!(
        matches!(said.last(), Some((LineKind::Ended { success: false }, _))),
        "{said:?}"
    );

    // Skipped: made.
    let skipping = NewCommit {
        no_verify: true,
        ..commit("Skipped")
    };
    create_commit(&git, &repository, &skipping, &CancelToken::new()).unwrap();
    let head = sandbox.run(&repo, "log", &["-1", "--format=%s"]).unwrap();
    assert_eq!(head.stdout.trim_ascii(), b"Skipped");
}

#[test]
fn a_hook_cancelled_part_way_makes_no_commit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    hook(
        &repo.join(".git/hooks"),
        "pre-commit",
        "echo 'running the tests'; sleep 30",
        true,
    );
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let transcript = Transcript::new();
    let git = sandbox.git().with_transcript(&transcript);
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let started = Instant::now();
    let watching = transcript.clone();
    std::thread::spawn(move || {
        // Once the hook said it started.
        while !watching.read(|log| {
            (0..log.len()).any(|at| {
                log.line(at)
                    .is_some_and(|line| line.text == "running the tests")
            })
        }) {
            std::thread::sleep(Duration::from_millis(20));
        }
        stopper.cancel();
    });
    let made = create_commit(&git, &repository, &commit("Never"), &cancel);
    assert!(matches!(made, Err(GitError::Cancelled { .. })), "{made:?}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the hook was stopped"
    );
    let head = sandbox.run(&repo, "log", &["-1", "--format=%s"]).unwrap();
    assert_eq!(head.stdout.trim_ascii(), b"First");
    let said = lines(&transcript);
    assert_eq!(
        said.last(),
        Some(&(LineKind::Ended { success: false }, "Cancelled.".to_owned()))
    );
}
