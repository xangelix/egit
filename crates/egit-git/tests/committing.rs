//! Making commits with the real git: messages, amending, sign-off, trailers, hooks, and
//! declining to sign; and who wrote the recent ones.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, NewCommit, Repository, Wait as _, create_commit, recent_authors, reword_head,
};

/// `HEAD`'s message.
fn message(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox.run(repo, "log", &["-1", "--format=%B"]).unwrap();
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

fn commit(sandbox: &Sandbox, repo: &Path, new: &NewCommit) -> Result<String, egit_git::GitError> {
    let repository = Repository::discover(sandbox.git(), repo).wait().unwrap();
    create_commit(sandbox.git(), &repository, new, &CancelToken::new())
        .wait()
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
    egit_git::testing::set_mode(&path, 0o755).unwrap();
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
    egit_git::testing::set_mode(&signer, 0o755).unwrap();
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

#[test]
fn rewording_head_changes_only_its_message() {
    let sandbox = Sandbox::new().unwrap();
    // A commit, then a change staged and another not.
    let repo = staged(&sandbox);
    std::fs::write(repo.join("other"), "untouched\n").unwrap();
    let before = sandbox
        .run(&repo, "rev-parse", &["HEAD^{tree}"])
        .unwrap()
        .stdout;
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    reword_head(
        sandbox.git(),
        &repository,
        "First, said better\n\n# kept\nWhy.",
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(
        message(&sandbox, &repo),
        "First, said better\n\n# kept\nWhy."
    );
    // Its files as they were: what is staged stays staged, out of the commit.
    let after = sandbox
        .run(&repo, "rev-parse", &["HEAD^{tree}"])
        .unwrap()
        .stdout;
    assert_eq!(before, after);
    let staged = sandbox
        .run(&repo, "diff", &["--cached", "--name-only"])
        .unwrap()
        .stdout;
    assert_eq!(staged, b"file\n");
    let count = sandbox
        .run(&repo, "rev-list", &["--count", "HEAD"])
        .unwrap()
        .stdout;
    assert_eq!(count, b"1\n");
}

#[test]
fn trailers_name_co_authors_after_the_message() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    commit(
        &sandbox,
        &repo,
        &NewCommit {
            message: "Pair on it\n\nWhy.".to_owned(),
            sign_off: true,
            trailers: vec![
                "Co-authored-by: Ada Lovelace <ada@example.com>".to_owned(),
                "Co-authored-by: Alan Turing <alan@example.com>".to_owned(),
            ],
            ..NewCommit::default()
        },
    )
    .unwrap();
    let said = message(&sandbox, &repo);
    let mut lines = said.lines();
    assert_eq!(lines.next(), Some("Pair on it"));
    let trailers: Vec<&str> = said
        .lines()
        .skip_while(|line| !line.contains(':'))
        .collect();
    assert!(
        trailers.contains(&"Co-authored-by: Ada Lovelace <ada@example.com>"),
        "{said}"
    );
    assert!(
        trailers.contains(&"Co-authored-by: Alan Turing <alan@example.com>"),
        "{said}"
    );
    assert!(
        trailers
            .iter()
            .any(|line| line.starts_with("Signed-off-by: ")),
        "{said}"
    );
    // Parsed back as git wrote them.
    let raw = sandbox
        .run(&repo, "cat-file", &["commit", "HEAD"])
        .unwrap()
        .stdout;
    let names: Vec<String> = egit_git::Commit::parse(&raw)
        .unwrap()
        .co_authors()
        .into_iter()
        .map(|person| person.name.to_string())
        .collect();
    assert_eq!(names, ["Ada Lovelace", "Alan Turing"]);
}

#[test]
fn recent_authors_are_counted_through_the_mailmap() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    // Nobody before the first commit.
    let none = recent_authors(sandbox.git(), &repository, 100, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(none, []);
    let by = |author: &str, subject: &str| {
        sandbox
            .run(
                &repo,
                "commit",
                &[
                    "--allow-empty",
                    "-m",
                    subject,
                    &format!("--author={author}"),
                ],
            )
            .unwrap();
    };
    by("Ada <ada@example.com>", "one");
    by("Alan Turing <alan@example.com>", "two");
    by("A. Lovelace <ADA@example.com>", "three");
    by("Ada L <ada@old.example.com>", "four");
    // One person, however many addresses: the mailmap names them alike.
    std::fs::write(
        repo.join(".mailmap"),
        "Ada Lovelace <ada@example.com> <ada@old.example.com>\n",
    )
    .unwrap();
    let authors = recent_authors(sandbox.git(), &repository, 100, &CancelToken::new())
        .wait()
        .unwrap();
    let found: Vec<(String, String, u32)> = authors
        .iter()
        .map(|author| {
            (
                author.person.name.to_string(),
                author.person.email.to_string(),
                author.commits,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("Ada Lovelace".to_owned(), "ada@example.com".to_owned(), 3),
            ("Alan Turing".to_owned(), "alan@example.com".to_owned(), 1),
        ]
    );
    // At most the last `limit` commits.
    let last = recent_authors(sandbox.git(), &repository, 1, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].person.email, "ada@example.com");
}
