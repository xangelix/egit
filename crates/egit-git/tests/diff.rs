//! One file's diff, from the real git.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use bstr::{BStr, ByteSlice as _};
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, DiffLineKind, DiffOptions, FileDiff, ObjectId, Repository, Uncommitted,
    WHOLE_FILE, Wait as _, file_diff, uncommitted_diff,
};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// The diff of `paths` in `HEAD` (against its parent, if it has one).
fn diff_head(sandbox: &Sandbox, repo: &Path, paths: &[&str], context: u32) -> FileDiff {
    let repository = Repository::discover(sandbox.git(), repo).wait().unwrap();
    let commit = rev(sandbox, repo, "HEAD");
    let parent = sandbox
        .run(repo, "rev-parse", &["--verify", "--quiet", "HEAD^"])
        .ok()
        .and_then(|out| ObjectId::from_hex(out.stdout.trim_ascii()));
    let paths: Vec<&BStr> = paths.iter().copied().map(BStr::new).collect();
    let options = DiffOptions {
        context,
        ..DiffOptions::default()
    };
    file_diff(
        sandbox.git(),
        &repository,
        commit,
        parent,
        &paths,
        &options,
        &CancelToken::new(),
    )
    .wait()
    .unwrap()
}

/// Each line as `kind old new text`.
fn shown(diff: &FileDiff) -> Vec<String> {
    let texts: Vec<&[u8]> = diff.text.lines().collect();
    diff.lines
        .iter()
        .zip(texts)
        .map(|(line, text)| {
            let kind = match line.kind {
                DiffLineKind::Hunk => "@",
                DiffLineKind::Context => " ",
                DiffLineKind::Added => "+",
                DiffLineKind::Removed => "-",
            };
            format!("{kind} {} {} {}", line.old, line.new, text.to_str_lossy())
        })
        .collect()
}

/// `count` lines `word 1` to `word count`.
fn numbered(word: &str, count: u32) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for line in 1..=count {
        let _ = writeln!(text, "{word} {line}");
    }
    text
}

fn repo_with(sandbox: &Sandbox, file: &str, contents: &str) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, file, contents, "Start").unwrap();
    repo
}

#[test]
fn a_change_is_numbered_in_both_files() {
    let sandbox = sandbox();
    let repo = repo_with(
        &sandbox,
        "f.txt",
        "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
    );
    sandbox
        .commit_file(
            &repo,
            "f.txt",
            "one\ntwo\n3\nfour\nfive\nsix\nseven\neight\n",
            "Change",
        )
        .unwrap();
    let diff = diff_head(&sandbox, &repo, &["f.txt"], 1);
    assert!(!diff.binary);
    assert_eq!(
        shown(&diff),
        [
            "@ 0 0 @@ -2,3 +2,3 @@ one",
            "  2 2 two",
            "- 3 0 three",
            "+ 0 3 3",
            "  4 4 four",
            "@ 0 0 @@ -7 +7,2 @@ six",
            "  7 7 seven",
            "+ 0 8 eight",
        ]
    );
}

#[test]
fn the_whole_file_comes_as_one_hunk() {
    let sandbox = sandbox();
    let lines = numbered("line", 50);
    let repo = repo_with(&sandbox, "f", &lines);
    let changed = lines.replace("line 25\n", "line twenty-five\n");
    sandbox.commit_file(&repo, "f", &changed, "Change").unwrap();
    let diff = diff_head(&sandbox, &repo, &["f"], WHOLE_FILE);
    let hunks = diff
        .lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Hunk)
        .count();
    assert_eq!(hunks, 1);
    assert_eq!(diff.lines.len(), 1 + 50 + 1);
}

