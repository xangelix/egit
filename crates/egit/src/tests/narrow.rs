//! A narrow window (a phone's screen): one thing at a time, each the whole width. A commit
//! tapped shows its details in the history's place, a file there shows its diff, and each goes
//! back; the sidebar is a page of its own, a branch tapped there going to its commit; the work
//! in progress's row shows the changes and the commit form. Home's cards fit it too.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use crate::theme::icon;

use super::screens::{
    history_shown, open_path, phone, profile_loaded, render, settle, two_branches, wait_for_label,
};

#[test]
fn a_narrow_window_shows_one_thing_at_a_time() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    for n in 1..=20 {
        sandbox
            .commit_file(&repo, "f", &format!("{n}\n"), &format!("Change {n} to f"))
            .unwrap();
    }
    let mut harness = phone(&sandbox, egui::Theme::Dark);
    settle(&mut harness, "the profile", profile_loaded);
    render(&mut harness, "narrow-home");
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for_label(&mut harness, "Change 20 to f");
    render(&mut harness, "narrow-history");

    // A commit tapped: its details in the history's place, a file there its diff.
    harness
        .get_by_role_and_label(Role::Button, "Change 20 to f")
        .click();
    wait_for_label(&mut harness, "1 file changed");
    render(&mut harness, "narrow-commit");
    harness.get_by_role_and_label(Role::Label, "f").click();
    wait_for_label(&mut harness, "Diff");
    render(&mut harness, "narrow-file");

    // Back to the details, then the history.
    harness.get_by_label_contains("History").click();
    wait_for_label(&mut harness, "1 file changed");
    harness.get_by_label_contains("History").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("1 file changed").is_none());

    // The sidebar, a page of its own: a branch tapped goes to its commit in the history.
    harness.get_by_label("Show the sidebar").click();
    wait_for_label(&mut harness, "Branches: 2");
    render(&mut harness, "narrow-sidebar");
    harness.get_by_role_and_label(Role::Button, "other").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Branches: 2").is_none());
    wait_for_label(&mut harness, "Other");

    // The work in progress: its changes and the commit form.
    std::fs::write(repo.join("f"), "changed\n").unwrap();
    wait_for_label(&mut harness, "1 uncommitted file");
    harness.get_by_label("1 uncommitted file").click();
    wait_for_label(&mut harness, "Not staged");
    render(&mut harness, "narrow-work");

    // Home, the repository among the recent ones: a tap opens it.
    harness
        .get_by_label(&format!("{}  Home", icon::HOUSE))
        .click();
    wait_for_label(&mut harness, "Recent repositories");
    render(&mut harness, "narrow-home-recent");
}
