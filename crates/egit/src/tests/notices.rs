//! What egit tells of on its own: a background fetch bringing the current branch's upstream
//! new commits, shown until pulled or dismissed.

use egit_git::testing::Sandbox;
use egui_kittest::kittest::Queryable as _;

use crate::theme::icon;

use super::screens::{
    colleague_pushes, harness, history_shown, open_path, profile_loaded, render, settle,
    shared_remote, wait_for_label,
};

#[test]
fn a_background_fetch_tells_of_new_commits_upstream() {
    let sandbox = Sandbox::new().unwrap();
    let (colleague, mine) = shared_remote(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &mine);
    settle(&mut harness, "the history", history_shown);

    // A fetch that brings nothing says nothing.
    let fetched = |count: u64| {
        move |app: &mut crate::app::EgitApp| app.state.repos[0].tasks.background_fetches >= count
    };
    harness.state_mut().state.repos[0]
        .tasks
        .fetch_in_background_now();
    settle(&mut harness, "a fetch bringing nothing", fetched(1));
    assert!(harness.state().state.repos[0].tasks.brought.is_none());

    // Two fetches, a commit each: one notice, counting both.
    for (round, file) in [(2, "one"), (3, "two")] {
        colleague_pushes(&sandbox, &colleague, file, "x\n");
        harness.state_mut().state.repos[0]
            .tasks
            .fetch_in_background_now();
        settle(&mut harness, "a fetch bringing a commit", fetched(round));
    }
    wait_for_label(
        &mut harness,
        &format!("{}  2 new commits on origin/main", icon::ARROW_DOWN),
    );
    render(&mut harness, "fetched-in-background");

    // Show them: the upstream's commit selected.
    harness.get_by_label("Show them").click();
    wait_for_label(&mut harness, "Colleague's two");

    // Dismissed, it's gone; pulling would have done as much.
    harness.get_by_label("Dismiss").click();
    harness.run_steps(2);
    assert!(harness.state().state.repos[0].tasks.brought.is_none());
}
