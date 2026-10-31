//! Several commits picked together and signed: a range by Shift-click, one more added by
//! Ctrl-click and taken away again; the details list them; their menu signs them (each made
//! again and signed, the commit after them too, the one before them left alone); Undo puts the
//! branch back. Two picked on another branch cherry-picked onto the current one, oldest first,
//! then reverted together. What commits picked change together: a range's files and a file's
//! diff; commits apart merged in turn, nothing here changing; one that doesn't go with those
//! before it, left out; and the panel's buttons kept to its width by a long branch's name.

use std::path::Path;

use egit_git::testing::Sandbox;
use egui::Modifiers;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, harness, history_shown, open_path, profile_loaded, render, settle,
    wait_for_label,
};

/// `git log`'s `format` for each commit of `main`, the newest first.
fn log(sandbox: &Sandbox, repo: &Path, format: &str) -> Vec<String> {
    let out = sandbox
        .run(repo, "log", &[&format!("--format={format}"), "main"])
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A repository whose commits are signed by an SSH key file, holding One to Four; `None`
/// where ssh-keygen isn't installed (unless every signer is required).
fn signing_repo(sandbox: &Sandbox) -> Option<std::path::PathBuf> {
    let has_ssh_keygen = std::process::Command::new("ssh-keygen")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    assert!(
        has_ssh_keygen || std::env::var_os("EGIT_REQUIRE_SIGNERS").is_none(),
        "ssh-keygen is needed to test signing"
    );
    if !has_ssh_keygen {
        return None;
    }
    let key = sandbox.home().join("id_ed25519");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "tests", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(made.success());
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let allowed = sandbox.home().join("allowed_signers");
    std::fs::write(
        &allowed,
        format!("tests@egit.invalid namespaces=\"git\" {}", public.trim()),
    )
    .unwrap();
    let repo = sandbox.init("repo").unwrap();
    for setting in [
        ["gpg.format", "ssh"],
        ["user.signingKey", key.to_str().unwrap()],
        ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
    ] {
        sandbox.run(&repo, "config", &setting).unwrap();
    }
    for (file, subject) in [("a", "One"), ("b", "Two"), ("c", "Three"), ("d", "Four")] {
        sandbox.commit_file(&repo, file, "x\n", subject).unwrap();
    }
    Some(repo)
}

#[test]
fn picked_commits_are_signed_from_their_menu() {
    let sandbox = Sandbox::new().unwrap();
    let Some(repo) = signing_repo(&sandbox) else {
        return;
    };
    let before = log(&sandbox, &repo, "%H");
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Two, then Shift-click Three: both picked; Ctrl-click One adds it, again takes it away.
    harness.get_by_role_and_label(Role::Button, "Two").click();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Three")
        .click_modifiers(Modifiers::SHIFT);
    wait_for_label(&mut harness, "2 commits selected");
    harness
        .get_by_role_and_label(Role::Button, "One")
        .click_modifiers(Modifiers::COMMAND);
    wait_for_label(&mut harness, "3 commits selected");
    harness
        .get_by_role_and_label(Role::Button, "One")
        .click_modifiers(Modifiers::COMMAND);
    wait_for_label(&mut harness, "2 commits selected");
    render(&mut harness, "commits-picked");

    // Their menu: signing them, asked first.
    harness
        .get_by_role_and_label(Role::Button, "Three")
        .click_secondary();
    wait_for_label(&mut harness, "Sign these commits…");
    // The menu's, over the table (the details offer it too).
    harness
        .query_all_by_label("Sign these commits…")
        .min_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .unwrap()
        .click();
    wait_for_label(&mut harness, "Sign 2 commits?");
    render(&mut harness, "sign-commits");
    harness.get_by_label("Sign").click();
    wait_for_label(
        &mut harness,
        "Signed 2 commits, and the 1 commit after them, made again.",
    );

    // Two, Three, and Four made again, signed; One left as it was; the picked ones let go.
    let after = log(&sandbox, &repo, "%H");
    assert_eq!(after.last(), before.last(), "One is the same");
    assert!(
        after
            .iter()
            .zip(&before)
            .take(3)
            .all(|(new, old)| new != old)
    );
    assert_eq!(
        log(&sandbox, &repo, "%G? %s"),
        ["G Four", "G Three", "G Two", "N One"]
    );
    // Let go once the history is read again (their commits are gone from it).
    for _ in 0..400 {
        if harness
            .query_all_by_label("2 commits selected")
            .next()
            .is_none()
        {
            break;
        }
        harness.step();
    }
    assert!(
        harness
            .query_all_by_label("2 commits selected")
            .next()
            .is_none()
    );

    // Undo: the branch back where it was.
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid signing 2 commits.");
    assert_eq!(log(&sandbox, &repo, "%H"), before);
}

#[test]
fn one_commit_is_signed_from_its_bulk_actions() {
    let sandbox = Sandbox::new().unwrap();
    let Some(repo) = signing_repo(&sandbox) else {
        return;
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Its menu's Bulk actions: what picking several offers, for it alone.
    harness
        .get_by_role_and_label(Role::Button, "Four")
        .click_secondary();
    wait_for_label(&mut harness, "Bulk actions");
    harness.get_by_label("Bulk actions").hover();
    wait_for_label(&mut harness, "Sign this commit…");
    harness.get_by_label("Sign this commit…").click();
    wait_for_label(&mut harness, "Sign 1 commit?");
    harness.get_by_label("Sign").click();
    wait_for_label(&mut harness, "Signed 1 commit.");
    assert_eq!(
        log(&sandbox, &repo, "%G? %s"),
        ["G Four", "N Three", "N Two", "N One"]
    );
}

#[test]
fn picked_commits_are_cherry_picked_and_reverted_together() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "base", "x\n", "Base").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    for (file, subject) in [("a", "A"), ("b", "B"), ("c", "C")] {
        sandbox.commit_file(&repo, file, "1\n", subject).unwrap();
    }
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    // main moved on: the picks are new commits (from Base, each would be made again as it was).
    sandbox.commit_file(&repo, "m", "1\n", "Main").unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // A and C, B left out: picked onto main in the order they were made.
    harness.get_by_role_and_label(Role::Button, "A").click();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "C")
        .click_modifiers(Modifiers::COMMAND);
    wait_for_label(&mut harness, "2 commits selected");
    render(&mut harness, "picked-panel");
    // Their menu offers it as the panel does.
    harness
        .get_by_role_and_label(Role::Button, "C")
        .click_secondary();
    let offered = |harness: &egui_kittest::Harness<'_, crate::app::EgitApp>| {
        harness.get_all_by_label("Cherry-pick onto main").count()
    };
    for _ in 0..200 {
        if offered(&harness) == 2 {
            break;
        }
        harness.step();
    }
    assert_eq!(offered(&harness), 2);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    click_when_enabled(&mut harness, "Cherry-pick onto main");
    wait_for_label(&mut harness, "Picked 2 commits.");
    assert_eq!(log(&sandbox, &repo, "%s"), ["C", "A", "Main", "Base"]);

    // Both reverted together, the newest first. side goes first: its commits are named as the
    // picks are, and which of two made in the same second comes first is git's to choose.
    sandbox
        .run(&repo, "branch", &["--quiet", "-D", "side"])
        .unwrap();
    settle(&mut harness, "the picks shown", |app| {
        app.state.repos[0].history.shown.rows() == 4
    });
    harness
        .query_all_by_role_and_label(Role::Button, "C")
        .next()
        .unwrap()
        .click();
    harness.run_steps(2);
    harness
        .query_all_by_role_and_label(Role::Button, "A")
        .next()
        .unwrap()
        .click_modifiers(Modifiers::COMMAND);
    wait_for_label(&mut harness, "2 commits selected");
    click_when_enabled(&mut harness, "Revert in main");
    wait_for_label(&mut harness, "Reverted 2 commits.");
    assert_eq!(
        log(&sandbox, &repo, "%s"),
        ["Revert \"A\"", "Revert \"C\"", "C", "A", "Main", "Base"]
    );
    assert!(!repo.join("a").exists() && !repo.join("c").exists());
}

