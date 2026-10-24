//! Commits picked anywhere in a history, with the real git: listed in the order given; made
//! into a series of patches (one text, or a folder; merges left out) that `git am` applies; and
//! their changes combined into one patch `git apply` takes, whether they follow each other or
//! not (a conflict refused), from a root commit too.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, CombineError, ObjectId, Repository, Wait as _, combined_patch, format_series,
    format_series_into, list_commits,
};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// `f` (one two three) committed as "One", then "Two" (`f`'s second line), "Three" (`g`),
/// "Four" (`f`'s third line, beside the second), "Five" (`h`), and a merge of a side branch
/// adding `side`; each named.
struct History {
    repo: PathBuf,
    one: ObjectId,
    two: ObjectId,
    three: ObjectId,
    four: ObjectId,
    five: ObjectId,
    merge: ObjectId,
}

fn history(sandbox: &Sandbox) -> History {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "one\ntwo\nthree\n", "One")
        .unwrap();
    sandbox
        .commit_file(&repo, "f", "one\nTWO\nthree\n", "Two")
        .unwrap();
    sandbox.commit_file(&repo, "g", "g\n", "Three").unwrap();
    sandbox
        .commit_file(&repo, "f", "one\nTWO\nTHREE\n", "Four")
        .unwrap();
    sandbox.commit_file(&repo, "h", "h\n", "Five").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side", "HEAD~1"])
        .unwrap();
    sandbox.commit_file(&repo, "side", "s\n", "Side").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &["--quiet", "--no-ff", "--message", "Merge side", "side"],
        )
        .unwrap();
    History {
        one: rev(sandbox, &repo, "HEAD~5"),
        two: rev(sandbox, &repo, "HEAD~4"),
        three: rev(sandbox, &repo, "HEAD~3"),
        four: rev(sandbox, &repo, "HEAD~2"),
        five: rev(sandbox, &repo, "HEAD~1"),
        merge: rev(sandbox, &repo, "HEAD"),
        repo,
    }
}

/// Applies `patch` with `git apply --index` onto `at`, detached; returns the tree it leaves
/// staged.
fn applied_at(sandbox: &Sandbox, repo: &Path, at: &str, patch: &[u8]) -> ObjectId {
    sandbox
        .run(repo, "switch", &["--quiet", "--detach", at])
        .unwrap();
    let file = sandbox.path("combined.patch");
    std::fs::write(&file, patch).unwrap();
    sandbox
        .run(repo, "apply", &["--index", file.to_str().unwrap()])
        .unwrap();
    let out = sandbox.run(repo, "write-tree", &[]).unwrap();
    let tree = ObjectId::from_hex(out.stdout.trim_ascii()).unwrap();
    sandbox.run(repo, "reset", &["--quiet", "--hard"]).unwrap();
    sandbox.run(repo, "switch", &["--quiet", "main"]).unwrap();
    tree
}

