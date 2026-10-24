//! Following a repository on another computer: this one, through the tests' sshd (skipped
//! without one, unless `EGIT_REQUIRE_SSHD` is set), or their relay (`EGIT_TEST_RELAY`). Told at
//! once by `inotifywait` (skipped without inotify-tools, unless `EGIT_REQUIRE_INOTIFYWAIT` is
//! set) of refs, the index, an operation's state, and the configuration, the repository's and the
//! user's; looking every second where it isn't installed, or stops; a work tree's folders
//! followed, those not named left alone; files followed, told or looked at, others in their
//! folder left alone; and every watcher there stopped as soon as the session is dropped.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egit_git::testing::Sandbox;
use egit_git::{
    Follow, Git, GitOptions, Host, INOTIFYWAIT, Looked, Looking, Repository, Seen, Wait as _,
    look_at_files_there, look_at_folders_there, look_there,
};

/// The sandbox's git, run on "another computer"; `None` (the test skipped) without the tests'
/// sshd.
fn there(sandbox: &Sandbox, tmp: &Path) -> Option<Git> {
    if egit_git::testing::through_a_relay() {
        std::fs::create_dir_all(tmp).unwrap();
        let relay = egit_git::testing::relay().unwrap();
        // As below: the folders' list goes here.
        let git = sandbox
            .git_through(&relay, [("TMPDIR", tmp.as_os_str().to_owned())])
            .unwrap();
        return Some(git);
    }
    let host = match egit_git::testing::ssh_host() {
        Ok(host) => host,
        Err(error) => {
            assert!(
                std::env::var_os("EGIT_REQUIRE_SSHD").is_none(),
                "EGIT_REQUIRE_SSHD is set, but the tests' sshd couldn't start: {error}"
            );
            return None;
        }
    };
    std::fs::create_dir_all(tmp).unwrap();
    let mut options = GitOptions {
        host: Host::Ssh(host),
        ..sandbox.options()
    };
    // The folders' list goes here, so this test's watchers are found by it.
    options
        .env
        .push(("TMPDIR".into(), tmp.as_os_str().to_owned()));
    Some(Git::new(options).wait().unwrap())
}

/// Whether `inotifywait` is installed (here is there); when it must be and isn't, fails.
fn inotifywait_installed() -> bool {
    let found = Command::new(INOTIFYWAIT)
        .arg("--help")
        .output()
        .is_ok_and(|output| !output.stdout.is_empty() || !output.stderr.is_empty());
    assert!(
        found || std::env::var_os("EGIT_REQUIRE_INOTIFYWAIT").is_none(),
        "EGIT_REQUIRE_INOTIFYWAIT is set, but inotifywait isn't installed"
    );
    found
}

/// Waits up to `limit` for `done`.
fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// Waits up to `limit` for `wanted` among what changed, taking all that came before.
fn changed(changes: &Receiver<Looked>, wanted: Looked, limit: Duration) -> bool {
    let until = Instant::now() + limit;
    let mut seen = Looked::NONE;
    while !seen.intersects(wanted) {
        let left = until.saturating_duration_since(Instant::now());
        match changes.recv_timeout(left) {
            Ok(looked) => seen = seen.union(looked),
            Err(_) => return false,
        }
    }
    true
}

/// Whether a process whose command line holds `marker` runs here (and so "there").
fn running(marker: &OsStr) -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg(marker)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn repository(git: &Git, path: &Path) -> Repository {
    Repository::discover(git, path).wait().unwrap()
}

/// Follows the repository as `follow` says; what changed comes through the receiver.
fn follow(git: &Git, repo: &Path, follow: &Follow) -> (Looking, Receiver<Looked>) {
    let (sender, changes) = mpsc::channel();
    let looking = look_there(git, &repository(git, repo), follow, move |looked| {
        let _ = sender.send(looked);
    })
    .unwrap();
    (looking, changes)
}

