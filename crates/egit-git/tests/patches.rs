//! Patches against sandboxed repositories: a series saved from commits (from one, from the
//! first), the uncommitted changes as a patch, what a patch holds and whether it applies, and
//! applying one: as commits (cleanly, signed off, stopping at a conflict with how far it got,
//! skipped, aborted; stopping where a patch doesn't apply at all) and as changes (staged or
//! not, with conflicts, refused); and skipping a stopped cherry-pick.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egit_git::testing::Sandbox;
use egit_git::{
    Applies, ApplyAs, CancelToken, Changes, Integrated, ObjectId, Operation, PatchChange,
    PatchKind, Repository, Wait as _, abort_operation, apply_patch, changes_patch, cherry_pick,
    current_patch, format_patches, skip_operation, summarize_patch,
};

fn open(sandbox: &Sandbox, path: &Path) -> Repository {
    Repository::discover(sandbox.git(), path).wait().unwrap()
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

fn subjects(sandbox: &Sandbox, repo: &Path, range: &str) -> Vec<String> {
    let out = sandbox.run(repo, "log", &["--format=%s", range]).unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `main` (`f`: a b c), and `topic` from it changing b, then c, then adding `g` (by Ada).
fn series(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nc\n", "Base")
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "topic"])
        .unwrap();
    for (file, contents, subject) in [
        ("f", "a\nB\nc\n", "Change b"),
        ("f", "a\nB\nC\n", "Change c"),
        ("g", "new\n", "Add g"),
    ] {
        std::fs::write(repo.join(file), contents).unwrap();
        sandbox.run(&repo, "add", &[file]).unwrap();
        sandbox
            .run(
                &repo,
                "commit",
                &[
                    "--quiet",
                    "--author",
                    "Ada Lovelace <ada@example.org>",
                    "--message",
                    subject,
                ],
            )
            .unwrap();
    }
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    repo
}

/// `topic`'s commits as patches in a folder, and all of them as one mailbox.
fn saved(sandbox: &Sandbox, repo: &Path) -> (Vec<PathBuf>, Arc<[u8]>) {
    let repository = open(sandbox, repo);
    sandbox.run(repo, "switch", &["--quiet", "topic"]).unwrap();
    let folder = sandbox.path("patches");
    let files = format_patches(
        sandbox.git(),
        &repository,
        rev(sandbox, repo, "topic~2"),
        &folder,
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    sandbox.run(repo, "switch", &["--quiet", "main"]).unwrap();
    let mut mailbox = Vec::new();
    for file in &files {
        mailbox.extend(std::fs::read(file).unwrap());
    }
    (files, Arc::from(mailbox))
}

#[test]
fn commits_are_saved_as_a_series_from_one_or_from_the_first() {
    let sandbox = Sandbox::new().unwrap();
    let repo = series(&sandbox);
    let (files, _) = saved(&sandbox, &repo);
    let names: Vec<String> = files
        .iter()
        .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "0001-Change-b.patch",
            "0002-Change-c.patch",
            "0003-Add-g.patch"
        ]
    );
    let repository = open(&sandbox, &repo);
    let all = format_patches(
        sandbox.git(),
        &repository,
        rev(&sandbox, &repo, "main"),
        &sandbox.path("all"),
        &CancelToken::new(),
    )
    .wait()
    .unwrap();
    assert_eq!(all.len(), 1, "main has its first commit alone");
    // A commit not in HEAD's history is refused.
    assert!(
        format_patches(
            sandbox.git(),
            &repository,
            rev(&sandbox, &repo, "topic"),
            &sandbox.path("none"),
            &CancelToken::new(),
        )
        .wait()
        .is_err()
    );
}