#[test]
fn a_rename_shows_only_what_changed() {
    let sandbox = sandbox();
    let body = numbered("row", 20);
    let repo = repo_with(&sandbox, "old name.txt", &body);
    sandbox
        .run(&repo, "mv", &["old name.txt", "new name.txt"])
        .unwrap();
    std::fs::write(
        repo.join("new name.txt"),
        body.replace("row 10\n", "row ten\n"),
    )
    .unwrap();
    sandbox.run(&repo, "add", &["--all"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Rename"])
        .unwrap();
    let diff = diff_head(&sandbox, &repo, &["old name.txt", "new name.txt"], 0);
    assert_eq!(
        shown(&diff),
        [
            "@ 0 0 @@ -10 +10 @@ row 9",
            "- 10 0 row 10",
            "+ 0 10 row ten"
        ]
    );
}

#[test]
fn a_first_commit_adds_and_a_deletion_removes() {
    let sandbox = sandbox();
    let repo = repo_with(&sandbox, "f", "a\nb");
    let added = diff_head(&sandbox, &repo, &["f"], 3);
    assert_eq!(
        shown(&added),
        ["@ 0 0 @@ -0,0 +1,2 @@", "+ 0 1 a", "+ 0 2 b"]
    );
    assert!(added.lines[2].missing_newline);

    sandbox.run(&repo, "rm", &["--quiet", "f"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Remove"])
        .unwrap();
    let removed = diff_head(&sandbox, &repo, &["f"], 3);
    assert_eq!(
        shown(&removed),
        ["@ 0 0 @@ -1,2 +0,0 @@", "- 1 0 a", "- 2 0 b"]
    );
}

#[test]
fn a_binary_file_says_so() {
    let sandbox = sandbox();
    let repo = repo_with(&sandbox, "data.bin", "\0\u{1}\u{2}");
    sandbox
        .commit_file(&repo, "data.bin", "\0\u{1}\u{3}", "Change")
        .unwrap();
    let diff = diff_head(&sandbox, &repo, &["data.bin"], 3);
    assert!(diff.binary);
    assert_eq!(diff.lines, []);
}

#[test]
fn paths_are_never_patterns() {
    let sandbox = sandbox();
    let repo = repo_with(&sandbox, "a*b [x].txt", "first\n");
    sandbox
        .commit_file(&repo, "axxb x.txt", "decoy\n", "Decoy")
        .unwrap();
    sandbox
        .commit_file(&repo, "a*b [x].txt", "second\n", "Change")
        .unwrap();
    let diff = diff_head(&sandbox, &repo, &["a*b [x].txt"], 3);
    assert_eq!(
        shown(&diff),
        ["@ 0 0 @@ -1 +1 @@", "- 1 0 first", "+ 0 1 second"]
    );
}

#[test]
fn whitespace_alone_is_left_out_when_asked() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb  c\nd\n", "Add")
        .unwrap();
    sandbox
        .commit_file(&repo, "f", "a\n  b c\nD\n", "Indent")
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let options = DiffOptions {
        context: 0,
        ignore_whitespace: true,
        ..DiffOptions::default()
    };
    let diff = file_diff(
        sandbox.git(),
        &repository,
        rev(&sandbox, &repo, "HEAD"),
        Some(rev(&sandbox, &repo, "HEAD^")),
        &[BStr::new("f")],
        &options,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    // The hunk header names the line before as its context, as git does.
    assert_eq!(
        shown(&diff),
        ["@ 0 0 @@ -3 +3 @@ b  c", "- 3 0 d", "+ 0 3 D"]
    );
    // The work tree's too, untracked or not.
    std::fs::write(repo.join("f"), "a\n  b c\n D\n").unwrap();
    let unstaged = uncommitted_diff(
        sandbox.git(),
        &repository,
        &[BStr::new("f")],
        Uncommitted::Unstaged,
        &options,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert!(unstaged.lines.is_empty(), "{:?}", shown(&unstaged));
}

/// The diff of the uncommitted changes `which` to `paths`.
fn uncommitted(sandbox: &Sandbox, repo: &Path, paths: &[&str], which: Uncommitted) -> FileDiff {
    let repository = Repository::discover(sandbox.git(), repo).wait().unwrap();
    let paths: Vec<&BStr> = paths.iter().copied().map(BStr::new).collect();
    uncommitted_diff(
        sandbox.git(),
        &repository,
        &paths,
        which,
        &DiffOptions::default(),
        &CancelToken::new(),
    )
    .wait()
    .unwrap()
}

#[test]
fn shows_what_is_staged_and_what_is_not() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "one\ntwo\n", "Add")
        .unwrap();
    std::fs::write(repo.join("f"), "one\nTWO\n").unwrap();
    sandbox.run(&repo, "add", &["f"]).unwrap();
    std::fs::write(repo.join("f"), "one\nTWO\nthree\n").unwrap();

    let staged = uncommitted(&sandbox, &repo, &["f"], Uncommitted::Staged);
    assert_eq!(
        shown(&staged),
        [
            "@ 0 0 @@ -1,2 +1,2 @@",
            "  1 1 one",
            "- 2 0 two",
            "+ 0 2 TWO"
        ]
    );
    let unstaged = uncommitted(&sandbox, &repo, &["f"], Uncommitted::Unstaged);
    assert_eq!(
        shown(&unstaged),
        [
            "@ 0 0 @@ -1,2 +1,3 @@",
            "  1 1 one",
            "  2 2 TWO",
            "+ 0 3 three"
        ]
    );
}

#[test]
fn shows_a_new_file_before_the_first_commit_and_untracked() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    std::fs::write(repo.join("staged"), "a\n").unwrap();
    sandbox.run(&repo, "add", &["staged"]).unwrap();
    let staged = uncommitted(&sandbox, &repo, &["staged"], Uncommitted::Staged);
    assert_eq!(shown(&staged), ["@ 0 0 @@ -0,0 +1 @@", "+ 0 1 a"]);

    // Untracked: through the file's line-ending conversion, as `git add` would store it.
    sandbox
        .run(&repo, "config", &["core.autocrlf", "true"])
        .unwrap();
    std::fs::write(repo.join("new file.txt"), "x\r\ny\r\n").unwrap();
    let untracked = uncommitted(&sandbox, &repo, &["new file.txt"], Uncommitted::Untracked);
    assert_eq!(
        shown(&untracked),
        ["@ 0 0 @@ -0,0 +1,2 @@", "+ 0 1 x", "+ 0 2 y"]
    );
    assert_eq!(untracked.text, b"@@ -0,0 +1,2 @@\nx\ny\n");
}