#[test]
fn told_at_once_of_refs_the_index_state_and_configuration() {
    if !inotifywait_installed() {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    let Some(git) = there(&sandbox, &sandbox.path("tmp")) else {
        return;
    };
    // Looking only every minute: whatever comes sooner was told.
    let (looking, changes) = follow(&git, &repo, &Follow::at_once(Duration::from_secs(60)));
    assert!(
        eventually(Duration::from_secs(10), || looking.is_live()),
        "never told at once"
    );
    let soon = Duration::from_secs(5);

    sandbox.commit_file(&repo, "f", "2\n", "Second").unwrap();
    assert!(changed(&changes, Looked::REFS, soon), "a commit");
    std::fs::write(repo.join("g"), "new\n").unwrap();
    sandbox.run(&repo, "add", &["g"]).unwrap();
    assert!(changed(&changes, Looked::INDEX, soon), "a file staged");
    sandbox.run(&repo, "bisect", &["start"]).unwrap();
    assert!(
        changed(&changes, Looked::STATE, soon),
        "a bisection started"
    );
    sandbox.run(&repo, "bisect", &["reset"]).unwrap();
    sandbox
        .run(&repo, "config", &["egit.test", "repository"])
        .unwrap();
    assert!(changed(&changes, Looked::CONFIG, soon), "the repository's");
    sandbox
        .run(sandbox.home(), "config", &["--global", "egit.test", "user"])
        .unwrap();
    assert!(changed(&changes, Looked::CONFIG, soon), "the user's");
    sandbox.run(&repo, "branch", &["topic"]).unwrap();
    assert!(changed(&changes, Looked::REFS, soon), "a branch made");
    // Its folder made since the session began: followed too.
    sandbox.run(&repo, "branch", &["feature/deep"]).unwrap();
    assert!(
        changed(&changes, Looked::REFS, soon),
        "a branch in a folder"
    );
    sandbox.run(&repo, "branch", &["feature/deeper"]).unwrap();
    assert!(
        changed(&changes, Looked::REFS, soon),
        "another in that folder"
    );

    // Dropped, its watchers there stop at once (not at their next heartbeat).
    let marker = repo.join(".git").into_os_string();
    assert!(running(&marker));
    drop(looking);
    assert!(
        eventually(Duration::from_secs(3), || !running(&marker)),
        "watchers left running there"
    );
}

#[test]
fn looks_every_second_where_inotifywait_isnt_there() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    let Some(git) = there(&sandbox, &sandbox.path("tmp")) else {
        return;
    };
    // None asked for; one that isn't installed; and one that stops at once, as `inotifywait`
    // does when inotify's limits are reached (noticed at the session's next heartbeat).
    let watchers = [
        None,
        Some("egit-no-such-inotifywait".into()),
        Some("false".into()),
    ];
    for (round, inotifywait) in watchers.into_iter().enumerate() {
        let looking_only = Follow {
            every: Duration::from_secs(1),
            inotifywait,
        };
        let (looking, changes) = follow(&git, &repo, &looking_only);
        // Its first look only notes how things are (a watcher stopping is noticed in 5 s).
        let first_look = if round == 2 { 6_500 } else { 1_500 };
        std::thread::sleep(Duration::from_millis(first_look));
        assert!(!looking.is_live());
        sandbox.run(&repo, "branch", &["-f", "topic"]).unwrap();
        sandbox
            .commit_file(&repo, "f", &format!("{round}\n"), "More")
            .unwrap();
        assert!(changed(&changes, Looked::REFS, Duration::from_secs(5)));
        assert!(!looking.has_ended());
    }
}

