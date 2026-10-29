//! Hooks with the real git: which a repository has (its hooks folder, `core.hooksPath`), and
//! what a commit's hooks print, as it comes, into a transcript: their colours, a refusal, skipped
//! with `--no-verify`, and cancelled part-way.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egit_git::testing::{Sandbox, set_mode};
use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{
    ApplyAs, CancelToken, Checks, Color, FastForward, GitError, Integrated, LineKind, NewCommit,
    ObjectId, Operation, Planned, Repository, Step, Transcript, Wait as _, apply_patch,
    cherry_pick_all, continue_operation, create_commit, installed_hooks, merge, rebase,
    rebase_interactive, reword_head,
};

/// Writes `script` as the hook `name` in `folder`, runnable unless `runnable` is false.
fn hook(folder: &Path, name: &str, script: &str, runnable: bool) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    set_mode(&path, if runnable { 0o755 } else { 0o644 }).unwrap();
    path
}

/// A repository with one commit and a change staged.
fn staged(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "one\n", "First").unwrap();
    std::fs::write(repo.join("a"), "two\n").unwrap();
    sandbox.run(&repo, "add", &["a"]).unwrap();
    repo
}

/// Every line's kind and text.
fn lines(transcript: &Transcript) -> Vec<(LineKind, String)> {
    transcript.read(|log| {
        (0..log.len())
            .filter_map(|at| log.line(at))
            .map(|line| (line.kind, line.text.clone()))
            .collect()
    })
}

fn commit(message: &str) -> NewCommit {
    NewCommit {
        message: message.to_owned(),
        sign: Some(false),
        ..NewCommit::default()
    }
}

#[test]
fn the_hooks_a_repository_has_are_the_runnable_ones_where_git_looks() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    let hooks_folder = repo.join(".git/hooks");
    hook(&hooks_folder, "pre-commit", "exit 0", true);
    hook(&hooks_folder, "commit-msg", "exit 0", false);
    hook(&hooks_folder, "pre-push.sample", "exit 0", true);
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let hooks = installed_hooks(sandbox.git(), &repository).wait().unwrap();
    assert_eq!(
        hooks.names,
        ["pre-commit"],
        "not the unrunnable, nor a sample"
    );
    assert!(!hooks.configured);
    assert!(hooks.folder.ends_with(".git/hooks"));
    assert_eq!(
        hooks.of(&["pre-commit", "commit-msg", "post-commit"]),
        ["pre-commit"]
    );

    // A team's folder, by `core.hooksPath`.
    hook(&repo.join(".husky"), "pre-push", "exit 0", true);
    sandbox
        .run(&repo, "config", &["core.hooksPath", ".husky"])
        .unwrap();
    let hooks = installed_hooks(sandbox.git(), &repository).wait().unwrap();
    assert_eq!(hooks.names, ["pre-push"]);
    assert!(hooks.configured && hooks.folder.ends_with(".husky"));
}

#[test]
fn a_commit_s_hooks_print_into_its_transcript() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    let hooks = repo.join(".git/hooks");
    hook(
        &hooks,
        "pre-commit",
        r#"printf '\033[32mchecking\033[0m style\n'; echo "colour: $PRE_COMMIT_COLOR""#,
        true,
    );
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let transcript = Transcript::new();
    let git = sandbox.git().with_transcript(&transcript);
    create_commit(&git, &repository, &commit("Checked"), &CancelToken::new())
        .wait()
        .unwrap();
    let said = lines(&transcript);
    assert_eq!(said[0].0, LineKind::Command);
    assert!(said[0].1.starts_with("git commit --file=-"), "{said:?}");
    assert!(
        said.contains(&(LineKind::Err, "checking style".to_owned())),
        "a hook's output goes to standard error: {said:?}"
    );
    assert!(said.contains(&(LineKind::Err, "colour: always".to_owned())));
    assert!(
        said.iter()
            .any(|(kind, text)| *kind == LineKind::Out && text.contains("Checked")),
        "git's own summary: {said:?}"
    );
    transcript.read(|log| {
        let checking = (0..log.len())
            .filter_map(|at| log.line(at))
            .find(|line| line.text == "checking style")
            .unwrap();
        assert_eq!(checking.spans[0].style.color, Some(Color::Indexed(2)));
    });
    // A reading command prints nothing into it; nor does a git without a transcript.
    let before = transcript.updates();
    sandbox
        .git()
        .write("status")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    git.read("status")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_eq!(transcript.updates(), before);
}

