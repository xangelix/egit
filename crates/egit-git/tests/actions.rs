//! The graph's actions against sandboxed repositories: branches switched to, created, renamed,
//! and deleted (a remote's through a bare repository); commits merged, rebased onto,
//! cherry-picked, reverted, and reset to; tags; and stashes.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, CloneOptions, DeleteBranchError, FastForward, Head, Integrated, ObjectId,
    Operation, Progress, PushFlag, Repository, ResetMode, StashError, SwitchError, SwitchTo,
    cherry_pick, clone, create_branch, create_tag, delete_branch, delete_remote_branch, delete_tag,
    merge, push_tag, rebase, rename_branch, reset, revert, stash_apply, stash_drop, stash_push,
    stashes, switch,
};

fn open(sandbox: &Sandbox, path: &Path) -> Repository {
    Repository::discover(sandbox.git(), path).unwrap()
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

fn subject(sandbox: &Sandbox, repo: &Path, name: &str) -> String {
    let out = sandbox
        .run(repo, "log", &["-1", "--format=%s", name])
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// The branch `HEAD` is on (`None` when detached).
fn branch(sandbox: &Sandbox, repository: &Repository) -> Option<String> {
    match repository.head(sandbox.git()).unwrap() {
        Head::Branch(branch) => Some(branch.name.to_string()),
        Head::Detached(_) => None,
    }
}

fn cancel() -> CancelToken {
    CancelToken::new()
}

/// `main` with `f` changed twice, and `other` from the first commit adding `g`.
fn two_branches(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "other"])
        .unwrap();
    sandbox.commit_file(&repo, "g", "other\n", "Other").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    repo
}

#[test]
fn branches_are_switched_to_created_renamed_and_deleted() {
    let sandbox = Sandbox::new().unwrap();
    let path = two_branches(&sandbox);
    let repository = open(&sandbox, &path);
    let git = sandbox.git();

    switch(
        git,
        &repository,
        &SwitchTo::Branch("other".into()),
        &cancel(),
    )
    .unwrap();
    assert_eq!(branch(&sandbox, &repository).as_deref(), Some("other"));
    // Changes git would overwrite stop it, and are named.
    std::fs::write(path.join("f"), "local\n").unwrap();
    let blocked = switch(
        git,
        &repository,
        &SwitchTo::Branch("main".into()),
        &cancel(),
    );
    let Err(SwitchError::WouldOverwrite { paths, untracked }) = blocked else {
        panic!("{blocked:?}");
    };
    assert_eq!((paths, untracked), (vec!["f".into()], false));
    sandbox.run(&path, "checkout", &["--", "f"]).unwrap();
    switch(
        git,
        &repository,
        &SwitchTo::Branch("main".into()),
        &cancel(),
    )
    .unwrap();
    std::fs::write(path.join("g"), "untracked\n").unwrap();
    let blocked = switch(
        git,
        &repository,
        &SwitchTo::Branch("other".into()),
        &cancel(),
    );
    assert!(
        matches!(
            blocked,
            Err(SwitchError::WouldOverwrite {
                untracked: true,
                ..
            })
        ),
        "{blocked:?}"
    );
    std::fs::remove_file(path.join("g")).unwrap();

    let first = rev(&sandbox, &path, "main~1");
    let new = SwitchTo::NewBranch {
        name: "fix".into(),
        at: first,
    };
    switch(git, &repository, &new, &cancel()).unwrap();
    assert_eq!(branch(&sandbox, &repository).as_deref(), Some("fix"));
    assert_eq!(rev(&sandbox, &path, "HEAD"), first);
    switch(git, &repository, &SwitchTo::Detached(first), &cancel()).unwrap();
    assert_eq!(branch(&sandbox, &repository), None);
    switch(
        git,
        &repository,
        &SwitchTo::Branch("main".into()),
        &cancel(),
    )
    .unwrap();

    create_branch(git, &repository, BStr::new("later"), first, &cancel()).unwrap();
    rename_branch(
        git,
        &repository,
        BStr::new("later"),
        BStr::new("sooner"),
        &cancel(),
    )
    .unwrap();
    assert_eq!(rev(&sandbox, &path, "sooner"), first);
    // `other` has a commit nowhere else: deleting it is confirmed first.
    let unmerged = delete_branch(git, &repository, BStr::new("other"), false, &cancel());
    assert!(
        matches!(unmerged, Err(DeleteBranchError::NotMerged)),
        "{unmerged:?}"
    );
    delete_branch(git, &repository, BStr::new("other"), true, &cancel()).unwrap();
    delete_branch(git, &repository, BStr::new("sooner"), false, &cancel()).unwrap();
    let left = sandbox
        .run(&path, "branch", &["--format=%(refname:short)"])
        .unwrap();
    assert_eq!(left.stdout.trim_ascii(), b"fix\nmain");
}