#[test]
fn a_work_trees_folders_are_followed_and_no_others() {
    if !inotifywait_installed() {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    for folder in ["src", "build"] {
        std::fs::create_dir(repo.join(folder)).unwrap();
    }
    sandbox.commit_file(&repo, "src/a", "1\n", "Start").unwrap();
    let tmp = sandbox.path("tmp-folders");
    let Some(git) = there(&sandbox, &tmp) else {
        return;
    };
    let repository = repository(&git, &repo);
    let (sender, seen) = mpsc::channel();
    let folders = [repo.clone(), repo.join("src"), repo.join("gone")];
    let looking = look_at_folders_there(
        &git,
        &repository,
        folders.iter().map(PathBuf::as_path),
        OsStr::new(INOTIFYWAIT),
        move |saw| {
            let _ = sender.send(saw);
        },
    )
    .unwrap();
    let soon = Duration::from_secs(5);
    // A folder named but gone is left out, rather than failing the rest.
    assert_eq!(seen.recv_timeout(Duration::from_secs(10)), Ok(Seen::Ready));
    assert!(looking.is_live());

    std::fs::write(repo.join("src/a"), "2\n").unwrap();
    assert_eq!(seen.recv_timeout(soon), Ok(Seen::Changed));
    while seen.recv_timeout(Duration::from_millis(300)).is_ok() {}
    std::fs::create_dir(repo.join("src/new")).unwrap();
    assert_eq!(
        seen.recv_timeout(soon),
        Ok(Seen::Created(repo.join("src/new")))
    );
    while seen.recv_timeout(Duration::from_millis(300)).is_ok() {}
    // Neither a folder not named, nor git's own.
    std::fs::write(repo.join("build/out"), "x\n").unwrap();
    sandbox.run(&repo, "branch", &["topic"]).unwrap();
    assert!(seen.recv_timeout(Duration::from_secs(1)).is_err());

    let marker = tmp.into_os_string();
    assert!(running(&marker));
    drop(looking);
    assert!(
        eventually(Duration::from_secs(3), || !running(&marker)),
        "the watcher left running there"
    );

    // Without inotifywait there, it says so.
    let (sender, seen) = mpsc::channel();
    let _looking = look_at_folders_there(
        &git,
        &repository,
        folders.iter().map(PathBuf::as_path),
        OsStr::new("egit-no-such-inotifywait"),
        move |saw| {
            let _ = sender.send(saw);
        },
    )
    .unwrap();
    assert_eq!(seen.recv_timeout(soon), Ok(Seen::Missing));
}

/// Follows `files` as `follow` says; each change comes through the receiver.
fn follow_files(git: &Git, files: &[PathBuf], follow: &Follow) -> (Looking, Receiver<()>) {
    let (sender, changes) = mpsc::channel();
    let looking = look_at_files_there(git, files, follow, move || {
        let _ = sender.send(());
    })
    .unwrap();
    (looking, changes)
}

#[test]
fn files_are_followed_and_no_others() {
    if !inotifywait_installed() {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let tmp = sandbox.path("tmp-files");
    let Some(git) = there(&sandbox, &tmp) else {
        return;
    };
    let home = sandbox.home();
    // One whose folder isn't there is left out, rather than failing the rest.
    let files = [home.join(".gitconfig"), home.join(".config/nowhere/config")];
    // Looking only every minute: whatever comes sooner was told.
    let (looking, changes) = follow_files(&git, &files, &Follow::at_once(Duration::from_secs(60)));
    assert!(
        eventually(Duration::from_secs(10), || looking.is_live()),
        "never told at once"
    );
    let soon = Duration::from_secs(5);
    sandbox
        .run(home, "config", &["--global", "egit.test", "user"])
        .unwrap();
    assert!(changes.recv_timeout(soon).is_ok(), "the file written");
    while changes.recv_timeout(Duration::from_millis(300)).is_ok() {}
    std::fs::write(home.join("other"), "x\n").unwrap();
    assert!(
        changes.recv_timeout(Duration::from_secs(1)).is_err(),
        "another file in its folder"
    );

    let marker = tmp.into_os_string();
    assert!(running(&marker));
    drop(looking);
    assert!(
        eventually(Duration::from_secs(3), || !running(&marker)),
        "the watcher left running there"
    );
}

#[test]
fn files_are_looked_at_where_inotifywait_isnt_there() {
    let sandbox = Sandbox::new().unwrap();
    let Some(git) = there(&sandbox, &sandbox.path("tmp")) else {
        return;
    };
    let home = sandbox.home();
    let looking_only = Follow {
        every: Duration::from_secs(1),
        inotifywait: Some("egit-no-such-inotifywait".into()),
    };
    let (looking, changes) = follow_files(&git, &[home.join(".gitconfig")], &looking_only);
    // Its first look only notes how things are.
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(!looking.is_live());
    assert!(changes.try_recv().is_err());
    sandbox
        .run(home, "config", &["--global", "egit.test", "user"])
        .unwrap();
    assert!(changes.recv_timeout(Duration::from_secs(5)).is_ok());
    assert!(!looking.has_ended());
}