#[test]
fn a_hook_s_refusal_and_skipping_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    hook(
        &repo.join(".git/hooks"),
        "pre-commit",
        "echo 'lint: 2 problems' >&2; exit 1",
        true,
    );
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let transcript = Transcript::new();
    let git = sandbox.git().with_transcript(&transcript);
    let refused = create_commit(&git, &repository, &commit("Refused"), &CancelToken::new()).wait();
    assert!(matches!(refused, Err(GitError::Failed(_))));
    let said = lines(&transcript);
    assert!(said.contains(&(LineKind::Err, "lint: 2 problems".to_owned())));
    assert!(
        matches!(said.last(), Some((LineKind::Ended { success: false }, _))),
        "{said:?}"
    );

    // Skipped: made.
    let skipping = NewCommit {
        checks: Checks::Skip,
        ..commit("Skipped")
    };
    create_commit(&git, &repository, &skipping, &CancelToken::new())
        .wait()
        .unwrap();
    let head = sandbox.run(&repo, "log", &["-1", "--format=%s"]).unwrap();
    assert_eq!(head.stdout.trim_ascii(), b"Skipped");
}

#[test]
fn a_hook_cancelled_part_way_makes_no_commit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = staged(&sandbox);
    hook(
        &repo.join(".git/hooks"),
        "pre-commit",
        "echo 'running the tests'; sleep 30",
        true,
    );
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let transcript = Transcript::new();
    let git = sandbox.git().with_transcript(&transcript);
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let started = Instant::now();
    let watching = transcript.clone();
    std::thread::spawn(move || {
        // Once the hook said it started.
        while !watching.read(|log| {
            (0..log.len()).any(|at| {
                log.line(at)
                    .is_some_and(|line| line.text == "running the tests")
            })
        }) {
            std::thread::sleep(Duration::from_millis(20));
        }
        stopper.cancel();
    });
    let made = create_commit(&git, &repository, &commit("Never"), &cancel).wait();
    assert!(matches!(made, Err(GitError::Cancelled { .. })), "{made:?}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the hook was stopped"
    );
    let head = sandbox.run(&repo, "log", &["-1", "--format=%s"]).unwrap();
    assert_eq!(head.stdout.trim_ascii(), b"First");
    let said = lines(&transcript);
    assert_eq!(
        said.last(),
        Some(&(LineKind::Ended { success: false }, "Cancelled.".to_owned()))
    );
}

/// A repository whose hooks that check (and `post-commit`, which runs anyway) write their
/// names into `log`: `main` and `side` from `First`, `side` changing `s` and `c`, `main` `c`
/// too (each other's conflict).
struct Checked {
    repo: PathBuf,
    repository: Repository,
    log: PathBuf,
}

impl Checked {
    fn new(sandbox: &Sandbox, name: &str) -> Self {
        let repo = sandbox.init(name).unwrap();
        sandbox.commit_file(&repo, "a", "one\n", "First").unwrap();
        sandbox
            .run(&repo, "switch", &["--quiet", "--create", "side"])
            .unwrap();
        sandbox.commit_file(&repo, "s", "side\n", "Side").unwrap();
        sandbox
            .commit_file(&repo, "c", "side\n", "Side's c")
            .unwrap();
        sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
        sandbox
            .commit_file(&repo, "c", "main\n", "Main's c")
            .unwrap();
        let log = sandbox.path(format!("{name}.log"));
        for name in egit_git::CHECK_HOOKS.iter().chain(&["post-commit"]) {
            let line = format!("echo {name} >> '{}'", log.display());
            hook(&repo.join(".git/hooks"), name, &line, true);
        }
        let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
        Self {
            repo,
            repository,
            log,
        }
    }

    /// The hooks that ran since last asked.
    fn ran(&self) -> Vec<String> {
        let said = std::fs::read_to_string(&self.log).unwrap_or_default();
        std::fs::remove_file(&self.log).ok();
        said.lines().map(str::to_owned).collect()
    }

