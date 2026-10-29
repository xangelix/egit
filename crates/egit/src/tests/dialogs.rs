//! The dialogs answered from the keyboard: Enter takes the dialog's main button (a name typed
//! then Enter makes the branch; a deletion confirmed, then confirmed again where it isn't
//! merged), but not in a field of several lines, where it writes a new line; Escape closes it,
//! nothing done.

use egit_git::testing::Sandbox;
use egui::Key;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, menu_item, open_path, profile_loaded, settle, two_branches, type_into,
    wait_for_label,
};
use crate::theme::icon;

fn badge(name: &str) -> String {
    format!("{} {name}", icon::GIT_BRANCH)
}

#[test]
fn enter_answers_a_dialog_and_escape_closes_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let rev = |name: &str| {
        sandbox
            .run(&repo, "rev-parse", &["--verify", "--quiet", name])
            .is_ok_and(|output| output.status.success())
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // A branch named, then Enter: made, as Create makes it.
    harness
        .get_by_role_and_label(Role::Button, "Other")
        .click_secondary();
    wait_for_label(&mut harness, "Create a branch here…");
    harness.get_by_label("Create a branch here…").click();
    wait_for_label(&mut harness, "Switch to it");
    harness.get_by_label("Switch to it").click();
    type_into(&mut harness, Role::TextInput, "Name", "topic");
    harness.key_press(Key::Enter);
    wait_for_label(&mut harness, "Made topic.");
    assert!(rev("refs/heads/topic"));

    // Enter in a tag's message is a new line, not an answer; Escape closes the dialog, nothing
    // made.
    harness
        .get_by_role_and_label(Role::Button, "One")
        .click_secondary();
    wait_for_label(&mut harness, "Create a tag here…");
    harness.get_by_label("Create a tag here…").click();
    wait_for_label(&mut harness, "Create tag");
    type_into(&mut harness, Role::TextInput, "Name", "v1");
    let message = "Message (makes it an annotated tag, with its author and date)";
    type_into(&mut harness, Role::MultilineTextInput, message, "First");
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    harness.get_by_label("Create tag");
    let typed = harness
        .get_by_role_and_label(Role::MultilineTextInput, message)
        .value()
        .unwrap_or_default();
    assert_eq!(typed, "First\n");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label("Create tag").is_none());
    assert!(!rev("refs/tags/v1"));

    // A deletion confirmed by Enter, and again where the branch isn't merged.
    menu_item(&mut harness, &badge("other"), "Delete other…");
    wait_for_label(&mut harness, "Delete other?");
    harness.key_press(Key::Enter);
    wait_for_label(&mut harness, "Delete anyway");
    harness.key_press(Key::Enter);
    wait_for_label(&mut harness, "Deleted other.");
    assert!(!rev("refs/heads/other"));
}
