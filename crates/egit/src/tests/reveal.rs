//! Files and folders handed to the desktop, driven as a user drives it (the opener records
//! them, starting nothing): a work tree file opened and shown in its folder from its row's
//! menu, opened from its file view's bar, and the repository's folder from its menu.

use std::path::PathBuf;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, settle, uncommitted, wait_for_label,
};
use crate::app::EgitApp;
use crate::desktop::Reveal;

/// What the opener was handed last.
fn last(harness: &Harness<'_, EgitApp>) -> Option<(Reveal, PathBuf)> {
    harness.state().state.opener.revealed().last().cloned()
}

/// Right-clicks the work list's row of `path`, then picks `item` in its menu.
fn row_menu(harness: &mut Harness<'_, EgitApp>, path: &str, item: &str) {
    harness
        .get_by_role_and_label(Role::Button, path)
        .click_secondary();
    wait_for_label(harness, item);
    harness.get_by_label(item).click();
    harness.run_steps(2);
}

#[test]
fn files_and_folders_are_opened_or_shown_in_their_folder() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    sandbox
        .commit_file(&repo, "src/main.rs", "fn main() {}\n", "Start")
        .unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() { }\n").unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    let work_tree = harness.state().state.repos[0]
        .repository
        .work_tree()
        .to_owned();
    let file = work_tree.join("src").join("main.rs");
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "src/main.rs");

    row_menu(&mut harness, "src/main.rs", "Open file");
    assert_eq!(last(&harness), Some((Reveal::Open, file.clone())));
    row_menu(&mut harness, "src/main.rs", "Show in folder");
    assert_eq!(last(&harness), Some((Reveal::InFolder, file.clone())));

    // From the file's view, its bar.
    harness
        .get_by_role_and_label(Role::Button, "src/main.rs")
        .click();
    wait_for_label(&mut harness, "Blame");
    harness.get_by_label("Open file").click();
    harness.run_steps(2);
    assert_eq!(last(&harness), Some((Reveal::Open, file)));

    // The repository's folder, from its menu.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    harness.get_by_label("Repository").click();
    wait_for_label(&mut harness, "Open the folder");
    harness.get_by_label("Open the folder").click();
    harness.run_steps(2);
    assert_eq!(last(&harness), Some((Reveal::Open, work_tree)));
}