#[test]
fn a_patch_says_what_it_holds_and_whether_it_applies() {
    let sandbox = Sandbox::new().unwrap();
    let repo = series(&sandbox);
    let (files, mailbox) = saved(&sandbox, &repo);
    let repository = open(&sandbox, &repo);
    let summary = |patch: &Arc<[u8]>| {
        summarize_patch(sandbox.git(), &repository, patch, &CancelToken::new())
            .wait()
            .unwrap()
    };
    let whole = summary(&mailbox);
    assert_eq!(whole.kind, PatchKind::Mailbox);
    assert_eq!(whole.commit_count, 3);
    let subjects: Vec<&str> = whole
        .commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Change b", "Change c", "Add g"]);
    assert_eq!(whole.commits[0].author, "Ada Lovelace <ada@example.org>");
    assert_eq!(whole.files.len(), 2);
    assert_eq!(whole.files[0].lines, Some((2, 2)), "f, over two commits");
    assert_eq!(whole.files[1].change, PatchChange::Added);
    assert_eq!(
        whole.applies,
        Applies::Unchecked,
        "a series applies in turn"
    );

    let first: Arc<[u8]> = Arc::from(std::fs::read(&files[0]).unwrap());
    assert_eq!(summary(&first).applies, Applies::Cleanly);
    // Main changes c: the first patch's context is gone, but merging conflicts.
    sandbox
        .commit_file(&repo, "f", "a\nb\nX\n", "Change c differently")
        .unwrap();
    assert_eq!(
        summary(&first).applies,
        Applies::Merging { conflicts: true }
    );
    // Without the blobs it was made from, it can't be merged either.
    let text = String::from_utf8(first.to_vec()).unwrap();
    let blobless: String = text
        .lines()
        .map(|line| {
            if line.starts_with("index ") {
                "index 1234567..89abcde 100644"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(matches!(
        summary(&Arc::from(format!("{blobless}\n").into_bytes())).applies,
        Applies::Not(_)
    ));
}

#[test]
fn a_series_applies_as_commits_stopping_where_one_conflicts() {
    let sandbox = Sandbox::new().unwrap();
    let repo = series(&sandbox);
    let (_, mailbox) = saved(&sandbox, &repo);
    let repository = open(&sandbox, &repo);
    let (git, cancel) = (sandbox.git(), CancelToken::new());

    // Cleanly, signed off, on a branch from main: the commits as they were, author and all.
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "clean", "main"])
        .unwrap();
    assert_eq!(
        apply_patch(
            git,
            &repository,
            &mailbox,
            ApplyAs::Commits { sign_off: true },
            &cancel
        )
        .wait()
        .unwrap(),
        Integrated::Done
    );
    assert_eq!(
        subjects(&sandbox, &repo, "main..clean"),
        ["Add g", "Change c", "Change b"]
    );
    let message = sandbox
        .run(&repo, "log", &["-1", "--format=%an%n%b"])
        .unwrap();
    let message = String::from_utf8(message.stdout).unwrap();
    assert!(message.starts_with("Ada Lovelace"), "{message}");
    assert!(message.contains("Signed-off-by: egit tests"), "{message}");

    // On a main that changed c too: the first patch conflicts.
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nX\n", "Change c differently")
        .unwrap();
    assert_eq!(
        apply_patch(
            git,
            &repository,
            &mailbox,
            ApplyAs::Commits { sign_off: false },
            &cancel
        )
        .wait()
        .unwrap(),
        Integrated::Conflicts
    );
    assert_eq!(
        repository.operations(sandbox.git()).wait().unwrap(),
        [Operation::ApplyMailbox]
    );
    let progress = repository
        .mailbox_progress(sandbox.git())
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!((progress.next, progress.last), (1, 3));
    assert_eq!(progress.subject, "Change b");
    let stopped_at = String::from_utf8(current_patch(git, &repository).wait().unwrap()).unwrap();
    assert!(stopped_at.contains("+B"), "{stopped_at}");

    // Skipped: the next conflicts too; aborted, all is as it was.
    assert_eq!(
        skip_operation(git, &repository, Operation::ApplyMailbox, &cancel)
            .wait()
            .unwrap(),
        Integrated::Conflicts
    );
    let progress = repository
        .mailbox_progress(sandbox.git())
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!((progress.next, progress.subject.as_str()), (2, "Change c"));
    abort_operation(git, &repository, Operation::ApplyMailbox, &cancel)
        .wait()
        .unwrap();
    assert_eq!(
        repository.operations(sandbox.git()).wait().unwrap(),
        Vec::<Operation>::new()
    );
    assert_eq!(
        repository.mailbox_progress(sandbox.git()).wait().unwrap(),
        None
    );
    assert_eq!(
        subjects(&sandbox, &repo, "main~1..main"),
        ["Change c differently"]
    );
}

#[test]
fn a_patch_that_doesnt_apply_at_all_stops_with_nothing_to_resolve() {
    let sandbox = Sandbox::new().unwrap();
    let repo = series(&sandbox);
    let (files, _) = saved(&sandbox, &repo);
    let repository = open(&sandbox, &repo);
    sandbox
        .commit_file(&repo, "f", "a\nb\nX\n", "Change c differently")
        .unwrap();
    let text = std::fs::read_to_string(&files[0]).unwrap();
    let blobless: String = text
        .lines()
        .map(|line| {
            if line.starts_with("index ") {
                "index 1234567..89abcde 100644"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let patch: Arc<[u8]> = Arc::from(format!("{blobless}\n").into_bytes());
    let cancel = CancelToken::new();
    assert_eq!(
        apply_patch(
            sandbox.git(),
            &repository,
            &patch,
            ApplyAs::Commits { sign_off: false },
            &cancel
        )
        .wait()
        .unwrap(),
        Integrated::Stopped
    );
    abort_operation(sandbox.git(), &repository, Operation::ApplyMailbox, &cancel)
        .wait()
        .unwrap();
    // As changes, it is refused outright.
    assert!(
        apply_patch(
            sandbox.git(),
            &repository,
            &patch,
            ApplyAs::Changes { staged: true },
            &cancel
        )
        .wait()
        .is_err()
    );
}

#[test]
fn changes_become_a_patch_and_apply_as_changes() {
    let sandbox = Sandbox::new().unwrap();
    let repo = series(&sandbox);
    let repository = open(&sandbox, &repo);
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    std::fs::write(repo.join("f"), "a\nb\nc\nd\n").unwrap();
    std::fs::write(repo.join("h"), [0_u8, 1, 2, 255]).unwrap();
    sandbox.run(&repo, "add", &["h"]).unwrap();
    let staged: Arc<[u8]> = Arc::from(
        changes_patch(git, &repository, Changes::Staged, &cancel)
            .wait()
            .unwrap(),
    );
    let all: Arc<[u8]> = Arc::from(
        changes_patch(git, &repository, Changes::All, &cancel)
            .wait()
            .unwrap(),
    );
    let (staged_text, all_text) = (
        String::from_utf8_lossy(&staged).into_owned(),
        String::from_utf8_lossy(&all).into_owned(),
    );
    assert!(staged_text.contains("GIT binary patch") && !staged_text.contains("+d"));
    assert!(all_text.contains("GIT binary patch") && all_text.contains("+d"));

    // Back to how things were, then the patch as changes: in the work tree only, then staged.
    sandbox.run(&repo, "reset", &["--quiet", "--hard"]).unwrap();
    // `reset --hard` removes a file only staged; be sure.
    let _ = std::fs::remove_file(repo.join("h"));
    assert_eq!(
        apply_patch(
            git,
            &repository,
            &all,
            ApplyAs::Changes { staged: false },
            &cancel
        )
        .wait()
        .unwrap(),
        Integrated::Done
    );
    assert_eq!(std::fs::read(repo.join("h")).unwrap(), [0, 1, 2, 255]);
    let unstaged = sandbox
        .run(&repo, "diff", &["--cached", "--name-only"])
        .unwrap();
    assert!(unstaged.stdout.is_empty(), "nothing staged");
    sandbox.run(&repo, "reset", &["--quiet", "--hard"]).unwrap();
    // `reset --hard` removes a file only staged; be sure.
    let _ = std::fs::remove_file(repo.join("h"));
    assert_eq!(
        apply_patch(
            git,
            &repository,
            &all,
            ApplyAs::Changes { staged: true },
            &cancel
        )
        .wait()
        .unwrap(),
        Integrated::Done
    );
    let staged_names = sandbox
        .run(&repo, "diff", &["--cached", "--name-only"])
        .unwrap();
    assert_eq!(String::from_utf8(staged_names.stdout).unwrap(), "f\nh\n");

    // A conflict, merged in three ways: left in the index to resolve.
    sandbox.run(&repo, "reset", &["--quiet", "--hard"]).unwrap();
    std::fs::write(repo.join("f"), "a\nb\nc\nD\n").unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--all", "--message", "Add D"])
        .unwrap();
    assert_eq!(
        apply_patch(
            git,
            &repository,
            &all,
            ApplyAs::Changes { staged: true },
            &cancel
        )
        .wait()
        .unwrap(),
        Integrated::Conflicts
    );
}

#[test]
fn a_stopped_cherry_pick_is_skipped() {
    let sandbox = Sandbox::new().unwrap();
    let repo = series(&sandbox);
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    sandbox
        .commit_file(&repo, "f", "a\nZ\nc\n", "Change b differently")
        .unwrap();
    let picked = cherry_pick(
        sandbox.git(),
        &repository,
        rev(&sandbox, &repo, "topic~2"),
        &cancel,
    )
    .wait()
    .unwrap();
    assert_eq!(picked, Integrated::Conflicts);
    assert_eq!(
        skip_operation(sandbox.git(), &repository, Operation::CherryPick, &cancel)
            .wait()
            .unwrap(),
        Integrated::Done
    );
    assert_eq!(
        repository.operations(sandbox.git()).wait().unwrap(),
        Vec::<Operation>::new()
    );
    assert_eq!(
        subjects(&sandbox, &repo, "main~1..main"),
        ["Change b differently"]
    );
    // A merge has nothing to skip.
    assert!(
        skip_operation(sandbox.git(), &repository, Operation::Merge, &cancel)
            .wait()
            .is_err()
    );
}