#[test]
fn a_remote_branch_is_tracked_deleted_and_a_tag_pushed() {
    let sandbox = Sandbox::new().unwrap();
    let seed = two_branches(&sandbox);
    let bare = sandbox.path("origin.git");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &[
                "--quiet",
                "--bare",
                seed.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        )
        .unwrap();
    let path = sandbox.path("mine");
    let url = format!("file://{}", bare.display());
    let git = sandbox.git();
    clone(
        git,
        &url,
        &path,
        CloneOptions::default(),
        &Progress::new(),
        &cancel(),
    )
    .unwrap();
    let repository = open(&sandbox, &path);

    let track = SwitchTo::Track {
        remote_branch: "origin/other".into(),
        local: "other".into(),
    };
    switch(git, &repository, &track, &cancel()).unwrap();
    let upstream = sandbox
        .run(
            &path,
            "for-each-ref",
            &["--format=%(upstream:short)", "refs/heads/other"],
        )
        .unwrap();
    assert_eq!(upstream.stdout.trim_ascii(), b"origin/other");

    let deleted = delete_remote_branch(
        git,
        &repository,
        BStr::new("origin"),
        BStr::new("other"),
        &Progress::new(),
        &cancel(),
    )
    .unwrap();
    assert_eq!(deleted.refs[0].flag, PushFlag::Deleted);
    let on_origin = sandbox
        .run(&bare, "branch", &["--format=%(refname:short)"])
        .unwrap();
    assert_eq!(on_origin.stdout.trim_ascii(), b"main");

    let head = rev(&sandbox, &path, "HEAD");
    create_tag(git, &repository, BStr::new("v1"), head, None, &cancel()).unwrap();
    let pushed = push_tag(
        git,
        &repository,
        BStr::new("origin"),
        BStr::new("v1"),
        &Progress::new(),
        &cancel(),
    )
    .unwrap();
    assert!(pushed.succeeded(), "{pushed:?}");
    assert_eq!(rev(&sandbox, &bare, "v1"), head);
}

#[test]
fn commits_are_merged_rebased_picked_reverted_and_reset() {
    let sandbox = Sandbox::new().unwrap();
    let path = two_branches(&sandbox);
    let repository = open(&sandbox, &path);
    let git = sandbox.git();
    let operations = || repository.operations(sandbox.git()).unwrap();
    // `f` is "one" there, "two" now.
    let one = rev(&sandbox, &path, "main~1");

    // Picked, then picked again: its change is in, so it is skipped.
    let other = rev(&sandbox, &path, "other");
    assert_eq!(
        cherry_pick(git, &repository, other, &cancel()).unwrap(),
        Integrated::Done
    );
    assert_eq!(subject(&sandbox, &path, "HEAD"), "Other");
    let picked = rev(&sandbox, &path, "HEAD");
    assert_eq!(
        cherry_pick(git, &repository, other, &cancel()).unwrap(),
        Integrated::Empty
    );
    assert_eq!(operations(), []);
    assert_eq!(rev(&sandbox, &path, "HEAD"), picked);

    assert_eq!(
        revert(git, &repository, picked, &cancel()).unwrap(),
        Integrated::Done
    );
    assert_eq!(subject(&sandbox, &path, "HEAD"), "Revert \"Other\"");
    assert!(!path.join("g").exists());

    // Merged: a merge commit (`other` is no ancestor of `main`).
    assert_eq!(
        merge(
            git,
            &repository,
            BStr::new("other"),
            FastForward::AsConfigured,
            &cancel()
        )
        .unwrap(),
        Integrated::Done
    );
    let parents = sandbox.run(&path, "log", &["-1", "--format=%P"]).unwrap();
    assert_eq!(parents.stdout.split(|&byte| byte == b' ').count(), 2);
    // A merge's changes, against its first parent, reverted.
    let merged = rev(&sandbox, &path, "HEAD");
    assert_eq!(
        revert(git, &repository, merged, &cancel()).unwrap(),
        Integrated::Done
    );

    // Reset: soft keeps the changes staged, mixed unstaged, hard nowhere.
    let start = one;
    let before = rev(&sandbox, &path, "HEAD");
    reset(git, &repository, start, ResetMode::Soft, &cancel()).unwrap();
    let staged = sandbox
        .run(&path, "diff", &["--cached", "--name-only"])
        .unwrap();
    assert_eq!(staged.stdout.trim_ascii(), b"f");
    reset(git, &repository, before, ResetMode::Soft, &cancel()).unwrap();
    reset(git, &repository, start, ResetMode::Mixed, &cancel()).unwrap();
    let changed = sandbox.run(&path, "diff", &["--name-only"]).unwrap();
    assert_eq!(changed.stdout.trim_ascii(), b"f");
    reset(git, &repository, start, ResetMode::Hard, &cancel()).unwrap();
    assert_eq!(std::fs::read_to_string(path.join("f")).unwrap(), "one\n");
    assert_eq!(rev(&sandbox, &path, "HEAD"), start);

    // Conflicts stop each, the operation in progress.
    sandbox.commit_file(&path, "g", "mine\n", "Mine").unwrap();
    assert_eq!(
        merge(
            git,
            &repository,
            BStr::new("other"),
            FastForward::AsConfigured,
            &cancel()
        )
        .unwrap(),
        Integrated::Conflicts
    );
    assert_eq!(operations(), [Operation::Merge]);
    sandbox.run(&path, "merge", &["--abort"]).unwrap();
    assert_eq!(
        rebase(git, &repository, BStr::new("other"), false, &cancel()).unwrap(),
        Integrated::Conflicts
    );
    assert_eq!(operations(), [Operation::Rebase]);
    sandbox.run(&path, "rebase", &["--abort"]).unwrap();
    assert_eq!(
        cherry_pick(git, &repository, other, &cancel()).unwrap(),
        Integrated::Conflicts
    );
    assert_eq!(operations(), [Operation::CherryPick]);
    sandbox.run(&path, "cherry-pick", &["--abort"]).unwrap();
    // A clean rebase: `main`'s own commit replayed onto `other`.
    sandbox
        .run(&path, "reset", &["--quiet", "--hard", "HEAD~1"])
        .unwrap();
    sandbox.commit_file(&path, "h", "later\n", "Later").unwrap();
    assert_eq!(
        rebase(git, &repository, BStr::new("other"), false, &cancel()).unwrap(),
        Integrated::Done
    );
    assert_eq!(subject(&sandbox, &path, "HEAD~1"), "Other");
}

