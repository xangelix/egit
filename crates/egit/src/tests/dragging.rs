//! A branch dragged onto another: what it can do asked where it lands, then done (merging a
//! branch into the current one, rebasing the current one onto another), or nothing when the
//! menu is dismissed.

use egit_git::testing::Sandbox;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, two_branches, wait_for_label,
};
use crate::app::EgitApp;
use crate::theme::icon;

fn badge(name: &str) -> String {
    format!("{} {name}", icon::GIT_BRANCH)
}

/// Drags the badge `from` onto the badge `onto`, a few frames of movement between.
fn drag(harness: &mut Harness<'_, EgitApp>, from: &str, onto: &str) {
    let start = harness.get_by_label(from).rect().center();
    let end = harness.get_by_label(onto).rect().center();
    harness.hover_at(start);
    harness.step();
    harness.drag_at(start);
    harness.step();
    for step in 1..=4_u8 {
        let along = f32::from(step) / 4.0;
        harness.hover_at(start.lerp(end, along));
        harness.step();
    }
    harness.drop_at(end);
    harness.run_steps(2);
}

#[test]
fn a_branch_dropped_on_another_merges_or_rebases() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Dismissed: nothing done.
    drag(&mut harness, &badge("other"), &badge("main"));
    wait_for_label(&mut harness, "Rebase main onto other");
    render(&mut harness, "branch-dropped");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label("Rebase main onto other").is_none());

    // Onto the current branch: merged into it.
    drag(&mut harness, &badge("other"), &badge("main"));
    wait_for_label(&mut harness, "Merge other into main");
    harness.get_by_label("Merge other into main").click();
    wait_for_label(&mut harness, "Merged other.");
    let parents = sandbox.run(&repo, "log", &["-1", "--format=%P"]).unwrap();
    assert_eq!(parents.stdout.split(|&byte| byte == b' ').count(), 2);
}

#[test]
fn the_current_branch_dropped_on_another_rebases_onto_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    drag(&mut harness, &badge("main"), &badge("other"));
    wait_for_label(&mut harness, "Rebase main onto other, keeping merges");
    harness.get_by_label("Rebase main onto other").click();
    wait_for_label(&mut harness, "Rebased onto other.");
    let base = sandbox
        .run(&repo, "merge-base", &["main", "other"])
        .unwrap()
        .stdout;
    let other = sandbox.run(&repo, "rev-parse", &["other"]).unwrap().stdout;
    assert_eq!(base, other, "main now sits on other");
}

#[test]
fn sidebar_rows_drag_and_drop_alike() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // From the sidebar's row onto the graph's badge.
    drag(&mut harness, "other", &badge("main"));
    wait_for_label(&mut harness, "Merge other into main");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    // From the graph's badge onto the sidebar's row.
    drag(&mut harness, &badge("main"), "other");
    wait_for_label(&mut harness, "Rebase main onto other");
}
