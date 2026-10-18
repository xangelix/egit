//! What bringing commits in would do, foreseen, then held to what git does when it is done:
//! merges (clean, conflicting, up to date, a fast-forward), cherry-picks and reverts (clean,
//! conflicting, changing nothing, a root commit), and rebases (clean, stopping at the commit
//! git stops at, too long to replay); nothing moves meanwhile.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use bstr::BString;
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, Foreseen, MAX_REPLAYED, ObjectId, Repository, foresee_merge, foresee_pick,
    foresee_rebase,
};

fn id(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox
        .run(
            repo,
            "rev-parse",
            &["--verify", &format!("{name}^{{commit}}")],
        )
        .unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// Runs git, whatever it ends with; whether it succeeded.
fn attempt(sandbox: &Sandbox, repo: &Path, subcommand: &'static str, args: &[&str]) -> bool {
    sandbox
        .git()
        .write(subcommand)
        .args(args)
        .current_dir(repo)
        .output_unchecked()
        .unwrap()
        .status
        .success()
}

/// The files git left in conflict.
fn conflicted(sandbox: &Sandbox, repo: &Path) -> Vec<BString> {
    let out = sandbox
        .run(repo, "diff", &["--name-only", "--diff-filter=U", "-z"])
        .unwrap();
    out.stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(BString::from)
        .collect()
}

/// `main` and `side` from a common commit: `side` changes `f`'s first line, then `g`; `main`
/// changes `f`'s last line, then `g` otherwise.
fn diverged(sandbox: &Sandbox) -> (std::path::PathBuf, Repository) {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nc\n", "Base")
        .unwrap();
    sandbox.commit_file(&repo, "g", "x\n", "Add g").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "f", "A\nb\nc\n", "Side f")
        .unwrap();
    sandbox.commit_file(&repo, "g", "y\n", "Side g").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nC\n", "Main f")
        .unwrap();
    sandbox.commit_file(&repo, "g", "z\n", "Main g").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    (repo, repository)
}

#[test]
fn merges_are_foreseen_as_git_then_merges_them() {
    let sandbox = Sandbox::new().unwrap();
    let (repo, repository) = diverged(&sandbox);
    let cancel = CancelToken::new();
    let at = |name| id(&sandbox, &repo, name);
    let foresee = |head, other| {
        foresee_merge(sandbox.git(), &repository, at(head), at(other), &cancel).unwrap()
    };
    let head = at("HEAD");

    assert_eq!(foresee("main", "main~2"), Foreseen::UpToDate);
    assert_eq!(foresee("main~2", "main"), Foreseen::FastForward);
    assert_eq!(foresee("main~1", "side~1"), Foreseen::Clean);
    let conflicts = Foreseen::Conflicts {
        files: vec!["g".into()],
        at: None,
    };
    assert_eq!(foresee("main", "side"), conflicts);
    // Nothing moved while foreseeing; then git's own merge stops where foreseen.
    assert_eq!(at("HEAD"), head);
    assert!(!attempt(&sandbox, &repo, "merge", &["--no-edit", "side"]));
    assert_eq!(conflicted(&sandbox, &repo), [BString::from("g")]);
    sandbox.run(&repo, "merge", &["--abort"]).unwrap();
}

#[test]
fn picks_and_reverts_are_foreseen() {
    let sandbox = Sandbox::new().unwrap();
    let (repo, repository) = diverged(&sandbox);
    sandbox
        .commit_file(&repo, "g", "w\n", "Main g again")
        .unwrap();
    let cancel = CancelToken::new();
    let at = |name| id(&sandbox, &repo, name);
    let foresee = |commit, revert| {
        foresee_pick(
            sandbox.git(),
            &repository,
            at("HEAD"),
            (at(commit), revert),
            &cancel,
        )
        .unwrap()
    };

    assert_eq!(foresee("side~1", false), Some(Foreseen::Clean));
    assert_eq!(
        foresee("side", false),
        Some(Foreseen::Conflicts {
            files: vec!["g".into()],
            at: None
        })
    );
    // Its change is in already.
    assert_eq!(foresee("main~2", false), Some(Foreseen::Empty));
    // The latest commit always reverts; one whose lines changed again since doesn't.
    assert_eq!(foresee("HEAD", true), Some(Foreseen::Clean));
    assert_eq!(foresee("HEAD~2", true), Some(Foreseen::Clean));
    let reverting = foresee("HEAD~1", true);
    assert!(reverting.as_ref().is_some_and(Foreseen::conflicts));
    // A root commit has no changes against a parent to work out.
    assert_eq!(foresee("main~4", true), None);

    // git agrees.
    assert!(attempt(&sandbox, &repo, "revert", &["--no-edit", "HEAD~2"]));
    assert!(!attempt(
        &sandbox,
        &repo,
        "revert",
        &["--no-edit", "HEAD~2"]
    ));
    assert_eq!(conflicted(&sandbox, &repo), [BString::from("g")]);
    sandbox.run(&repo, "revert", &["--abort"]).unwrap();
}

#[test]
fn rebases_are_foreseen_to_the_commit_they_stop_at() {
    let sandbox = Sandbox::new().unwrap();
    let (repo, repository) = diverged(&sandbox);
    let cancel = CancelToken::new();
    let at = |name| id(&sandbox, &repo, name);
    let foresee = |head, onto| {
        foresee_rebase(sandbox.git(), &repository, at(head), at(onto), &cancel).unwrap()
    };

    assert_eq!(foresee("main", "main~1"), Foreseen::UpToDate);
    assert_eq!(foresee("main~1", "main"), Foreseen::FastForward);
    assert_eq!(foresee("side~1", "main"), Foreseen::Clean);
    let side_g = at("side");
    assert_eq!(
        foresee("side", "main"),
        Foreseen::Conflicts {
            files: vec!["g".into()],
            at: Some(side_g)
        }
    );

    // git stops at that commit.
    sandbox.run(&repo, "switch", &["--quiet", "side"]).unwrap();
    assert!(!attempt(&sandbox, &repo, "rebase", &["main"]));
    assert_eq!(at("REBASE_HEAD"), side_g);
    sandbox.run(&repo, "rebase", &["--abort"]).unwrap();

    // Past the most replayed, not worked out.
    for index in 0..MAX_REPLAYED {
        sandbox
            .run(
                &repo,
                "commit",
                &["--quiet", "--allow-empty", "-m", &index.to_string()],
            )
            .unwrap();
    }
    assert_eq!(foresee("side", "main"), Foreseen::TooLong(MAX_REPLAYED + 2));
}
