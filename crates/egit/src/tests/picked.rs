//! Several commits picked together and signed: a range by Shift-click, one more added by
//! Ctrl-click and taken away again; the details list them; their menu signs them (each made
//! again and signed, the commit after them too, the one before them left alone); Undo puts the
//! branch back.

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
