//! A narrow window (a phone's screen): one thing at a time, each the whole width. A commit
//! tapped shows its details in the history's place, a file there shows its diff, and each goes
//! back; the sidebar is a page of its own, a branch tapped there going to its commit; the work
//! in progress's row shows the changes and the commit form; the agents take the whole tab.
//! Home's cards fit it too. A pull request's badge shows its details, going back to the list;
//! a pipeline's mark its page, a job there its log, and the top bar every pipeline.

use egit_forge::ForgeKind;
use egit_forge::testing::FakeForge;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::pipelines::{TOKEN, pipeline};
use super::screens::{
    commit_of, forge_clone, history_of, history_shown, into_view, open_path, phone, profile_loaded,
    pull_requests_read, render, settle, two_branches, wait_for_label,
};
use crate::theme::icon;

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

    // The agents: the whole tab, until hidden.
    harness.get_by_label("Agents").click();
    wait_for_label(&mut harness, "Hide the agents");
    render(&mut harness, "narrow-agents");
    harness.get_by_label("Hide the agents").click();
    wait_for_label(&mut harness, "Show the sidebar");

    // Home (the tab bar's "+"), the repository among the recent ones; back to it from the tab
    // bar's menu of every tab.
    harness.get_by_label("Open or clone a repository").click();
    wait_for_label(&mut harness, "Recent repositories");
    render(&mut harness, "narrow-home-recent");
    harness.get_by_role_and_label(Role::Button, "Home").click();
    wait_for_label(&mut harness, "repo");
    render(&mut harness, "narrow-tabs");
    // The menu's item (under the tab bar), not the recent repositories' row of the same name.
    harness
        .query_all_by_role_and_label(Role::Button, "repo")
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    wait_for_label(&mut harness, "Show the sidebar");
}

#[test]
fn a_narrow_window_shows_a_request_then_the_list() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitHub, "t0ken", "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "octo/tool", "t0ken");
    let mut harness = phone(&sandbox, egui::Theme::Light);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the pull requests", pull_requests_read(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Its badge tapped: the request's details, the whole width.
    let badge = format!("{} #8", icon::GIT_PULL_REQUEST);
    wait_for_label(&mut harness, &badge);
    harness.get_by_label(&badge).click();
    wait_for_label(&mut harness, "Adds the topic.");
    render(&mut harness, "narrow-pull-request");

    // Back to the list of them, then another's details.
    harness
        .get_by_label(&format!("{}  Pull requests", icon::ARROW_LEFT))
        .click();
    wait_for_label(&mut harness, "Fork change");
    assert!(harness.query_by_label("Adds the topic.").is_none());
    render(&mut harness, "narrow-pull-requests");
    harness
        .get_by_role_and_label(Role::Button, "Fork change")
        .click();
    wait_for_label(
        &mut harness,
        "cy wants to merge feature into main, from cy/tool",
    );
}

#[test]
fn a_narrow_window_shows_pipelines_one_page_at_a_time() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, topic, _fork) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    fake.state().repositories[0].pipelines = vec![
        pipeline(9_003, &head, "RUNNING", "RUNNING"),
        pipeline(9_002, &topic, "FAILED", "FAILED"),
        pipeline(9_001, &head, "SUCCESS", "SUCCESS"),
    ];
    let mut harness = phone(&sandbox, egui::Theme::Dark);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, "Pipeline #9003: running");
    render(&mut harness, "narrow-pipeline-marks");

    // A mark tapped: its pipeline, then a job's log, each the whole width; Escape back.
    harness
        .get_by_role_and_label(Role::Button, "Pipeline #9002: failed")
        .click();
    wait_for_label(&mut harness, "unit: failed");
    render(&mut harness, "narrow-pipeline");
    harness
        .get_by_role_and_label(Role::Button, "lint: failed")
        .click();
    wait_for_label(
        &mut harness,
        "This job has no log (it may have been erased).",
    );
    render(&mut harness, "narrow-job");
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "unit: failed");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);

    // Every pipeline, from the top bar's second row.
    harness.get_by_label("Pipelines").click();
    wait_for_label(&mut harness, "Pipeline #9001: passed");
    render(&mut harness, "narrow-pipelines");
}

#[test]
fn a_narrow_window_s_menus_dialogs_and_pages_fit_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = phone(&sandbox, egui::Theme::Dark);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // A commit's menu (a long press), and a dialog from it; Escape closes it.
    harness
        .get_by_role_and_label(Role::Button, "Two")
        .click_secondary();
    wait_for_label(&mut harness, "Create a branch here…");
    render(&mut harness, "narrow-menu");
    harness.get_by_label("Create a branch here…").click();
    wait_for_label(&mut harness, "Switch to it");
    render(&mut harness, "narrow-dialog");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);

    // Every command, from the top bar's last button: the Overview from it.
    harness.get_by_label("Every command").click();
    wait_for_label(&mut harness, "Command, branch, or repository");
    harness
        .get_by_role_and_label(Role::TextInput, "Command, branch, or repository")
        .type_text("overview");
    harness.run_steps(2);
    render(&mut harness, "narrow-palette");
    harness.key_press(egui::Key::Enter);
    wait_for_label(&mut harness, "Where it lives");
    render(&mut harness, "narrow-overview");

    // Preferences, and the profile.
    harness.get_by_label("Preferences").click();
    wait_for_label(&mut harness, "Keyboard shortcuts");
    render(&mut harness, "narrow-preferences");
    into_view(&mut harness, "Keyboard shortcuts");
    render(&mut harness, "narrow-shortcuts");
    harness.get_by_label("Open or clone a repository").click();
    wait_for_label(
        &mut harness,
        &format!("{}  Edit your profile", icon::USER_CIRCLE),
    );
    harness
        .get_by_label(&format!("{}  Edit your profile", icon::USER_CIRCLE))
        .click();
    wait_for_label(&mut harness, "Every setting");
    render(&mut harness, "narrow-profile");
}