/// A repository holding `commits`: each a subject and a file written with its contents, in turn.
fn repo_of(sandbox: &Sandbox, commits: &[(&str, &str, &str)]) -> std::path::PathBuf {
    let repo = sandbox.init("repo").unwrap();
    for (subject, file, contents) in commits {
        sandbox.commit_file(&repo, file, contents, subject).unwrap();
    }
    repo
}

/// Clicks the commit table's row of `subject`, with these keys held.
fn click_row(
    harness: &mut egui_kittest::Harness<'_, crate::app::EgitApp>,
    subject: &str,
    keys: Modifiers,
) {
    harness
        .get_all_by_role_and_label(Role::Button, subject)
        .next()
        .unwrap()
        .click_modifiers(keys);
    harness.run_steps(1);
}

#[test]
fn a_range_picked_shows_what_it_changes_together() {
    let sandbox = Sandbox::new().unwrap();
    let repo = repo_of(
        &sandbox,
        &[
            ("Start", "a.txt", "one\ntwo\n"),
            ("Change a", "a.txt", "one\nTWO\n"),
            ("Add b", "b.txt", "bee\n"),
            ("Change a again", "a.txt", "ONE\nTWO\nthree\n"),
        ],
    );
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    click_row(&mut harness, "Change a", Modifiers::NONE);
    wait_for_label(&mut harness, "1 file changed");
    click_row(&mut harness, "Change a again", Modifiers::SHIFT);
    wait_for_label(&mut harness, "3 commits selected");
    // Not "Start"'s: from what the range started from, each file once.
    wait_for_label(&mut harness, "2 files changed");
    wait_for_label(&mut harness, "+4");
    wait_for_label(&mut harness, "−2");
    render(&mut harness, "picked-together");

    // A file's diff, from where they started to where they ended; back with them still picked.
    harness.get_by_role_and_label(Role::Button, "a.txt").click();
    settle(&mut harness, "a.txt's diff", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("a.txt".to_owned(), true))
    });
    // The hunk's head, what it started as (two lines), and what it ended as (three): no sign of
    // the lines the first of them made and the last replaced.
    assert_eq!(
        harness.state_mut().state.repos[0].history_view.diff_lines(),
        Some(6)
    );
    assert!(
        harness
            .query_all_by_label("3 commits selected")
            .next()
            .is_some()
    );
    render(&mut harness, "picked-together-file");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
    assert_eq!(
        harness.state_mut().state.repos[0].history_view.file_shown(),
        None
    );
    harness.get_by_role_and_label(Role::Button, "Change a again");
    assert!(
        harness
            .query_all_by_label("3 commits selected")
            .next()
            .is_some()
    );

    // A commit fewer: from elsewhere, so the files are read anew.
    click_row(&mut harness, "Add b", Modifiers::SHIFT);
    wait_for_label(&mut harness, "2 commits selected");
    wait_for_label(&mut harness, "2 files changed");
    wait_for_label(&mut harness, "+2");
    // Down to the first commit (the range still stretching from where it began): from nothing.
    click_row(&mut harness, "Start", Modifiers::SHIFT);
    wait_for_label(&mut harness, "1 file changed");
    wait_for_label(&mut harness, "a.txt");
    assert!(harness.query_by_label("b.txt").is_none());

    // Let go, the commit's own again.
    click_row(&mut harness, "Add b", Modifiers::NONE);
    wait_for_label(&mut harness, "1 file changed");
}

