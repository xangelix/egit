//! Making commits with the real git: messages, amending, sign-off, hooks, and declining to sign.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use egit_git::testing::Sandbox;
use egit_git::{CancelToken, NewCommit, Repository, create_commit};

/// `HEAD`'s message.
fn message(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox.run(repo, "log", &["-1", "--format=%B"]).unwrap();
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

fn commit(sandbox: &Sandbox, repo: &Path, new: &NewCommit) -> Result<String, egit_git::GitError> {
    let repository = Repository::discover(sandbox.git(), repo).unwrap();
    create_commit(sandbox.git(), &repository, new, &CancelToken::new())
        .map(|committed| committed.output)
}

/// A repository with one commit and a change staged.
fn staged(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    std::fs::write(repo.join("file"), "two\n").unwrap();
    sandbox.run(&repo, "add", &["file"]).unwrap();
    repo
}

#[test]
fn commits_the_index_with_the_message_as_written() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    let output = commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Change the file\n\n# Not a comment\nBody.  \n\n\n".to_owned(),
            ..NewCommit::default()
        },
    )
    .unwrap();
    assert!(output.contains("Change the file"), "{output}");
    assert_eq!(
        message(&sandbox, &repo),
        "Change the file\n\n# Not a comment\nBody."
    );
    // Nothing left to commit: git says so.
    let refused = commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Again".to_owned(),
            ..NewCommit::default()
        },
    );
    assert!(refused.is_err());
}

#[test]
fn amends_and_signs_off() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Draft".to_owned(),
            ..NewCommit::default()
        },
    )
    .unwrap();
    commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Final".to_owned(),
            amend: true,
            sign_off: true,
            ..NewCommit::default()
        },
    )
    .unwrap();
    assert_eq!(
        message(&sandbox, &repo),
        "Final\n\nSigned-off-by: egit tests <tests@egit.invalid>"
    );
    let count = sandbox
        .run(&repo, "rev-list", &["--count", "HEAD"])
        .unwrap();
    assert_eq!(count.stdout.trim_ascii(), b"2", "amended, not added");
}

/// Installs a hook printing `say` and exiting with `code`.
fn hook(repo: &Path, name: &str, say: &str, code: u8) {
    let path = repo.join(".git/hooks").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\necho '{say}'\nexit {code}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_hooks_output_is_shown_and_its_refusal_stops_the_commit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    hook(&repo, "pre-commit", "checked 1 file", 0);
    let output = commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Checked".to_owned(),
            ..NewCommit::default()
        },
    )
    .unwrap();
    assert!(output.contains("checked 1 file"), "{output}");

    std::fs::write(repo.join("file"), "three\n").unwrap();
    sandbox.run(&repo, "add", &["file"]).unwrap();
    hook(&repo, "commit-msg", "rejected: say why", 1);
    let refused = commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Refused".to_owned(),
            ..NewCommit::default()
        },
    )
    .unwrap_err();
    let stderr = refused.failure().unwrap().stderr.as_bytes();
    assert!(
        String::from_utf8_lossy(stderr).contains("rejected: say why"),
        "{refused}"
    );
    assert_eq!(message(&sandbox, &repo), "Checked");
}

#[test]
fn declining_to_sign_overrides_the_configuration() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    // A signer that always fails: had it run, the commit would have failed.
    let signer = sandbox.path("refuse-to-sign");
    std::fs::write(&signer, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&signer, std::fs::Permissions::from_mode(0o755)).unwrap();
    sandbox
        .run(&repo, "config", &["commit.gpgSign", "true"])
        .unwrap();
    sandbox
        .run(&repo, "config", &["gpg.program", signer.to_str().unwrap()])
        .unwrap();
    let configured = commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Signed as configured".to_owned(),
            ..NewCommit::default()
        },
    );
    assert!(configured.is_err(), "the configured signer ran and failed");
    commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Not signed".to_owned(),
            sign: Some(false),
            ..NewCommit::default()
        },
    )
    .unwrap();
    assert_eq!(message(&sandbox, &repo), "Not signed");
}
