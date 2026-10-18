//! Commits picked together, exported and compared, driven as a user drives them: two picked
//! apart (one left out between them), copied as separate patches and as a changelog, saved as
//! one patch and as separate patches (the dialogs answered by a folder); then compared: the
//! files changed from one to the other, a file's diff opened, swapped, closed.

use std::path::Path;

use egit_git::testing::Sandbox;
use egui::Modifiers;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::patches::wait_for_text;
use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};
use crate::app::EgitApp;
use crate::desktop::FileDialogs;
use crate::tasks::Outcome;

/// The text the last export copied.
fn copied(harness: &mut Harness<'_, EgitApp>) -> String {
    match harness.state_mut().state.repos[0].tasks.result.ok_ref() {
        Some(Outcome::Exported {
            copied: Some(text), ..
        }) => text.to_string(),
        other => panic!("nothing copied: {other:?}"),
    }
}

/// Right-clicks the row of `subject`, then picks `item` in its menu (under `submenu`, if
/// given).
fn picked_menu(
    harness: &mut Harness<'_, EgitApp>,
    (subject, submenu): (&str, Option<&str>),
    item: &str,
) {
    harness
        .get_by_role_and_label(Role::Button, subject)
        .click_secondary();
    if let Some(submenu) = submenu {
        wait_for_label(harness, submenu);
        harness.get_by_label(submenu).hover();
    }
    wait_for_label(harness, item);
    harness.get_by_label(item).click();
}

fn short(sandbox: &Sandbox, repo: &Path, name: &str) -> String {
    let out = sandbox
        .run(repo, "rev-parse", &["--short=8", name])
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn picked_commits_are_exported_and_compared() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    for (file, contents, subject) in [
        ("a", "one\n", "One"),
        ("b", "two\n", "Two"),
        ("a", "ONE\n", "Three"),
        ("c", "four\n", "Four"),
    ] {
        sandbox.commit_file(&repo, file, contents, subject).unwrap();
    }
    let out = sandbox.path("out");
    std::fs::create_dir(&out).unwrap();
    let mut harness = harness(&sandbox);
    harness.state_mut().state.files = FileDialogs::in_folder(out.clone());
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Two and Four, Three left out between them.
    harness.get_by_role_and_label(Role::Button, "Two").click();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Four")
        .click_modifiers(Modifiers::COMMAND);
    wait_for_label(&mut harness, "2 commits selected");

    // Separate patches, copied: the oldest first.
    picked_menu(
        &mut harness,
        ("Four", Some("As separate patches")),
        "Copy to the clipboard",
    );
    wait_for_label(&mut harness, "Copied 2 commits as patches.");
    let patches = copied(&mut harness);
    let first = patches.find("Subject: [PATCH 1/2] Two").unwrap();
    assert!(first < patches.find("Subject: [PATCH 2/2] Four").unwrap());

    // A changelog, copied.
    picked_menu(&mut harness, ("Four", None), "Copy as a changelog");
    wait_for_label(&mut harness, "Copied a changelog of 2 commits.");
    let (two, four) = (
        short(&sandbox, &repo, "HEAD~2"),
        short(&sandbox, &repo, "HEAD"),
    );
    assert_eq!(
        copied(&mut harness),
        format!("- Two ({two})\n- Four ({four})\n")
    );

    // One patch of both, saved: Three's change to `a` isn't in it.
    picked_menu(
        &mut harness,
        ("Four", Some("As a single patch")),
        "Save to a file…",
    );
    wait_for_text(&mut harness, "Saved the changes of 2 commits as one patch");
    let combined = std::fs::read_to_string(out.join("changes-of-2-commits.patch")).unwrap();
    assert!(combined.starts_with("The changes of 2 commits together"));
    assert!(combined.contains("+++ b/b") && combined.contains("+++ b/c"));
    assert!(!combined.contains("ONE"));

    // Separate patches, saved into a folder.
    picked_menu(
        &mut harness,
        ("Four", Some("As separate patches")),
        "Save to a folder…",
    );
    wait_for_text(&mut harness, "Saved 2 patches to");
    assert!(out.join("0001-Two.patch").exists() && out.join("0002-Four.patch").exists());
    render(&mut harness, "commits-exported");

    // Compared: what changed from Two to Four (Three's `a` among it), a file's diff opened.
    harness.get_by_label("Compare these two").click();
    wait_for_label(&mut harness, "Comparing two commits");
    wait_for_label(&mut harness, "2 files changed");
    harness.get_by_role_and_label(Role::Button, "c").click();
    settle(&mut harness, "the file's diff", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("c".to_owned(), true))
    });
    render(&mut harness, "commits-compared");
    harness.get_by_label("Swap them").click();
    wait_for_label(&mut harness, "2 files changed");
    harness.get_by_label("Close the comparison").click();
    wait_for_label(&mut harness, "2 commits selected");
}