#[test]
fn commits_apart_are_merged_in_turn_and_nothing_changes() {
    let sandbox = Sandbox::new().unwrap();
    let repo = repo_of(
        &sandbox,
        &[
            ("Start", "a.txt", "a\n"),
            ("Add b", "b.txt", "bee\n"),
            ("Add c", "c.txt", "sea\n"),
            ("Add d", "d.txt", "dee\n"),
        ],
    );
    let refs = |sandbox: &Sandbox| sandbox.run(&repo, "for-each-ref", &[]).unwrap().stdout;
    let before = refs(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    click_row(&mut harness, "Add b", Modifiers::NONE);
    wait_for_label(&mut harness, "1 file changed");
    click_row(&mut harness, "Add d", Modifiers::COMMAND);
    wait_for_label(&mut harness, "2 commits selected");
    wait_for_label(&mut harness, "2 files changed");
    wait_for_label(&mut harness, "b.txt");
    wait_for_label(&mut harness, "d.txt");
    assert!(harness.query_by_label("c.txt").is_none());
    assert!(
        harness
            .query_by_label_contains("Not next to each other")
            .is_some()
    );
    render(&mut harness, "picked-apart");

    // Its diff reads, though only they lead to the commit it runs to; a blame goes to the newest
    // of those picked, whose history is real.
    harness.get_by_role_and_label(Role::Button, "d.txt").click();
    settle(&mut harness, "d.txt's diff", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("d.txt".to_owned(), true))
    });
    assert_eq!(
        harness.state_mut().state.repos[0].history_view.diff_lines(),
        Some(2)
    );
    let newest = sandbox
        .run(&repo, "rev-parse", &["--short=8", "HEAD"])
        .unwrap()
        .stdout;
    let newest = String::from_utf8(newest).unwrap();
    click_when_enabled(&mut harness, "Blame");
    wait_for_label(&mut harness, &format!("at {}", newest.trim()));
    assert_eq!(refs(&sandbox), before);
    let status = sandbox
        .run(&repo, "status", &["--porcelain", "--untracked-files=all"])
        .unwrap()
        .stdout;
    assert_eq!(status, b"");
}

