//! Finding commits against sandboxed repositories: by message and author (case matched or
//! folded, text taken as it is), by path as `git log` lists them (a file, a folder, a side
//! branch's change, a stash left out), names resolved, and which partial clones keep their
//! trees.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, CommitFilter, ConfigPlace, ConfigSnapshot, ObjectId, Repository, find_commits,
    resolve_commit,
};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// What `filter` finds, sorted.
fn found(sandbox: &Sandbox, repo: &Path, filter: CommitFilter<'_>) -> Vec<ObjectId> {
    let repository = Repository::discover(sandbox.git(), repo).unwrap();
    let mut found = Vec::new();
    find_commits(
        sandbox.git(),
        &repository,
        filter,
        &CancelToken::new(),
        |id| {
            found.push(id);
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    found.sort_unstable();
    found
}

fn sorted(mut ids: Vec<ObjectId>) -> Vec<ObjectId> {
    ids.sort_unstable();
    ids
}

/// Commits with messages and authors to tell apart, a file in a folder changed on a side
/// branch merged in, and a stash.
fn searched(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "README", "hello\n", "Start")
        .unwrap();
    for (author, message) in [
        (
            "Ada Lovelace <ada@example.org>",
            "Fix a memory LEAK\n\nIt leaked.",
        ),
        ("Bo Diddley <bo@example.org>", "Add a feature (v1.2)"),
        ("Cy Twombly <cy@example.org>", "Tidy up"),
    ] {
        sandbox
            .run(
                &repo,
                "commit",
                &[
                    "--quiet",
                    "--allow-empty",
                    "--author",
                    author,
                    "--message",
                    message,
                ],
            )
            .unwrap();
    }
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "src/lib.rs", "fn main() {}\n", "Add the library")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&repo, "src/other.rs", "\n", "Add another file")
        .unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &["--quiet", "--no-ff", "--message", "Merge side", "side"],
        )
        .unwrap();
    std::fs::write(repo.join("src/lib.rs"), "changed\n").unwrap();
    sandbox.run(&repo, "stash", &["--quiet"]).unwrap();
    repo
}

#[test]
fn messages_and_authors_are_found_as_typed() {
    let sandbox = Sandbox::new().unwrap();
    let repo = searched(&sandbox);
    let message =
        |text, fold_case| found(&sandbox, &repo, CommitFilter::Message { text, fold_case });
    let leak = vec![rev(&sandbox, &repo, "main~4")];
    assert_eq!(message("leak", true), leak, "the body counts too");
    assert_eq!(message("LEAK", false), leak);
    assert_eq!(message("Leak", false), Vec::new());
    // Text, not a pattern.
    assert_eq!(
        message("(v1.2)", false),
        vec![rev(&sandbox, &repo, "main~3")]
    );
    assert_eq!(message("v1.2.", false), Vec::new());

    let author = |text, fold_case| found(&sandbox, &repo, CommitFilter::Author { text, fold_case });
    assert_eq!(author("ada", true), leak);
    assert_eq!(author("ada", false), leak, "the email holds it");
    assert_eq!(author("LOVELACE", true), leak);
    assert_eq!(
        author("bo@example.org>", false),
        vec![rev(&sandbox, &repo, "main~3")]
    );
    assert_eq!(
        author("Tidy", true),
        Vec::new(),
        "only the author is searched"
    );
}

#[test]
fn paths_find_the_commits_that_changed_them_as_git_log_lists_them() {
    let sandbox = Sandbox::new().unwrap();
    let repo = searched(&sandbox);
    let path = |path: &str| found(&sandbox, &repo, CommitFilter::Path(BStr::new(path)));
    let library = rev(&sandbox, &repo, "side");
    // The file came from the side branch: its history is followed there, past the merge; the
    // stash isn't searched.
    assert_eq!(path("src/lib.rs"), vec![library]);
    // Both sides changed the folder: the merge combined them.
    assert_eq!(
        path("src"),
        sorted(vec![
            library,
            rev(&sandbox, &repo, "main"),
            rev(&sandbox, &repo, "main^1")
        ])
    );
    assert_eq!(path("src/"), path("src"));
    assert_eq!(path("README"), vec![rev(&sandbox, &repo, "main~5")]);
    // Literally: no pattern matches.
    assert_eq!(path("src/*.rs"), Vec::new());
    assert_eq!(path("nowhere"), Vec::new());
}

#[test]
fn names_resolve_to_their_commits() {
    let sandbox = Sandbox::new().unwrap();
    let repo = searched(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let resolve = |name| resolve_commit(sandbox.git(), &repository, name).unwrap();
    let main = rev(&sandbox, &repo, "main");
    assert_eq!(resolve("main"), Some(main));
    assert_eq!(resolve("HEAD~1"), Some(rev(&sandbox, &repo, "main~1")));
    let abbreviated = main.abbreviated(10).to_string();
    assert_eq!(resolve(&abbreviated), Some(main));
    sandbox
        .run(
            &repo,
            "tag",
            &["--annotate", "--message", "One", "v1", "main~2"],
        )
        .unwrap();
    assert_eq!(
        resolve("v1"),
        Some(rev(&sandbox, &repo, "main~2")),
        "a tag is peeled"
    );
    assert_eq!(resolve("nowhere"), None);
    assert_eq!(resolve("--all"), None, "never an option");
    assert_eq!(resolve(""), None);
}

#[test]
fn a_partial_clone_has_every_tree_when_it_left_out_only_blobs() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "a\n", "One").unwrap();
    let every_tree = || {
        ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&repo))
            .unwrap()
            .has_every_tree()
    };
    assert!(every_tree(), "not a partial clone");
    let set = |key: &str, value: &str| {
        sandbox.run(&repo, "config", &[key, value]).unwrap();
    };
    set("remote.origin.url", "file:///nowhere");
    set("remote.origin.promisor", "true");
    assert!(!every_tree(), "no filter known");
    set("remote.origin.partialclonefilter", "blob:none");
    assert!(every_tree());
    set("remote.other.promisor", "true");
    set("remote.other.partialclonefilter", "tree:0");
    assert!(!every_tree());
    set("remote.other.partialclonefilter", "blob:limit=1m");
    assert!(every_tree());
}
