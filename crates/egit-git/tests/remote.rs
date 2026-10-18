//! Cloning, fetching, pulling, and pushing against a local bare repository reached through
//! `file://`, so git talks to it as to any remote (and reports its progress).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, CloneOptions, FetchOptions, GitError, Operation, Progress, PullMode, Pulled,
    PushFlag, PushOptions, Repository, abort_operation, clone, continue_operation, count_between,
    fetch, fetch_refspecs, pull, push, remote_urls, remotes,
};

/// A bare repository, `origin.git`, allowing partial clones, with `main` holding a commit; and
/// its address.
fn origin(sandbox: &Sandbox) -> (PathBuf, String) {
    let bare = sandbox.path("origin.git");
    sandbox
        .run(
            sandbox.home(),
            "init",
            &["--quiet", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
    sandbox
        .run(&bare, "config", &["uploadpack.allowFilter", "true"])
        .unwrap();
    let seed = sandbox.init("seed").unwrap();
    for index in 0..20 {
        std::fs::write(seed.join(format!("file{index}")), format!("{index}\n")).unwrap();
    }
    std::fs::write(seed.join("shared"), "base\n").unwrap();
    sandbox.run(&seed, "add", &["--all"]).unwrap();
    sandbox
        .run(&seed, "commit", &["--quiet", "--message", "Start"])
        .unwrap();
    sandbox
        .run(
            &seed,
            "push",
            &["--quiet", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        )
        .unwrap();
    let url = format!("file://{}", bare.display());
    (bare, url)
}

/// A clone of `url` named `name`, made by egit.
fn cloned(sandbox: &Sandbox, url: &str, name: &str) -> (PathBuf, Repository) {
    let destination = sandbox.path(name);
    clone(
        sandbox.git(),
        url,
        &destination,
        CloneOptions::default(),
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    let repository = Repository::discover(sandbox.git(), &destination).unwrap();
    (destination, repository)
}

/// Commits `contents` to `file` in `repo` and pushes it to `origin`'s `main` with plain git.
fn commit_and_push(sandbox: &Sandbox, repo: &Path, file: &str, contents: &str, message: &str) {
    sandbox.commit_file(repo, file, contents, message).unwrap();
    sandbox
        .run(repo, "push", &["--quiet", "origin", "HEAD:refs/heads/main"])
        .unwrap();
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> String {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn clones_reporting_progress_whole_or_partial() {
    let sandbox = Sandbox::new().unwrap();
    let (_, url) = origin(&sandbox);
    let progress = Progress::new();
    let whole = sandbox.path("whole");
    clone(
        sandbox.git(),
        &url,
        &whole,
        CloneOptions::default(),
        &progress,
        &CancelToken::new(),
    )
    .unwrap();
    assert!(whole.join("file7").exists());
    let report = progress.report();
    assert!(report.updates > 0);
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.starts_with("Cloning into")),
        "{report:?}"
    );

    let partial = sandbox.path("partial");
    clone(
        sandbox.git(),
        &url,
        &partial,
        CloneOptions {
            submodules: true,
            partial: true,
        },
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    let filter = sandbox
        .run(&partial, "config", &["remote.origin.partialclonefilter"])
        .unwrap();
    assert_eq!(filter.stdout.trim_ascii(), b"blob:none");

    // Not over what is there, and not once cancelled.
    let again = clone(
        sandbox.git(),
        &url,
        &whole,
        CloneOptions::default(),
        &Progress::new(),
        &CancelToken::new(),
    );
    assert!(matches!(again, Err(GitError::Failed(_))));
    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = clone(
        sandbox.git(),
        &url,
        &sandbox.path("never"),
        CloneOptions::default(),
        &Progress::new(),
        &cancel,
    );
    assert!(matches!(cancelled, Err(GitError::Cancelled { .. })));
}

#[test]
fn a_clone_cancelled_midway_leaves_nothing_behind() {
    let sandbox = Sandbox::new().unwrap();
    let (bare, _) = origin(&sandbox);
    // The remote answers only after a long pause (git's `ext::` transport runs a command as
    // the remote): the clone is cancelled while it waits, its folder already made.
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.ext.allow", "always"],
        )
        .unwrap();
    let url = format!(
        "ext::sh -c sleep% 60;% exec% {}% upload-pack% {}",
        sandbox.git().program().display(),
        bare.display()
    );
    let destination = sandbox.path("halfway");
    let cancel = CancelToken::new();
    let cancelling = {
        let (cancel, destination) = (cancel.clone(), destination.clone());
        std::thread::spawn(move || {
            while !destination.join(".git").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(100));
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let cancelled = clone(
        sandbox.git(),
        &url,
        &destination,
        CloneOptions::default(),
        &Progress::new(),
        &cancel,
    );
    cancelling.join().unwrap();
    assert!(
        matches!(cancelled, Err(GitError::Cancelled { .. })),
        "{cancelled:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!destination.exists(), "git left {}", destination.display());
}

#[test]
fn fetches_every_remote_and_prunes_what_is_gone() {
    let sandbox = Sandbox::new().unwrap();
    let (_, url) = origin(&sandbox);
    let (colleague, _) = cloned(&sandbox, &url, "colleague");
    sandbox
        .run(
            &colleague,
            "push",
            &["--quiet", "origin", "HEAD:refs/heads/feature"],
        )
        .unwrap();
    let (mine, repository) = cloned(&sandbox, &url, "mine");
    assert_eq!(remotes(sandbox.git(), &repository).unwrap(), ["origin"]);
    assert!(
        sandbox
            .run(
                &mine,
                "rev-parse",
                &["--verify", "--quiet", "origin/feature"]
            )
            .is_ok()
    );

    sandbox
        .run(
            &colleague,
            "push",
            &["--quiet", "origin", ":refs/heads/feature"],
        )
        .unwrap();
    commit_and_push(&sandbox, &colleague, "news", "x\n", "News");
    let options = FetchOptions {
        all: true,
        prune: true,
        background: true,
    };
    fetch(
        sandbox.git(),
        &repository,
        options,
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(
        sandbox
            .run(
                &mine,
                "rev-parse",
                &["--verify", "--quiet", "origin/feature"]
            )
            .is_err()
    );
    assert_eq!(
        rev(&sandbox, &mine, "origin/main"),
        rev(&sandbox, &colleague, "HEAD")
    );
}

#[test]
fn pulls_forward_and_asks_when_branches_diverged() {
    let sandbox = Sandbox::new().unwrap();
    let (_, url) = origin(&sandbox);
    let (colleague, _) = cloned(&sandbox, &url, "colleague");
    let (mine, repository) = cloned(&sandbox, &url, "mine");
    let pulled = |mode| {
        pull(
            sandbox.git(),
            &repository,
            (mode, false),
            &Progress::new(),
            &CancelToken::new(),
        )
    };

    commit_and_push(&sandbox, &colleague, "theirs", "x\n", "Theirs");
    assert_eq!(pulled(PullMode::AsConfigured).unwrap(), Pulled::Updated);
    assert_eq!(
        rev(&sandbox, &mine, "HEAD"),
        rev(&sandbox, &colleague, "HEAD")
    );

    // Each side has a commit the other hasn't, and nothing says how to bring them together.
    commit_and_push(&sandbox, &colleague, "theirs", "y\n", "Theirs again");
    sandbox.commit_file(&mine, "mine", "m\n", "Mine").unwrap();
    assert_eq!(pulled(PullMode::AsConfigured).unwrap(), Pulled::Diverged);
    assert_eq!(pulled(PullMode::FastForwardOnly).unwrap(), Pulled::Diverged);
    assert_eq!(pulled(PullMode::Rebase).unwrap(), Pulled::Updated);
    assert_eq!(
        rev(&sandbox, &mine, "HEAD^"),
        rev(&sandbox, &colleague, "HEAD"),
        "mine on top of theirs"
    );
}

#[test]
fn changes_in_a_pulls_way_are_named_or_stashed_and_put_back() {
    let sandbox = Sandbox::new().unwrap();
    let (_, url) = origin(&sandbox);
    let (colleague, _) = cloned(&sandbox, &url, "colleague");
    let (mine, repository) = cloned(&sandbox, &url, "mine");
    let pulled = |mode, autostash| {
        pull(
            sandbox.git(),
            &repository,
            (mode, autostash),
            &Progress::new(),
            &CancelToken::new(),
        )
        .unwrap()
    };
    let stashes = || {
        let out = sandbox.run(&mine, "stash", &["list"]).unwrap();
        out.stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count()
    };
    commit_and_push(&sandbox, &colleague, "notes", "one\ntwo\nthree\n", "Notes");
    assert_eq!(pulled(PullMode::AsConfigured, false), Pulled::Updated);

    // Their change to a file mine changes too: named, nothing pulled; a rebase names nothing.
    commit_and_push(&sandbox, &colleague, "notes", "ONE\ntwo\nthree\n", "Shout");
    std::fs::write(mine.join("notes"), "one\ntwo\nTHREE\n").unwrap();
    let in_the_way = Pulled::ChangesInTheWay {
        paths: vec!["notes".into()],
        untracked: false,
    };
    assert_eq!(pulled(PullMode::Merge, false), in_the_way);
    assert_eq!(pulled(PullMode::FastForwardOnly, false), in_the_way);
    assert_eq!(
        pulled(PullMode::Rebase, false),
        Pulled::ChangesInTheWay {
            paths: Vec::new(),
            untracked: false
        }
    );
    assert_ne!(
        rev(&sandbox, &mine, "HEAD"),
        rev(&sandbox, &colleague, "HEAD")
    );

    // Stashed first, then put back: theirs and mine both, no stash left.
    assert_eq!(pulled(PullMode::AsConfigured, true), Pulled::Updated);
    assert_eq!(
        rev(&sandbox, &mine, "HEAD"),
        rev(&sandbox, &colleague, "HEAD")
    );
    assert_eq!(
        std::fs::read_to_string(mine.join("notes")).unwrap(),
        "ONE\ntwo\nTHREE\n"
    );
    assert_eq!(stashes(), 0);

    // Put back over a change of theirs to the same line: conflicts, the stash kept.
    commit_and_push(&sandbox, &colleague, "notes", "ONE\ntwo\n3\n", "Count");
    assert_eq!(pulled(PullMode::Rebase, true), Pulled::ChangesConflicted);
    assert_eq!(
        rev(&sandbox, &mine, "HEAD"),
        rev(&sandbox, &colleague, "HEAD")
    );
    assert_eq!(stashes(), 1);
    sandbox
        .run(&mine, "checkout", &["--theirs", "--", "notes"])
        .unwrap();
    sandbox.run(&mine, "reset", &["--quiet"]).unwrap();
    sandbox.run(&mine, "stash", &["drop", "--quiet"]).unwrap();

    // A file of theirs that mine has untracked: named, and a stash first doesn't help.
    commit_and_push(&sandbox, &colleague, "new", "theirs\n", "New");
    std::fs::write(mine.join("new"), "mine\n").unwrap();
    assert_eq!(
        pulled(PullMode::AsConfigured, true),
        Pulled::ChangesInTheWay {
            paths: vec!["new".into()],
            untracked: true
        }
    );
}

#[test]
fn a_conflicted_pull_is_aborted_or_continued() {
    let sandbox = Sandbox::new().unwrap();
    let (_, url) = origin(&sandbox);
    let (colleague, _) = cloned(&sandbox, &url, "colleague");
    let (mine, repository) = cloned(&sandbox, &url, "mine");
    commit_and_push(&sandbox, &colleague, "shared", "theirs\n", "Theirs");
    sandbox
        .commit_file(&mine, "shared", "mine\n", "Mine")
        .unwrap();
    let before = rev(&sandbox, &mine, "HEAD");
    let cancel = CancelToken::new();
    let pulled = |mode| {
        pull(
            sandbox.git(),
            &repository,
            (mode, false),
            &Progress::new(),
            &cancel,
        )
    };

    assert_eq!(pulled(PullMode::Merge).unwrap(), Pulled::Conflicts);
    assert_eq!(repository.operations().unwrap(), [Operation::Merge]);
    abort_operation(sandbox.git(), &repository, Operation::Merge, &cancel).unwrap();
    assert_eq!(repository.operations().unwrap(), []);
    assert_eq!(rev(&sandbox, &mine, "HEAD"), before);

    assert_eq!(pulled(PullMode::Rebase).unwrap(), Pulled::Conflicts);
    assert_eq!(repository.operations().unwrap(), [Operation::Rebase]);
    abort_operation(sandbox.git(), &repository, Operation::Rebase, &cancel).unwrap();
    assert_eq!(rev(&sandbox, &mine, "HEAD"), before);

    // Resolved and staged, the merge carries on (its message as git made it).
    assert_eq!(pulled(PullMode::Merge).unwrap(), Pulled::Conflicts);
    let refused = continue_operation(sandbox.git(), &repository, Operation::Merge, &cancel);
    assert!(refused.is_err(), "conflicts are left");
    std::fs::write(mine.join("shared"), "both\n").unwrap();
    sandbox.run(&mine, "add", &["shared"]).unwrap();
    continue_operation(sandbox.git(), &repository, Operation::Merge, &cancel).unwrap();
    assert_eq!(repository.operations().unwrap(), []);
    let parents = sandbox
        .run(&mine, "rev-list", &["--parents", "-n", "1", "HEAD"])
        .unwrap();
    assert_eq!(
        String::from_utf8(parents.stdout)
            .unwrap()
            .split(' ')
            .count(),
        3,
        "a merge commit"
    );
}

#[test]
fn pushes_new_branches_and_explains_refusals_and_forces_with_a_lease() {
    let sandbox = Sandbox::new().unwrap();
    let (bare, url) = origin(&sandbox);
    let (colleague, _) = cloned(&sandbox, &url, "colleague");
    let (mine, repository) = cloned(&sandbox, &url, "mine");
    let pushed = |branch: &str, set_upstream, force| {
        push(
            sandbox.git(),
            &repository,
            &PushOptions {
                remote: "origin".into(),
                branch: branch.into(),
                remote_branch: branch.into(),
                set_upstream,
                force,
            },
            &Progress::new(),
            &CancelToken::new(),
        )
        .unwrap()
    };

    // A new branch, made the local one's upstream.
    sandbox
        .run(&mine, "switch", &["--quiet", "--create", "topic"])
        .unwrap();
    sandbox.commit_file(&mine, "topic", "t\n", "Topic").unwrap();
    let new = pushed("topic", true, false);
    assert!(new.succeeded());
    assert_eq!(new.refs[0].flag, PushFlag::New);
    // Git's own syntax, not a format string.
    #[allow(clippy::literal_string_with_formatting_args)]
    let upstream = sandbox
        .run(&mine, "rev-parse", &["--abbrev-ref", "topic@{upstream}"])
        .unwrap();
    assert_eq!(upstream.stdout.trim_ascii(), b"origin/topic");

    // Behind the remote: refused, saying why.
    sandbox.run(&mine, "switch", &["--quiet", "main"]).unwrap();
    commit_and_push(&sandbox, &colleague, "theirs", "x\n", "Theirs");
    sandbox.commit_file(&mine, "mine", "m\n", "Mine").unwrap();
    let refused = pushed("main", false, false);
    assert!(!refused.succeeded());
    assert_eq!(refused.refs[0].flag, PushFlag::Rejected);
    assert_eq!(
        refused.refs[0].reason.as_ref().map(ToString::to_string),
        Some("fetch first".to_owned())
    );

    // A commit already pushed, then amended: replaced with a lease, since the remote is still
    // what was last fetched, and that was in the branch.
    sandbox
        .run(&mine, "pull", &["--quiet", "--rebase", "origin", "main"])
        .unwrap();
    assert!(pushed("main", false, false).succeeded());
    sandbox
        .run(
            &mine,
            "commit",
            &["--quiet", "--amend", "--message", "Mine, amended"],
        )
        .unwrap();
    assert!(!pushed("main", false, false).succeeded());
    let forced = pushed("main", false, true);
    assert!(forced.succeeded());
    assert_eq!(forced.refs[0].flag, PushFlag::Forced);

    // The remote moved on since: forcing would lose that, so it is refused.
    sandbox
        .run(&colleague, "fetch", &["--quiet", "origin"])
        .unwrap();
    sandbox
        .run(&colleague, "reset", &["--quiet", "--hard", "origin/main"])
        .unwrap();
    commit_and_push(&sandbox, &colleague, "later", "l\n", "Later");
    sandbox
        .run(
            &mine,
            "commit",
            &["--quiet", "--amend", "--message", "Mine, amended again"],
        )
        .unwrap();
    let stale = pushed("main", false, true);
    assert!(!stale.succeeded());
    assert_eq!(
        rev(&sandbox, &bare, "main"),
        rev(&sandbox, &colleague, "HEAD"),
        "the colleague's work stays"
    );
}

#[test]
fn a_pull_request_s_head_and_a_lone_branch_are_fetched_by_refspec() {
    let sandbox = Sandbox::new().unwrap();
    let (bare, url) = origin(&sandbox);
    let (work, repository) = cloned(&sandbox, &url, "work");
    // A forge keeps a pull request's head outside the branches; and a branch the clone hasn't
    // seen.
    commit_and_push(&sandbox, &work, "feature", "1\n", "Feature");
    let head = rev(&sandbox, &work, "HEAD");
    sandbox
        .run(&bare, "update-ref", &["refs/pull/7/head", &head])
        .unwrap();
    sandbox
        .run(&bare, "update-ref", &["refs/heads/topic", &head])
        .unwrap();
    fetch_refspecs(
        sandbox.git(),
        &repository,
        "origin".into(),
        &[
            "refs/pull/7/head:refs/heads/pr/7".into(),
            "+refs/heads/topic:refs/remotes/origin/topic".into(),
        ],
        "Fetching pull request #7",
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(rev(&sandbox, &work, "pr/7"), head);
    assert_eq!(rev(&sandbox, &work, "origin/topic"), head);
    // No tags came along, and a ref the remote lacks fails.
    let missing = fetch_refspecs(
        sandbox.git(),
        &repository,
        "origin".into(),
        &["refs/pull/8/head:refs/heads/pr/8".into()],
        "Fetching pull request #8",
        &Progress::new(),
        &CancelToken::new(),
    );
    assert!(missing.is_err());
}

#[test]
fn remote_urls_are_as_git_rewrites_them() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .run(&repo, "remote", &["add", "origin", "gh:octo/tool"])
        .unwrap();
    sandbox
        .run(
            &repo,
            "remote",
            &["add", "plain", "https://example.com/a/b.git"],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "config",
            &["url.https://github.com/.insteadOf", "gh:"],
        )
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let urls = remote_urls(sandbox.git(), &repository).unwrap();
    assert_eq!(
        urls,
        [
            ("origin".into(), "https://github.com/octo/tool".into()),
            ("plain".into(), "https://example.com/a/b.git".into()),
        ]
    );
}

#[test]
fn a_fetch_moves_the_upstream_by_the_commits_it_brings() {
    let sandbox = Sandbox::new().unwrap();
    let (_, url) = origin(&sandbox);
    let (mine, repository) = cloned(&sandbox, &url, "mine");
    let (theirs, _) = cloned(&sandbox, &url, "theirs");
    let git = sandbox.git();
    let (name, before) = repository.upstream_tip(git).unwrap().unwrap();
    assert_eq!(name, "refs/remotes/origin/main");
    assert_eq!(before.to_string(), rev(&sandbox, &mine, "origin/main"));

    commit_and_push(&sandbox, &theirs, "a", "1\n", "One");
    commit_and_push(&sandbox, &theirs, "b", "2\n", "Two");
    let options = FetchOptions {
        all: true,
        prune: true,
        background: true,
    };
    fetch(
        git,
        &repository,
        options,
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    let (_, after) = repository.upstream_tip(git).unwrap().unwrap();
    assert_ne!(after, before);
    let cancel = CancelToken::new();
    assert_eq!(
        count_between(git, &repository, before, after, &cancel).unwrap(),
        2
    );
    assert_eq!(
        count_between(git, &repository, after, before, &cancel).unwrap(),
        0
    );

    // No upstream: a branch of its own, or detached.
    sandbox
        .run(&mine, "switch", &["--quiet", "-c", "alone"])
        .unwrap();
    assert_eq!(repository.upstream_tip(git).unwrap(), None);
    sandbox
        .run(&mine, "switch", &["--quiet", "--detach"])
        .unwrap();
    assert_eq!(repository.upstream_tip(git).unwrap(), None);
}
