//! What egit tells of on its own: a background fetch bringing the current branch's upstream
//! new commits, shown until pulled or dismissed; one failing, its warning gone as soon as a
//! fetch of the user's own goes through, and none for one a task of the user's stopped; and how
//! tasks end, under the commit table (nothing above it moving), one that went well going by
//! itself after a few seconds, one that stopped staying.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use crate::theme::icon;

use super::screens::{
    click_when_enabled, colleague_pushes, harness, history_shown, open_path, profile_loaded,
    render, settle, shared_remote, uncommitted, wait_for_label,
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

#[test]
fn a_background_fetch_s_trouble_goes_once_a_fetch_of_your_own_goes_through() {
    let sandbox = Sandbox::new().unwrap();
    let (_, mine) = shared_remote(&sandbox);
    let origin = sandbox.path("origin.git");
    // Out of reach, as a remote whose key nobody has unlocked yet is for a background fetch.
    sandbox
        .run(
            &mine,
            "remote",
            &["set-url", "origin", "/nowhere/origin.git"],
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &mine);
    settle(&mut harness, "the history", history_shown);
    let fetched = |count: u64| {
        move |app: &mut crate::app::EgitApp| app.state.repos[0].tasks.background_fetches >= count
    };
    harness.state_mut().state.repos[0]
        .tasks
        .fetch_in_background_now();
    settle(&mut harness, "a background fetch failing", fetched(1));
    assert!(
        harness.state().state.repos[0]
            .tasks
            .background_problem
            .is_some()
    );

    // Within reach again (a passphrase typed, say): the user's own fetch goes through, and the
    // warning goes with it, not five minutes later.
    sandbox
        .run(
            &mine,
            "remote",
            &["set-url", "origin", origin.to_str().unwrap()],
        )
        .unwrap();
    click_when_enabled(&mut harness, &format!("{}  Fetch", icon::ARROWS_CLOCKWISE));
    wait_for_label(&mut harness, "Fetched.");
    assert!(
        harness.state().state.repos[0]
            .tasks
            .background_problem
            .is_none()
    );

    // A background fetch a task of the user's stops (a remote answering only after a long
    // pause, git's `ext::` transport running a command as it) is no trouble.
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.ext.allow", "always"],
        )
        .unwrap();
    let slow = format!(
        "ext::sh -c sleep% 60;% exec% {}% upload-pack% {}",
        sandbox.git().program().display(),
        origin.display()
    );
    sandbox
        .run(&mine, "remote", &["add", "slow", &slow])
        .unwrap();
    harness.state_mut().state.repos[0]
        .tasks
        .fetch_in_background_now();
    settle(&mut harness, "a background fetch under way", |app| {
        app.state.repos[0].tasks.is_fetching_in_background()
    });
    let head = sandbox.run(&mine, "rev-parse", &["HEAD"]).unwrap();
    let at = egit_git::ObjectId::from_hex(head.stdout.trim_ascii()).unwrap();
    let id = harness.state().state.repos[0].id;
    harness
        .state_mut()
        .state
        .actions
        .push(crate::app::Action::Task(
            id,
            crate::tasks::Task::CreateBranch {
                name: "topic".into(),
                at,
            },
        ));
    settle(&mut harness, "the background fetch stopped", |app| {
        !app.state.repos[0].tasks.is_fetching_in_background()
    });
    harness.run_steps(2);
    let tasks = &harness.state().state.repos[0].tasks;
    assert!(tasks.background_problem.is_none());
    assert_eq!(tasks.background_fetches, 1);
}

#[test]
fn outcomes_show_under_the_table_and_those_that_went_well_go_by_themselves() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "g", "x\n", "Add g").unwrap();
    sandbox.commit_file(&repo, "g", "z\n", "Change g").unwrap();
    sandbox
        .commit_file(&repo, "g", "w\n", "Change g again")
        .unwrap();
    std::fs::write(repo.join("notes.tmp"), "x\n").unwrap();
    let mut harness = harness(&sandbox);
    crate::views::set_outcome_lasts(&harness.ctx, Some(crate::views::OUTCOME_LASTS));
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    let title = |harness: &egui_kittest::Harness<'_, crate::app::EgitApp>| {
        harness.get_by_label("Message").rect()
    };
    let before = title(&harness);

    // Done: said under the table, the table's head where it was; gone five seconds later.
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "notes.tmp");
    harness
        .get_by_role_and_label(Role::Button, "notes.tmp")
        .click_secondary();
    wait_for_label(&mut harness, "Ignore");
    harness.get_by_label("Ignore").click();
    wait_for_label(&mut harness, "This file");
    harness
        .get_all_by_label("This file")
        .next()
        .unwrap()
        .click();
    let said = "Ignored notes.tmp in .gitignore: commit it to share that.";
    wait_for_label(&mut harness, said);
    // The pointer away, so it doesn't hold the outcome.
    harness.hover_at(egui::pos2(4.0, 4.0));
    let outcome = harness.get_by_label(said).rect();
    let row = harness
        .get_by_role_and_label(Role::Button, "Change g again")
        .rect();
    assert!(outcome.top() > row.bottom(), "{outcome:?} under {row:?}");
    assert_eq!(title(&harness), before, "nothing above the table moved");
    harness.run_steps(24);
    assert!(harness.query_by_label(said).is_none(), "gone by itself");

    // Stopped short: it stays.
    harness
        .get_by_role_and_label(Role::Button, "Change g")
        .click_secondary();
    wait_for_label(&mut harness, "Revert in main");
    harness.get_by_label("Revert in main").click();
    wait_for_label(&mut harness, "Revert and resolve the conflicts");
    harness.hover_at(egui::pos2(4.0, 4.0));
    harness.run_steps(24);
    assert!(
        harness
            .query_by_label("Revert and resolve the conflicts")
            .is_some()
    );
    assert_eq!(title(&harness), before);
    render(&mut harness, "outcome-under-the-table");
}
