//! Interactive rebase against sandboxed repositories: ranges read and refused, and plans that
//! reorder, reword, squash, fix up, and drop commits, from a commit or the root, stopping at
//! conflicts and carrying on after them.

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
    Step, rebase_interactive, rebase_range,
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
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
    let git = sandbox.git();
    let range = rebase_range(
        git,
        &repository,
        rev(&sandbox, &path, "main~2"),
        &CancelToken::new(),
    )
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
    let all = rebase_range(git, &repository, root, &CancelToken::new()).unwrap();
    assert_eq!((all.base, all.commits.len()), (None, 5));

    // A commit elsewhere, and a range holding a merge.
    sandbox
        .run(&path, "switch", &["--quiet", "--create", "side", "main~1"])
        .unwrap();
    sandbox.commit_file(&path, "s", "side\n", "Side").unwrap();
    let side = rev(&sandbox, &path, "side");
    sandbox.run(&path, "switch", &["--quiet", "main"]).unwrap();
    let elsewhere = rebase_range(git, &repository, side, &CancelToken::new());
    assert!(
        matches!(elsewhere, Err(RangeError::NotInBranch)),
        "{elsewhere:?}"
    );
    sandbox
        .run(&path, "merge", &["--quiet", "--no-edit", "side"])
        .unwrap();
    let merged = rebase_range(git, &repository, root, &CancelToken::new());
    assert!(matches!(merged, Err(RangeError::Merges(1))), "{merged:?}");
}

#[test]
fn commits_are_reordered_reworded_squashed_fixed_up_and_dropped() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
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
    .unwrap();
    assert_eq!(log(&sandbox, &path, "%s"), ["Four", "One"]);
}

#[test]
fn a_squash_without_a_reword_joins_the_messages() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
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
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
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
    .unwrap();
    assert_eq!(stopped, Integrated::Conflicts);
    assert_eq!(repository.operations().unwrap(), [Operation::Rebase]);
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
            .unwrap();
    }
    assert_eq!(repository.operations().unwrap(), []);
    assert_eq!(
        log(&sandbox, &path, "%s"),
        ["Two, after three", "Three", "One"]
    );
}

#[test]
fn the_root_is_reworded_and_a_dirty_work_tree_refused() {
    let sandbox = Sandbox::new().unwrap();
    let path = five(&sandbox);
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
    let range = rebase_range(
        sandbox.git(),
        &repository,
        rev(&sandbox, &path, "main~4"),
        &CancelToken::new(),
    )
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
    );
    assert!(matches!(refused, Err(RebaseError::Git(_))), "{refused:?}");
    assert_eq!(repository.operations().unwrap(), []);
    sandbox.run(&path, "checkout", &["--", "f2"]).unwrap();

    rebase_interactive(
        sandbox.git(),
        &repository,
        range.base,
        &steps,
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(
        log(&sandbox, &path, "%s").last().map(String::as_str),
        Some("The first")
    );
    assert_eq!(log(&sandbox, &path, "%s").len(), 5);
}
