//! Long text typed or edited stays in its field: a commit's 200-line message edited in place
//! scrolls inside the field, its Save button still on screen; the commit form's description
//! alike.

use std::fmt::Write as _;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, harness, history_shown, open_path, profile_loaded, render, settle,
    uncommitted, wait_for_label,
};

/// The window's height in the tests.
const WINDOW_BOTTOM: f32 = 800.0;

#[test]
fn a_long_message_scrolls_inside_its_field() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let body = (1..=200).fold(String::new(), |mut body, n| {
        writeln!(body, "Line {n} of why.").unwrap();
        body
    });
    std::fs::write(repo.join("f"), "x\n").unwrap();
    sandbox.run(&repo, "add", &["f"]).unwrap();
    sandbox
        .run(&repo, "commit", &["-m", "Long", "-m", &body])
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    harness.get_by_role_and_label(Role::Button, "Long").click();
    click_when_enabled(&mut harness, "Edit the message");
    wait_for_label(&mut harness, "Save message");
    // The field scrolls inside itself (its own rect is its whole text): what follows it shows.
    let save = harness.get_by_label("Save message");
    assert!(save.rect().bottom() < WINDOW_BOTTOM, "{:?}", save.rect());
    render(&mut harness, "message-edit-long");
    harness.get_by_label("Cancel").click();
    harness.run_steps(2);

    // The commit form's description, as long.
    std::fs::write(repo.join("f"), "y\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage all");
    let description = harness.get_by_role_and_label(Role::MultilineTextInput, "Description");
    description.focus();
    description.type_text(&body);
    harness.run_steps(3);
    let commit = harness.get_by_label("Commit 0 files");
    assert!(
        commit.rect().bottom() < WINDOW_BOTTOM,
        "{:?}",
        commit.rect()
    );
}