#[test]
fn commits_are_listed_in_the_order_given() {
    let sandbox = Sandbox::new().unwrap();
    let h = history(&sandbox);
    let repository = Repository::discover(sandbox.git(), &h.repo).wait().unwrap();
    let listed = list_commits(
        sandbox.git(),
        &repository,
        &[h.four, h.two, h.merge],
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    let subjects: Vec<&str> = listed
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Four", "Two", "Merge side"]);
    assert_eq!(listed[0].parents, [h.three]);
    assert_eq!(listed[2].parents.len(), 2, "a merge");
    assert_eq!(
        list_commits(sandbox.git(), &repository, &[], &CancelToken::new())
            .wait()
            .unwrap(),
        []
    );
}

#[test]
fn commits_picked_apart_make_a_series_that_applies() {
    let sandbox = Sandbox::new().unwrap();
    let h = history(&sandbox);
    let repository = Repository::discover(sandbox.git(), &h.repo).wait().unwrap();
    let cancel = CancelToken::new();
    // The newest first, as a history lists them; the merge is left out.
    let picked = [h.merge, h.five, h.three];
    let series = format_series(sandbox.git(), &repository, &picked, &cancel)
        .wait()
        .unwrap();
    assert_eq!(series.patches, 2);
    let text = String::from_utf8(series.text.clone()).unwrap();
    let first = text.find("Subject: [PATCH 1/2] Three").unwrap();
    let second = text.find("Subject: [PATCH 2/2] Five").unwrap();
    assert!(first < second, "the oldest first");

    let folder = sandbox.path("patches");
    std::fs::create_dir(&folder).unwrap();
    let written = format_series_into(sandbox.git(), &repository, &picked, &folder, &cancel)
        .wait()
        .unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["0001-Three.patch", "0002-Five.patch"]);

    // `git am` takes the text onto Two: Three and Five follow it.
    sandbox
        .run(
            &h.repo,
            "switch",
            &["--quiet", "--create", "applied", &h.two.to_string()],
        )
        .unwrap();
    let mailbox = sandbox.path("series.mbox");
    std::fs::write(&mailbox, &series.text).unwrap();
    sandbox
        .run(&h.repo, "am", &["--quiet", mailbox.to_str().unwrap()])
        .unwrap();
    let out = sandbox.run(&h.repo, "log", &["--format=%s", "-3"]).unwrap();
    assert_eq!(out.stdout.trim_ascii(), b"Five\nThree\nTwo");
}

#[test]
fn commits_combine_into_one_patch() {
    let sandbox = Sandbox::new().unwrap();
    let h = history(&sandbox);
    let repository = Repository::discover(sandbox.git(), &h.repo).wait().unwrap();
    let cancel = CancelToken::new();
    let tree = |name: String| rev(&sandbox, &h.repo, &format!("{name}^{{tree}}"));

    // Following each other: Two's parent to Four.
    let patch = combined_patch(
        sandbox.git(),
        &repository,
        &[h.four, h.three, h.two],
        &cancel,
    )
    .wait()
    .unwrap();
    let text = String::from_utf8(patch.clone()).unwrap();
    assert!(
        text.starts_with("The changes of 3 commits together, the oldest first:\n"),
        "{text}"
    );
    assert!(text.find(" Two\n").unwrap() < text.find(" Four\n").unwrap());
    assert_eq!(
        applied_at(&sandbox, &h.repo, &h.one.to_string(), &patch),
        tree(h.four.to_string())
    );

    // Apart: Three and Five onto One, without Two and Four.
    let patch = combined_patch(sandbox.git(), &repository, &[h.five, h.three], &cancel)
        .wait()
        .unwrap();
    let made = applied_at(&sandbox, &h.repo, &format!("{}", h.two), &patch);
    sandbox
        .run(
            &h.repo,
            "switch",
            &["--quiet", "--detach", &h.two.to_string()],
        )
        .unwrap();
    std::fs::write(h.repo.join("g"), "g\n").unwrap();
    std::fs::write(h.repo.join("h"), "h\n").unwrap();
    sandbox.run(&h.repo, "add", &["g", "h"]).unwrap();
    let out = sandbox.run(&h.repo, "write-tree", &[]).unwrap();
    assert_eq!(made, ObjectId::from_hex(out.stdout.trim_ascii()).unwrap());
    sandbox
        .run(&h.repo, "reset", &["--quiet", "--hard"])
        .unwrap();
    sandbox
        .run(&h.repo, "switch", &["--quiet", "main"])
        .unwrap();

    // From the root: everything One and Two hold.
    let patch = combined_patch(sandbox.git(), &repository, &[h.two, h.one], &cancel)
        .wait()
        .unwrap();
    assert!(String::from_utf8_lossy(&patch).contains("new file mode 100644"));

    // Four without Two, its neighbour: a conflict, named.
    let refused = combined_patch(sandbox.git(), &repository, &[h.four, h.one], &cancel).wait();
    assert!(
        matches!(refused, Err(CombineError::Conflict(commit)) if commit == h.four),
        "{refused:?}"
    );
    assert!(matches!(
        combined_patch(sandbox.git(), &repository, &[], &cancel).wait(),
        Err(CombineError::Nothing)
    ));
}