#[test]
fn a_merge_fast_forwards_as_configured_or_never_and_names_a_branch_as_people_do() {
    let sandbox = Sandbox::new().unwrap();
    let path = sandbox.init("repo").unwrap();
    sandbox.commit_file(&path, "f", "one\n", "One").unwrap();
    sandbox.run(&path, "branch", &["ahead"]).unwrap();
    sandbox.run(&path, "switch", &["--quiet", "ahead"]).unwrap();
    sandbox.commit_file(&path, "g", "two\n", "Two").unwrap();
    sandbox.run(&path, "switch", &["--quiet", "main"]).unwrap();
    let (repository, git) = (open(&sandbox, &path), sandbox.git());
    let (one, ahead) = (rev(&sandbox, &path, "main"), rev(&sandbox, &path, "ahead"));

    // Never: a merge commit, though `main` could just move forward.
    assert_eq!(
        merge(
            git,
            &repository,
            BStr::new("ahead"),
            FastForward::Never,
            &cancel()
        )
        .unwrap(),
        Integrated::Done
    );
    let parents = sandbox.run(&path, "log", &["-1", "--format=%P"]).unwrap();
    assert_eq!(
        String::from_utf8(parents.stdout).unwrap().trim(),
        format!("{one} {ahead}")
    );
    assert_eq!(subject(&sandbox, &path, "HEAD"), "Merge branch 'ahead'");

    // As configured (unset): it just moves forward.
    sandbox
        .run(&path, "reset", &["--hard", "--quiet", &one.to_string()])
        .unwrap();
    assert_eq!(
        merge(
            git,
            &repository,
            BStr::new("ahead"),
            FastForward::AsConfigured,
            &cancel()
        )
        .unwrap(),
        Integrated::Done
    );
    assert_eq!(rev(&sandbox, &path, "HEAD"), ahead);

    // By its full name: git's message names it as people do.
    let merge_full = |sandbox: &Sandbox| {
        sandbox
            .run(&path, "reset", &["--hard", "--quiet", &one.to_string()])
            .unwrap();
        merge(
            git,
            &repository,
            BStr::new("refs/heads/ahead"),
            FastForward::Never,
            &cancel(),
        )
        .unwrap();
    };
    merge_full(&sandbox);
    assert_eq!(subject(&sandbox, &path, "HEAD"), "Merge branch 'ahead'");
    // Beside a tag of its name (elsewhere), still the branch, named apart from the tag.
    sandbox
        .run(&path, "tag", &["ahead", &one.to_string()])
        .unwrap();
    merge_full(&sandbox);
    assert_eq!(
        subject(&sandbox, &path, "HEAD"),
        "Merge branch 'heads/ahead'"
    );
    let parents = sandbox.run(&path, "log", &["-1", "--format=%P"]).unwrap();
    assert!(
        String::from_utf8(parents.stdout)
            .unwrap()
            .contains(&ahead.to_string())
    );
}

