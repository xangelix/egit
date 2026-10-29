//! Several commits picked together and signed: a range by Shift-click, one more added by
//! Ctrl-click and taken away again; the details list them; their menu signs them (each made
//! again and signed, the commit after them too, the one before them left alone); Undo puts the
//! branch back. Two picked on another branch cherry-picked onto the current one, oldest first,
//! then reverted together.

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