#[test]
fn a_commit_that_doesn_t_go_with_those_before_it_can_be_left_out() {
    let sandbox = Sandbox::new().unwrap();
    let repo = repo_of(
        &sandbox,
        &[
            ("Start", "a.txt", "one\n"),
            ("Add b", "b.txt", "bee\n"),
            ("Add c", "c.txt", "sea\n"),
            ("Change a", "a.txt", "two\n"),
            ("Change a again", "a.txt", "three\n"),
        ],
    );
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    click_row(&mut harness, "Add b", Modifiers::NONE);
    wait_for_label(&mut harness, "1 file changed");
    click_row(&mut harness, "Add c", Modifiers::COMMAND);
    wait_for_label(&mut harness, "2 commits selected");
    click_row(&mut harness, "Change a again", Modifiers::COMMAND);
    wait_for_label(&mut harness, "3 commits selected");
    // "Change a" isn't picked, and "Change a again" changes the line it wrote.
    wait_for_label(&mut harness, "Leave it out");
    assert!(
        harness
            .query_by_label_contains("They don't go together")
            .is_some()
    );
    render(&mut harness, "picked-conflict");
    click_when_enabled(&mut harness, "Leave it out");
    wait_for_label(&mut harness, "2 commits selected");
    wait_for_label(&mut harness, "2 files changed");
    wait_for_label(&mut harness, "c.txt");
}

#[test]
fn a_long_branch_name_keeps_the_panels_buttons_inside_it() {
    const BRANCH: &str = "chore/a-branch-with-a-name-much-too-long-for-the-panel";
    let sandbox = Sandbox::new().unwrap();
    let repo = repo_of(
        &sandbox,
        &[
            ("Start", "a.txt", "a\n"),
            ("Add b", "b.txt", "bee\n"),
            ("Add c", "c.txt", "sea\n"),
        ],
    );
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", BRANCH])
        .unwrap();
    let mut harness = super::screens::windowed(&sandbox, [880.0, 800.0], 1.0, egui::Theme::Dark);
    let inner_edge = 880.0 - 12.0;
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    click_row(&mut harness, "Add b", Modifiers::NONE);
    wait_for_label(&mut harness, "1 file changed");
    click_row(&mut harness, "Add c", Modifiers::SHIFT);
    wait_for_label(&mut harness, "2 files changed");
    for words in [
        format!("Cherry-pick onto {BRANCH}"),
        format!("Revert in {BRANCH}"),
    ] {
        let button = harness.get_by_role_and_label(Role::Button, &words).rect();
        assert!(button.right() <= inner_edge + 0.5, "{words}: {button:?}");
    }
    render(&mut harness, "picked-narrowest");
}