    /// `c` resolved and staged.
    fn resolve(&self, sandbox: &Sandbox) {
        std::fs::write(self.repo.join("c"), "both\n").unwrap();
        sandbox.run(&self.repo, "add", &["c"]).unwrap();
    }

    fn rev(&self, sandbox: &Sandbox, name: &str) -> ObjectId {
        let out = sandbox.run(&self.repo, "rev-parse", &[name]).unwrap();
        ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
    }

    fn subjects(&self, sandbox: &Sandbox, count: &str) -> Vec<String> {
        let out = sandbox
            .run(&self.repo, "log", &["--format=%s", count])
            .unwrap();
        out.stdout
            .to_str_lossy()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// Asserts the hooks that check among `ran` are `wanted`, or none when `checks` skips them.
fn expect(checks: Checks, ran: &[String], wanted: &[&str], what: &str) {
    let ran: Vec<&str> = ran
        .iter()
        .map(String::as_str)
        .filter(|hook| egit_git::CHECK_HOOKS.contains(hook))
        .collect();
    let wanted = if checks.skipped() { &[][..] } else { wanted };
    assert_eq!(ran, wanted, "{what}, {checks:?}");
}

/// What a test of the checks names its repositories after.
const fn tag(checks: Checks) -> &'static str {
    if checks.skipped() { "skip" } else { "run" }
}

#[test]
fn a_merge_runs_its_checks_or_skips_them_and_so_does_one_continued() {
    let sandbox = Sandbox::new().unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    for checks in [Checks::Run, Checks::Skip] {
        let merging = Checked::new(&sandbox, &format!("merge-{}", tag(checks)));
        let side = merging.rev(&sandbox, "side~1").to_string();
        merge(
            git,
            &merging.repository,
            side.as_bytes().as_bstr(),
            FastForward::Never,
            checks,
            &cancel,
        )
        .wait()
        .unwrap();
        expect(
            checks,
            &merging.ran(),
            &["pre-merge-commit", "commit-msg"],
            "a merge",
        );
        let stopped = merge(
            git,
            &merging.repository,
            b"side".as_bstr(),
            FastForward::AsConfigured,
            checks,
            &cancel,
        )
        .wait()
        .unwrap();
        assert_eq!(stopped, Integrated::Conflicts);
        merging.ran();
        merging.resolve(&sandbox);
        continue_operation(git, &merging.repository, Operation::Merge, checks, &cancel)
            .wait()
            .unwrap();
        expect(
            checks,
            &merging.ran(),
            &["pre-commit", "commit-msg"],
            "continued",
        );
        // git's message kept, and nothing left under way.
        assert_eq!(merging.subjects(&sandbox, "-1"), ["Merge branch 'side'"]);
        assert_eq!(merging.repository.operations(git).wait().unwrap(), []);
    }
}

#[test]
fn picks_and_reverts_continued_run_their_checks_or_skip_them() {
    let sandbox = Sandbox::new().unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    for checks in [Checks::Run, Checks::Skip] {
        // Two picked, the first stopping at a conflict: continued, both made.
        let picking = Checked::new(&sandbox, &format!("pick-{}", tag(checks)));
        let commits = [
            picking.rev(&sandbox, "side"),
            picking.rev(&sandbox, "side~1"),
        ];
        let stopped = cherry_pick_all(git, &picking.repository, &commits, &cancel)
            .wait()
            .unwrap();
        assert_eq!(stopped, Integrated::Conflicts);
        picking.ran();
        picking.resolve(&sandbox);
        let operation = Operation::CherryPick;
        continue_operation(git, &picking.repository, operation, checks, &cancel)
            .wait()
            .unwrap();
        expect(
            checks,
            &picking.ran(),
            &["pre-commit", "commit-msg"],
            "picks",
        );
        assert_eq!(
            picking.subjects(&sandbox, "-3"),
            ["Side", "Side's c", "Main's c"]
        );
        assert_eq!(picking.repository.operations(git).wait().unwrap(), []);

        // A revert stopped at a conflict, continued.
        let reverting = Checked::new(&sandbox, &format!("revert-{}", tag(checks)));
        sandbox
            .commit_file(&reverting.repo, "c", "again\n", "Main's c again")
            .unwrap();
        let reverted = reverting.rev(&sandbox, "HEAD~1");
        reverting.ran();
        let stopped = egit_git::revert(git, &reverting.repository, reverted, &cancel)
            .wait()
            .unwrap();
        assert_eq!(stopped, Integrated::Conflicts);
        reverting.resolve(&sandbox);
        let operation = Operation::Revert;
        continue_operation(git, &reverting.repository, operation, checks, &cancel)
            .wait()
            .unwrap();
        expect(
            checks,
            &reverting.ran(),
            &["pre-commit", "commit-msg"],
            "a revert",
        );
        assert_eq!(reverting.subjects(&sandbox, "-1"), ["Revert \"Main's c\""]);
    }
}

#[test]
fn rebases_and_rewords_run_their_checks_or_skip_them() {
    let sandbox = Sandbox::new().unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    for checks in [Checks::Run, Checks::Skip] {
        let rebasing = Checked::new(&sandbox, &format!("rebase-{}", tag(checks)));
        sandbox
            .run(&rebasing.repo, "switch", &["--quiet", "side"])
            .unwrap();
        sandbox
            .run(&rebasing.repo, "reset", &["--quiet", "--hard", "side~1"])
            .unwrap();
        let onto = b"main".as_bstr();
        rebase(git, &rebasing.repository, onto, false, checks, &cancel)
            .wait()
            .unwrap();
        expect(checks, &rebasing.ran(), &["pre-rebase"], "a rebase");
        let base = rebasing.rev(&sandbox, "main");
        let plan = [Planned {
            id: rebasing.rev(&sandbox, "HEAD"),
            step: Step::Reword("Side, reworded".to_owned()),
        }];
        rebase_interactive(
            git,
            &rebasing.repository,
            Some(base),
            &plan,
            checks,
            &cancel,
        )
        .wait()
        .unwrap();
        let reworded = ["pre-rebase", "pre-commit", "commit-msg"];
        expect(checks, &rebasing.ran(), &reworded, "a reword");
        assert_eq!(rebasing.subjects(&sandbox, "-1"), ["Side, reworded"]);
        reword_head(git, &rebasing.repository, "Side, again", checks, &cancel)
            .wait()
            .unwrap();
        expect(
            checks,
            &rebasing.ran(),
            &["pre-commit", "commit-msg"],
            "HEAD",
        );
        assert_eq!(rebasing.subjects(&sandbox, "-1"), ["Side, again"]);
    }
}

#[test]
fn patches_applied_run_their_checks_or_skip_them_continued_too() {
    let sandbox = Sandbox::new().unwrap();
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    for checks in [Checks::Run, Checks::Skip] {
        let applying = Checked::new(&sandbox, &format!("am-{}", tag(checks)));
        let mailbox = |range: &[&str]| -> Arc<[u8]> {
            let made = sandbox.run(&applying.repo, "format-patch", range).unwrap();
            made.stdout.as_bytes().into()
        };
        let (first, series) = (
            mailbox(&["--stdout", "-1", "side~1"]),
            mailbox(&["--stdout", "main..side"]),
        );
        let how = ApplyAs::Commits { sign_off: false };
        apply_patch(git, &applying.repository, &first, how, checks, &cancel)
            .wait()
            .unwrap();
        let checked = ["applypatch-msg", "pre-applypatch"];
        expect(checks, &applying.ran(), &checked, "a patch");
        sandbox
            .run(&applying.repo, "reset", &["--quiet", "--hard", "HEAD~1"])
            .unwrap();
        let stopped = apply_patch(git, &applying.repository, &series, how, checks, &cancel)
            .wait()
            .unwrap();
        assert_eq!(stopped, Integrated::Conflicts);
        applying.ran();
        applying.resolve(&sandbox);
        let operation = Operation::ApplyMailbox;
        continue_operation(git, &applying.repository, operation, checks, &cancel)
            .wait()
            .unwrap();
        expect(checks, &applying.ran(), &["pre-applypatch"], "continued");
        assert_eq!(
            applying.subjects(&sandbox, "-3"),
            ["Side's c", "Side", "Main's c"]
        );
    }
}