#[test]
fn tags_are_made_annotated_or_light_and_deleted() {
    let sandbox = Sandbox::new().unwrap();
    let path = two_branches(&sandbox);
    let repository = open(&sandbox, &path);
    let git = sandbox.git();
    // Signing every tag is the user's choice; a lightweight tag isn't signed anyway.
    sandbox
        .run(&path, "config", &["tag.gpgSign", "true"])
        .unwrap();
    let head = rev(&sandbox, &path, "HEAD");
    create_tag(git, &repository, BStr::new("light"), head, None, &cancel()).unwrap();
    let kind = sandbox.run(&path, "cat-file", &["-t", "light"]).unwrap();
    assert_eq!(kind.stdout.trim_ascii(), b"commit");
    sandbox
        .run(&path, "config", &["tag.gpgSign", "false"])
        .unwrap();
    let message = "Release one\n\n  its notes  \n";
    create_tag(
        git,
        &repository,
        BStr::new("v1"),
        head,
        Some(message),
        &cancel(),
    )
    .unwrap();
    let shown = sandbox
        .run(&path, "tag", &["--list", "--format=%(contents)", "v1"])
        .unwrap();
    assert_eq!(shown.stdout.trim_ascii(), b"Release one\n\n  its notes");
    assert!(create_tag(git, &repository, BStr::new("v1"), head, None, &cancel()).is_err());
    delete_tag(git, &repository, BStr::new("v1"), &cancel()).unwrap();
    let left = sandbox.run(&path, "tag", &["--list"]).unwrap();
    assert_eq!(left.stdout.trim_ascii(), b"light");
}

#[test]
fn stashes_are_made_listed_applied_popped_and_dropped() {
    let sandbox = Sandbox::new().unwrap();
    let path = two_branches(&sandbox);
    let repository = open(&sandbox, &path);
    let git = sandbox.git();
    assert!(!stash_push(git, &repository, "", false, &cancel()).unwrap());
    assert_eq!(stashes(git, &repository, &cancel()).unwrap(), []);

    std::fs::write(path.join("f"), "changed\n").unwrap();
    assert!(stash_push(git, &repository, "First", false, &cancel()).unwrap());
    std::fs::write(path.join("new"), "untracked\n").unwrap();
    assert!(stash_push(git, &repository, "", true, &cancel()).unwrap());
    assert!(!path.join("new").exists());
    let listed = stashes(git, &repository, &cancel()).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].index, 1);
    assert_eq!(listed[1].message, "On main: First");
    assert!(listed[0].message.starts_with("WIP on main:"), "{listed:?}");

    // The untracked file back, the stash kept; then popped.
    assert_eq!(
        stash_apply(git, &repository, &listed[0], false, &cancel()).unwrap(),
        Integrated::Done
    );
    assert!(path.join("new").exists());
    std::fs::remove_file(path.join("new")).unwrap();
    assert_eq!(
        stash_apply(git, &repository, &listed[0], true, &cancel()).unwrap(),
        Integrated::Done
    );
    std::fs::remove_file(path.join("new")).unwrap();
    // The list moved: `First` isn't second any more.
    let stale = stash_drop(git, &repository, &listed[1], &cancel());
    assert!(matches!(stale, Err(StashError::Stale)), "{stale:?}");

    // `First` changes `f`, as the work tree now does: applying it conflicts.
    let listed = stashes(git, &repository, &cancel()).unwrap();
    sandbox
        .commit_file(&path, "f", "committed\n", "Committed")
        .unwrap();
    assert_eq!(
        stash_apply(git, &repository, &listed[0], true, &cancel()).unwrap(),
        Integrated::Conflicts
    );
    // A stash that conflicts is kept.
    sandbox.run(&path, "reset", &["--quiet", "--hard"]).unwrap();
    let listed = stashes(git, &repository, &cancel()).unwrap();
    assert_eq!(listed.len(), 1);
    stash_drop(git, &repository, &listed[0], &cancel()).unwrap();
    assert_eq!(stashes(git, &repository, &cancel()).unwrap(), []);
}
