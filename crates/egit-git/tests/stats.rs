//! What a repository's statistics read, with the real git: the files at a commit with their
//! sizes (a name holding a newline, a submodule left out), the lines each commit changed (a
//! root's, an edit's, a merge's and an empty commit's none, authors through the mailmap), and
//! contacts through the mailmap.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::ops::ControlFlow;
use std::path::Path;

use bstr::{BStr, BString};
use egit_git::testing::Sandbox;
use egit_git::{CancelToken, ObjectId, Repository, commit_lines, map_contacts, tree_files};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

#[test]
fn a_commit_s_files_are_listed_with_their_sizes() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    std::fs::create_dir(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.join("odd\nname.txt"), "abc").unwrap();
    sandbox.run(&repo, "add", &["."]).unwrap();
    // A submodule: a commit, not a file.
    let inner = sandbox.init("inner").unwrap();
    sandbox.commit_file(&inner, "x", "x\n", "Inner").unwrap();
    let gitlink = format!("160000,{},vendor", rev(&sandbox, &inner, "HEAD"));
    sandbox
        .run(&repo, "update-index", &["--add", "--cacheinfo", &gitlink])
        .unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "-m", "Files"])
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let listed = |sizes: bool| {
        let mut found: Vec<(BString, Option<u64>)> = Vec::new();
        tree_files(
            sandbox.git(),
            &repository,
            "HEAD",
            sizes,
            &CancelToken::new(),
            |path, size| found.push((path.to_owned(), size)),
        )
        .unwrap();
        found.sort();
        found
    };
    assert_eq!(
        listed(true),
        [
            (BString::from("odd\nname.txt"), Some(3)),
            (BString::from("src/main.rs"), Some(13)),
        ]
    );
    assert_eq!(listed(false)[1], (BString::from("src/main.rs"), None));
}

#[test]
fn each_commit_s_lines_are_counted_as_git_counts_them() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    std::fs::write(
        repo.join(".mailmap"),
        "Ada Lovelace <ada@example.com> <ada@old.example>\n",
    )
    .unwrap();
    sandbox
        .commit_file(&repo, "f", "one\ntwo\nthree\n", "Three lines")
        .unwrap();
    std::fs::write(repo.join("f"), "one\nTWO\nthree\n").unwrap();
    sandbox.run(&repo, "add", &["f"]).unwrap();
    sandbox
        .run(
            &repo,
            "commit",
            &["--quiet", "-m", "Edit", "--author=Ada <ada@old.example>"],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "commit",
            &["--quiet", "--allow-empty", "-m", "Empty"],
        )
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side", "HEAD~1"])
        .unwrap();
    sandbox.commit_file(&repo, "g", "g\n", "Side").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &["--quiet", "--no-ff", "-m", "Merge", "side"],
        )
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let commits = ["HEAD", "HEAD~1", "HEAD~2", "HEAD~3"].map(|name| rev(&sandbox, &repo, name));
    let mut counted: Vec<(BString, u64, u64)> = Vec::new();
    commit_lines(
        sandbox.git(),
        &repository,
        &commits,
        &CancelToken::new(),
        |lines| {
            assert!(lines.time > 0);
            counted.push((lines.email.to_owned(), lines.added, lines.removed));
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    let ada = BString::from("ada@example.com");
    let me = BString::from("tests@egit.invalid");
    assert_eq!(
        counted,
        [
            (me.clone(), 0, 0),
            (me.clone(), 0, 0),
            (ada, 1, 1),
            (me, 3, 0),
        ],
        "the merge, the empty commit, the edit (mailmapped by the untracked .mailmap), the root"
    );

    // Stopped after the first.
    let mut seen = 0;
    commit_lines(
        sandbox.git(),
        &repository,
        &commits,
        &CancelToken::new(),
        |_| {
            seen += 1;
            ControlFlow::Break(())
        },
    )
    .unwrap();
    assert_eq!(seen, 1);

    let mapped = map_contacts(
        sandbox.git(),
        &repository,
        &[
            BStr::new("Ada <ada@old.example>"),
            BStr::new("not a contact"),
            BStr::new("Bob <bob@example.com>"),
        ],
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(
        mapped,
        [
            "Ada Lovelace <ada@example.com>",
            "not a contact",
            "Bob <bob@example.com>"
        ]
    );
}
