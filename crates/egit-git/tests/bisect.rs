//! Bisecting against sandboxed repositories: a bisection started by marking a commit, driven
//! to its first bad commit by marking the commits git checks out; skipped commits leaving a
//! choice; a bisection in its own terms; and none read when none is under way.

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
    BisectLeft, BisectMark, BisectTerms, CancelToken, ObjectId, Operation, Repository,
    abort_operation, bisect_mark, bisection,
};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// Ten commits writing `1` … `10` into `f`: from the seventh on, `f` is "bad" (7 or more).
fn ten(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    for number in 1..=10 {
        sandbox
            .commit_file(&repo, "f", &format!("{number}\n"), &format!("c{number}"))
            .unwrap();
    }
    repo
}

/// What the commit checked out says.
fn checked_out(repo: &Path) -> u32 {
    std::fs::read_to_string(repo.join("f"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn a_bisection_finds_the_first_bad_commit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = ten(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    assert_eq!(bisection(git, &repository).unwrap(), None, "none yet");

    // Started by marking the newest bad, then the oldest good.
    bisect_mark(
        git,
        &repository,
        BisectMark::Bad,
        rev(&sandbox, &repo, "HEAD"),
        &cancel,
    )
    .unwrap();
    let started = bisection(git, &repository).unwrap().unwrap();
    assert_eq!(started.terms, BisectTerms::default());
    assert_eq!(started.left, BisectLeft::NeedsGood);
    assert!(
        repository
            .operations(sandbox.git())
            .unwrap()
            .contains(&Operation::Bisect)
    );
    let first = rev(&sandbox, &repo, "HEAD~9");
    bisect_mark(git, &repository, BisectMark::Good, first, &cancel).unwrap();
    let testing = bisection(git, &repository).unwrap().unwrap();
    assert_eq!(testing.left, BisectLeft::Testing { candidates: 9 });
    assert_eq!(testing.left.steps(), 3);
    assert_eq!(testing.good, [first]);

    // Each commit git checks out marked as it is, until the first bad one is found.
    let seventh = rev(&sandbox, &repo, "main~3");
    for _ in 0..10 {
        let state = bisection(git, &repository).unwrap().unwrap();
        if let BisectLeft::Found(found) = state.left {
            assert_eq!(found, seventh);
            break;
        }
        let mark = if checked_out(&repo) >= 7 {
            BisectMark::Bad
        } else {
            BisectMark::Good
        };
        let head = rev(&sandbox, &repo, "HEAD");
        bisect_mark(git, &repository, mark, head, &cancel).unwrap();
    }
    assert_eq!(
        bisection(git, &repository).unwrap().unwrap().left,
        BisectLeft::Found(seventh)
    );

    // Stopped: back on the branch.
    abort_operation(git, &repository, Operation::Bisect, &cancel).unwrap();
    assert_eq!(bisection(git, &repository).unwrap(), None);
    assert_eq!(rev(&sandbox, &repo, "HEAD"), rev(&sandbox, &repo, "main"));
}

#[test]
fn skipped_commits_leave_a_choice() {
    let sandbox = Sandbox::new().unwrap();
    let repo = ten(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    // Between the fifth (good) and the seventh (bad), the sixth can't be tested.
    let [fifth, sixth, seventh] =
        ["main~5", "main~4", "main~3"].map(|name| rev(&sandbox, &repo, name));
    bisect_mark(git, &repository, BisectMark::Bad, seventh, &cancel).unwrap();
    bisect_mark(git, &repository, BisectMark::Good, fifth, &cancel).unwrap();
    bisect_mark(git, &repository, BisectMark::Skip, sixth, &cancel).unwrap();
    let state = bisection(git, &repository).unwrap().unwrap();
    assert_eq!(state.skipped, [sixth]);
    let BisectLeft::Among(mut among) = state.left else {
        panic!("{:?}", state.left)
    };
    among.sort_unstable();
    let mut expected = vec![sixth, seventh];
    expected.sort_unstable();
    assert_eq!(among, expected);
}

#[test]
fn a_bisection_in_its_own_terms_is_marked_in_them() {
    let sandbox = Sandbox::new().unwrap();
    let repo = ten(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    // Looking for a fix: `fixed` is the side with the change.
    sandbox
        .run(
            &repo,
            "bisect",
            &["start", "--term-new=fixed", "--term-old=broken"],
        )
        .unwrap();
    bisect_mark(
        git,
        &repository,
        BisectMark::Bad,
        rev(&sandbox, &repo, "HEAD"),
        &cancel,
    )
    .unwrap();
    let state = bisection(git, &repository).unwrap().unwrap();
    assert_eq!(
        state.terms,
        BisectTerms {
            bad: "fixed".to_owned(),
            good: "broken".to_owned()
        }
    );
    assert_eq!(state.bad, Some(rev(&sandbox, &repo, "main")));
    assert_eq!(state.left, BisectLeft::NeedsGood);
}
