//! Conflicts foreseen in the menus, driven as a user drives them: a commit's menu says which of
//! merging, rebasing onto, cherry-picking, and reverting it would conflict, and where (on
//! hover), before anything is done; nothing moves meanwhile.

use std::time::Duration;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};
use crate::app::EgitApp;

/// Hovers the menu's item `item` (still: a tooltip waits for the pointer to rest) until what
/// it foresees is said, starting so: `said`.
fn foreseen(harness: &mut Harness<'_, EgitApp>, item: &str, said: &str) {
    harness.get_by_label(item).hover();
    for _ in 0..2_000 {
        harness.step();
        if harness.query_all_by_label_contains(said).next().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{item:?} never said {said:?}");
}

#[test]
fn a_commits_menu_foresees_conflicts() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nc\n", "Base")
        .unwrap();
    sandbox.commit_file(&repo, "g", "x\n", "Add g").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "f", "A\nb\nc\n", "Side f")
        .unwrap();
    sandbox.commit_file(&repo, "g", "y\n", "Side g").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nC\n", "Main f")
        .unwrap();
    sandbox.commit_file(&repo, "g", "z\n", "Main g").unwrap();
    let head = sandbox.run(&repo, "rev-parse", &["HEAD"]).unwrap().stdout;
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // The side branch's last commit: everything bringing it in meets main's change to g.
    wait_for_label(&mut harness, "Side g");
    harness
        .get_by_role_and_label(Role::Button, "Side g")
        .click_secondary();
    wait_for_label(&mut harness, "Merge into main");
    let conflicts = "Would stop at conflicts in g";
    foreseen(&mut harness, "Merge into main", conflicts);
    foreseen(&mut harness, "Cherry-pick onto main", conflicts);
    foreseen(
        &mut harness,
        "Rebase main onto this commit",
        "Would stop at",
    );
    render(&mut harness, "foreseen-conflicts");

    // The one before: its change to f doesn't meet main's.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Side f")
        .click_secondary();
    wait_for_label(&mut harness, "Cherry-pick onto main");
    let clean = "No conflicts (worked out without touching anything)";
    foreseen(&mut harness, "Cherry-pick onto main", clean);
    // main's own commit: reverting the latest always goes through.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Main g")
        .click_secondary();
    wait_for_label(&mut harness, "Revert in main");
    foreseen(&mut harness, "Revert in main", clean);

    // Nothing moved, nothing left behind.
    let after = sandbox.run(&repo, "rev-parse", &["HEAD"]).unwrap().stdout;
    assert_eq!(after, head);
    let status = sandbox
        .run(&repo, "status", &["--porcelain"])
        .unwrap()
        .stdout;
    assert!(status.is_empty(), "{}", String::from_utf8_lossy(&status));
}
