//! Long text typed or edited stays in its field: a commit's 200-line message edited in place
//! scrolls inside the field, its Save button still on screen; the commit form's description
//! alike. And a branch's long name leaves the details their width: at their narrowest, its
//! badge is cut, and every file's numbers and the commit's totals show whole.

use std::fmt::Write as _;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, harness, history_shown, open_path, profile_loaded, render, settle,
    uncommitted, wait_for_label, windowed,
};
use crate::theme::icon;

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
    let commit = harness.get_by_role_and_label(Role::Button, "Commit");
    assert!(
        commit.rect().bottom() < WINDOW_BOTTOM,
        "{:?}",
        commit.rect()
    );
}

#[test]
fn a_long_branch_name_leaves_the_files_numbers_whole() {
    const SUBJECT: &str = "build: a commit with a long enough subject to wrap";
    const BRANCH: &str = "chore/a-branch-with-a-name-much-too-long-for-the-panel";
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    std::fs::write(repo.join("big"), "before\n".repeat(110_000)).unwrap();
    sandbox.run(&repo, "add", &["big"]).unwrap();
    sandbox.run(&repo, "commit", &["-m", "First"]).unwrap();
    std::fs::create_dir_all(repo.join("core/deeply/nested/folder")).unwrap();
    std::fs::write(repo.join("big"), "after\n".repeat(120_000)).unwrap();
    std::fs::write(repo.join("Cargo.lock"), "line\n".repeat(340)).unwrap();
    std::fs::write(
        repo.join("core/deeply/nested/folder/a_rather_long_file_name.rs"),
        "line\n".repeat(25),
    )
    .unwrap();
    sandbox.run(&repo, "add", &["-A"]).unwrap();
    sandbox.run(&repo, "commit", &["-m", SUBJECT]).unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", BRANCH])
        .unwrap();
    sandbox
        .run(
            &repo,
            "update-ref",
            &[&format!("refs/remotes/origin/{BRANCH}"), "HEAD"],
        )
        .unwrap();

    // Room for the details at their least: 260 points, up to the window's edge.
    let mut harness = windowed(&sandbox, [880.0, WINDOW_BOTTOM], 1.0, egui::Theme::Dark);
    let inner_edge = 880.0 - 12.0;
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness
        .get_all_by_role_and_label(Role::Button, SUBJECT)
        .next()
        .unwrap()
        .click();
    wait_for_label(&mut harness, "3 files changed");
    wait_for_label(&mut harness, "+120,000");
    let whole = |harness: &egui_kittest::Harness<'_, crate::app::EgitApp>, label: &str| {
        for node in harness.get_all_by_label(label) {
            assert!(
                node.rect().right() <= inner_edge + 0.5,
                "{label} ends at {}",
                node.rect().right()
            );
        }
    };
    let numbers = ["+120,000", "−110,000", "+340", "+25"];
    // The totals, too many to say beside the title, on a line of their own.
    let total = "+120,365";
    wait_for_label(&mut harness, total);
    let title = harness.get_by_label("3 files changed").rect();
    assert!(harness.get_by_label(total).rect().top() >= title.bottom() - 1.0);
    for label in numbers.iter().copied().chain([total]) {
        whole(&harness, label);
    }
    // The badges are cut to the line, named in full.
    for badge in [
        format!("{} {BRANCH}", icon::GIT_BRANCH),
        format!("{} origin/{BRANCH}", icon::CLOUD),
    ] {
        whole(&harness, &badge);
    }
    render(&mut harness, "details-narrowest");

    // As a tree of their folders, alike.
    harness
        .get_by_label("Show the files in a tree of their folders")
        .click();
    wait_for_label(&mut harness, "core/deeply/nested/folder");
    for label in numbers.iter().copied().chain([total]) {
        whole(&harness, label);
    }
    render(&mut harness, "details-narrowest-tree");
}
