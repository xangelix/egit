//! Interactive rebase against sandboxed repositories: ranges read (merges marked) and refused,
//! and plans that reorder, reword, squash, fix up, and drop commits, from a commit or the root,
//! stopping at conflicts and carrying on after them; a range with a merge rebased keeping it
//! (drops and rewords, melds and a dropped merge refused); and a rebase onto a branch keeping
//! merges.

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
    CancelToken, Integrated, ObjectId, Operation, Planned, RangeError, RebaseError, Repository,
    Step, Wait as _, rebase_interactive, rebase_keeping_merges, rebase_range,
};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// `git log` of `main`, one line per commit in `format`, newest first.
fn log(sandbox: &Sandbox, repo: &Path, format: &str) -> Vec<String> {
    let out = sandbox
        .run(repo, "log", &[&format!("--format={format}"), "main"])
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

fn message(sandbox: &Sandbox, repo: &Path, name: &str) -> String {
    let out = sandbox
        .run(repo, "log", &["-1", "--format=%B", name])
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

/// `One` to `Five`, each adding its own file (`f1` … `f5`), `Four` by another author on
/// another date.
fn five(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    for (index, subject) in ["One", "Two", "Three", "Four", "Five"].iter().enumerate() {
        let file = format!("f{}", index + 1);
        std::fs::write(repo.join(&file), format!("{subject}\n")).unwrap();
        sandbox.run(&repo, "add", &["--", &file]).unwrap();
        let mut args = vec!["--quiet", "--message", subject];
        if *subject == "Four" {
            args.extend([
                "--author",
                "Ada <ada@example.com>",
                "--date",
                "2001-02-03T04:05:06Z",
            ]);
        }
        sandbox.run(&repo, "commit", &args).unwrap();
    }
    repo
}

fn plan(steps: Vec<(ObjectId, Step)>) -> Vec<Planned> {
    steps
        .into_iter()
        .map(|(id, step)| Planned { id, step })
        .collect()
}

#[test]
fn a_range_is_read_from_a_commit_up_to_head() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let git = sandbox.git();
    let range = rebase_range(
        git,
        &repository,
        rev(&sandbox, &path, "main~2"),
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(range.base, Some(rev(&sandbox, &path, "main~3")));
    let subjects: Vec<_> = range
        .commits
        .iter()
        .map(egit_git::RangeCommit::subject)
        .collect();
    assert_eq!(subjects, ["Three", "Four", "Five"]);
    assert_eq!(range.commits[1].author, "Ada");
    let root = rev(&sandbox, &path, "main~4");
    let all = rebase_range(git, &repository, root, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!((all.base, all.commits.len()), (None, 5));

    // A commit elsewhere, and a range holding a merge.
    sandbox
        .run(&path, "switch", &["--quiet", "--create", "side", "main~1"])
        .unwrap();
    sandbox.commit_file(&path, "s", "side\n", "Side").unwrap();
    let side = rev(&sandbox, &path, "side");
    sandbox.run(&path, "switch", &["--quiet", "main"]).unwrap();
    let elsewhere = rebase_range(git, &repository, side, &CancelToken::new()).wait();
    assert!(
        matches!(elsewhere, Err(RangeError::NotInBranch)),
        "{elsewhere:?}"
    );
    sandbox
        .run(&path, "merge", &["--quiet", "--no-edit", "side"])
        .unwrap();
    // A merge is marked, its branch's commit in the range too.
    let merged = rebase_range(git, &repository, root, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(merged.merges(), 1);
    assert!(merged.commits.iter().any(|commit| commit.id == side));
    assert!(merged.commits.last().unwrap().merge);
}

#[test]
fn commits_are_reordered_reworded_squashed_fixed_up_and_dropped() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let [two, three, four, five] =
        ["main~3", "main~2", "main~1", "main"].map(|name| rev(&sandbox, &path, name));
    let base = rev(&sandbox, &path, "main~4");
    let ada = log(&sandbox, &path, "%an %at")[1].clone();
    assert!(ada.starts_with("Ada "), "{ada}");
    let reworded = "Two and three, reworded\n\nIt's 100% '%s', and # isn't a comment.";
    // `Four` first; `Two` reworded, `Three` squashed into it (the reword is the group's);
    // `Five` dropped.
    let steps = plan(vec![
        (four, Step::Pick),
        (two, Step::Reword(reworded.to_owned())),
        (three, Step::Squash),
        (five, Step::Drop),
    ]);
    let done = rebase_interactive(
        sandbox.git(),
        &repository,
        Some(base),
        &steps,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(done, Integrated::Done);
    assert_eq!(
        log(&sandbox, &path, "%s"),
        ["Two and three, reworded", "Four", "One"]
    );
    assert_eq!(message(&sandbox, &path, "main"), reworded);
    // Authors and dates kept.
    assert_eq!(log(&sandbox, &path, "%an %at")[1], ada);
    let files = sandbox.run(&path, "ls-files", &[]).unwrap();
    assert_eq!(files.stdout, b"f1\nf2\nf3\nf4\n");

    // A fixup keeps the message it melds into.
    let [four, two] = ["main~1", "main"].map(|name| rev(&sandbox, &path, name));
    let steps = plan(vec![(four, Step::Pick), (two, Step::Fixup)]);
    rebase_interactive(
        sandbox.git(),
        &repository,
        Some(base),
        &steps,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(log(&sandbox, &path, "%s"), ["Four", "One"]);
}

#[test]
fn a_squash_without_a_reword_joins_the_messages() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let base = rev(&sandbox, &path, "main~2");
    let steps = plan(vec![
        (rev(&sandbox, &path, "main~1"), Step::Pick),
        (rev(&sandbox, &path, "main"), Step::Squash),
    ]);
    rebase_interactive(
        sandbox.git(),
        &repository,
        Some(base),
        &steps,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(message(&sandbox, &path, "main"), "Four\n\nFive");
}

#[test]
fn conflicts_stop_the_rebase_and_rewords_after_them_still_apply() {
    let sandbox = Sandbox::new().unwrap();
    let path = sandbox.init("repo").unwrap();
    for (content, subject) in [("1\n", "One"), ("2\n", "Two"), ("3\n", "Three")] {
        sandbox.commit_file(&path, "f", content, subject).unwrap();
    }
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let base = rev(&sandbox, &path, "main~2");
    // `Three` before `Two`: each changes what the other did.
    let steps = plan(vec![
        (rev(&sandbox, &path, "main"), Step::Pick),
        (
            rev(&sandbox, &path, "main~1"),
            Step::Reword("Two, after three".to_owned()),
        ),
    ]);
    let stopped = rebase_interactive(
        sandbox.git(),
        &repository,
        Some(base),
        &steps,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(stopped, Integrated::Conflicts);
    assert_eq!(
        repository.operations(sandbox.git()).wait().unwrap(),
        [Operation::Rebase]
    );
    // Each conflict resolved as the commit being replayed has it, until the rebase ends.
    for resolved in ["3\n", "2\n"] {
        std::fs::write(path.join("f"), resolved).unwrap();
        sandbox.run(&path, "add", &["f"]).unwrap();
        sandbox
            .git()
            .write("rebase")
            .arg("--continue")
            .current_dir(&path)
            .output_unchecked()
            .wait()
            .unwrap();
    }
    assert_eq!(repository.operations(sandbox.git()).wait().unwrap(), []);
    assert_eq!(
        log(&sandbox, &path, "%s"),
        ["Two, after three", "Three", "One"]
    );
}

#[test]
fn the_root_is_reworded_and_a_dirty_work_tree_refused() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let range = rebase_range(
        sandbox.git(),
        &repository,
        rev(&sandbox, &path, "main~4"),
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    let mut steps: Vec<Planned> = range
        .commits
        .iter()
        .map(|commit| Planned {
            id: commit.id,
            step: Step::Pick,
        })
        .collect();
    steps[0].step = Step::Reword("The first".to_owned());

    std::fs::write(path.join("f2"), "changed\n").unwrap();
    let refused = rebase_interactive(
        sandbox.git(),
        &repository,
        range.base,
        &steps,
        &CancelToken::new(),
    )
    .wait();
    assert!(matches!(refused, Err(RebaseError::Git(_))), "{refused:?}");
    assert_eq!(repository.operations(sandbox.git()).wait().unwrap(), []);
    sandbox.run(&path, "checkout", &["--", "f2"]).unwrap();

    rebase_interactive(
        sandbox.git(),
        &repository,
        range.base,
        &steps,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(
        log(&sandbox, &path, "%s").last().map(String::as_str),
        Some("The first")
    );
    assert_eq!(log(&sandbox, &path, "%s").len(), 5);
}

/// `One`, then `side` (`Side one`, `Side two`) and `main` (`Two`) from it, `side` merged into
/// `main` (`Merge side`), then `Three`: a history with a merge.
fn merged(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "s1", "s1\n", "Side one")
        .unwrap();
    sandbox
        .commit_file(&repo, "s2", "s2\n", "Side two")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &["--quiet", "--no-ff", "--message", "Merge side", "side"],
        )
        .unwrap();
    sandbox.commit_file(&repo, "f", "three\n", "Three").unwrap();
    repo
}

#[test]
fn a_range_with_a_merge_keeps_it_as_its_plan_drops_and_rewords() {
    let sandbox = Sandbox::new().unwrap();
    let path = merged(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let git = sandbox.git();
    let cancel = CancelToken::new();
    let from = rev(&sandbox, &path, "main~2");
    let range = rebase_range(git, &repository, from, &cancel)
        .wait()
        .unwrap();
    assert_eq!(range.merges(), 1);
    let id_of = |subject: &str| {
        range
            .commits
            .iter()
            .find(|commit| commit.subject() == subject)
            .unwrap()
            .id
    };
    let plan = |steps: &[(&str, Step)]| -> Vec<Planned> {
        range
            .commits
            .iter()
            .map(|commit| Planned {
                id: commit.id,
                step: steps
                    .iter()
                    .find(|(subject, _)| *subject == commit.subject())
                    .map_or(Step::Pick, |(_, step)| step.clone()),
            })
            .collect()
    };

    // Nothing melded, and no merge dropped: refused, nothing changed.
    let head = rev(&sandbox, &path, "HEAD");
    for (steps, why) in [
        (vec![("Side two", Step::Squash)], "melded"),
        (vec![("Merge side", Step::Drop)], "a merge dropped"),
    ] {
        let refused =
            rebase_keeping_merges(git, &repository, range.base, &plan(&steps), &cancel).wait();
        assert!(refused.is_err(), "{why}");
        assert_eq!(rev(&sandbox, &path, "HEAD"), head, "{why}");
    }
    assert_eq!(
        repository.operations(sandbox.git()).wait().unwrap(),
        Vec::new()
    );

    // A side commit dropped, a commit and the merge reworded: the merge stays one.
    let steps = [
        ("Side one", Step::Drop),
        ("Two", Step::Reword("Two, again".to_owned())),
        ("Merge side", Step::Reword("Merge side, again".to_owned())),
    ];
    let done = rebase_keeping_merges(git, &repository, range.base, &plan(&steps), &cancel)
        .wait()
        .unwrap();
    assert_eq!(done, Integrated::Done);
    let _ = id_of("Side one");
    let subjects = |args: &[&str]| -> Vec<String> {
        let out = sandbox.run(&path, "log", args).unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(ToOwned::to_owned)
            .collect()
    };
    assert_eq!(
        subjects(&["--format=%s", "--first-parent", "main"]),
        ["Three", "Merge side, again", "Two, again", "One"]
    );
    assert_eq!(
        subjects(&["--format=%s", "main~1^2"]),
        ["Side two", "One"],
        "the merge's branch, less its dropped commit"
    );
    let merges = sandbox
        .run(&path, "rev-list", &["--merges", "--count", "HEAD"])
        .unwrap();
    assert_eq!(merges.stdout.trim_ascii(), b"1");
    assert!(!path.join("s1").exists(), "its commit dropped");
    assert!(path.join("s2").exists());
}

#[test]
fn a_rebase_onto_a_branch_keeps_merges_when_asked() {
    let sandbox = Sandbox::new().unwrap();
    let path = merged(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).wait().unwrap();
    let cancel = CancelToken::new();
    // `upstream` moved on from `One`.
    sandbox
        .run(&path, "branch", &["upstream", "main~2^1"])
        .unwrap();
    sandbox
        .run(&path, "switch", &["--quiet", "upstream"])
        .unwrap();
    sandbox.commit_file(&path, "u", "u\n", "Upstream").unwrap();
    sandbox.run(&path, "switch", &["--quiet", "main"]).unwrap();
    let merges = |sandbox: &Sandbox, branch: &str| {
        let range = format!("upstream..{branch}");
        let out = sandbox
            .run(&path, "rev-list", &["--merges", "--count", &range])
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    // A copy of `main`, rebased after it without being asked to keep merges.
    sandbox.run(&path, "branch", &["copy", "main"]).unwrap();
    let upstream = bstr::BStr::new("upstream");
    let done = egit_git::rebase(sandbox.git(), &repository, upstream, true, &cancel)
        .wait()
        .unwrap();
    assert_eq!(done, Integrated::Done);
    assert_eq!(merges(&sandbox, "main"), "1", "kept");
    assert!(path.join("u").exists());
    sandbox.run(&path, "switch", &["--quiet", "copy"]).unwrap();
    let done = egit_git::rebase(sandbox.git(), &repository, upstream, false, &cancel)
        .wait()
        .unwrap();
    assert_eq!(done, Integrated::Done);
    assert_eq!(merges(&sandbox, "copy"), "0", "flattened");
}
