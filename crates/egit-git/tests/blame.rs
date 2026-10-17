//! Blame and a file's history against sandboxed repositories: lines attributed across a rename
//! and a move within the file, uncommitted lines, and the commits that changed a file
//! followed through its rename.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::fmt::Write as _;
use std::path::Path;

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    BlameCommit, BlameRun, CancelToken, FileCommit, ObjectId, Repository, blame, file_log,
};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// Each line's commit summary (`None` for uncommitted lines), in the file's order.
fn summaries(sandbox: &Sandbox, repo: &Path, at: Option<ObjectId>, path: &str) -> Vec<String> {
    let repository = Repository::discover(sandbox.git(), repo).unwrap();
    let mut commits: Vec<BlameCommit> = Vec::new();
    let mut runs: Vec<BlameRun> = Vec::new();
    blame(
        sandbox.git(),
        &repository,
        at,
        BStr::new(path),
        &CancelToken::new(),
        |found, run| {
            commits = found.to_vec();
            runs.push(run);
        },
    )
    .unwrap();
    let lines = runs
        .iter()
        .map(|run| run.line + run.count)
        .max()
        .unwrap_or(0);
    let mut shown = vec![String::new(); usize::try_from(lines).unwrap()];
    for run in runs {
        let commit = &commits[usize::try_from(run.commit).unwrap()];
        let summary = if commit.id.is_some() {
            commit.summary.clone()
        } else {
            "(uncommitted)".to_owned()
        };
        for line in run.line..run.line + run.count {
            shown[usize::try_from(line).unwrap()].clone_from(&summary);
        }
    }
    shown
}

/// `notes` written three times, then renamed to `doc`, its first block moved to its end.
fn history(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("repo").unwrap();
    let block = |name: &str| {
        (1..=4).fold(String::new(), |mut block, n| {
            let _ = writeln!(block, "{name} line {n}");
            block
        })
    };
    sandbox
        .commit_file(
            &repo,
            "notes",
            &format!("{}{}", block("alpha"), block("beta")),
            "Write",
        )
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "notes",
            &format!("{}{}changed\n", block("alpha"), block("beta")),
            "Add",
        )
        .unwrap();
    sandbox.run(&repo, "mv", &["notes", "doc"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Rename"])
        .unwrap();
    // The alpha block moved below the beta one.
    sandbox
        .commit_file(
            &repo,
            "doc",
            &format!("{}changed\n{}", block("beta"), block("alpha")),
            "Move",
        )
        .unwrap();
    repo
}

#[test]
fn lines_are_blamed_across_a_rename_and_a_move() {
    let sandbox = Sandbox::new().unwrap();
    let repo = history(&sandbox);
    let head = rev(&sandbox, &repo, "HEAD");
    let shown = summaries(&sandbox, &repo, Some(head), "doc");
    assert_eq!(shown.len(), 9);
    // The beta block and the moved alpha block still come from the first commit; the line
    // added before the rename from its commit.
    assert_eq!(&shown[..4], ["Write"; 4]);
    assert_eq!(shown[4], "Add");
    assert_eq!(&shown[5..], ["Write"; 4]);
}

#[test]
fn the_work_tree_s_uncommitted_lines_are_said_so() {
    let sandbox = Sandbox::new().unwrap();
    let repo = history(&sandbox);
    let mut doc = std::fs::read_to_string(repo.join("doc")).unwrap();
    doc.push_str("not yet\n");
    std::fs::write(repo.join("doc"), doc).unwrap();
    let shown = summaries(&sandbox, &repo, None, "doc");
    assert_eq!(shown.last().map(String::as_str), Some("(uncommitted)"));
    assert_eq!(shown[4], "Add");
}

#[test]
fn a_file_s_history_follows_its_rename() {
    let sandbox = Sandbox::new().unwrap();
    let repo = history(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let mut commits: Vec<FileCommit> = Vec::new();
    file_log(
        sandbox.git(),
        &repository,
        BStr::new("doc"),
        &CancelToken::new(),
        |commit| commits.push(commit),
    )
    .unwrap();
    let seen: Vec<(&str, u8, String)> = commits
        .iter()
        .map(|commit| {
            (
                commit.subject.as_str(),
                commit.status,
                commit.path.to_string(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("Move", b'M', "doc".to_owned()),
            ("Rename", b'R', "doc".to_owned()),
            ("Add", b'M', "notes".to_owned()),
            ("Write", b'A', "notes".to_owned()),
        ]
    );
    assert_eq!(
        commits[1].from.as_ref().map(ToString::to_string),
        Some("notes".to_owned())
    );
    assert_eq!(commits[0].parent, Some(rev(&sandbox, &repo, "HEAD~1")));
    assert_eq!(commits[3].parent, None);
}
