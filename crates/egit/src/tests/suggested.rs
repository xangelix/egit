//! Repositories suggested on Home: two opened from one folder, the others there are suggested,
//! the most recently active first (by their reflogs' dates: commits made at set times), one
//! without a commit last; a plain folder, a file, and a repository in a folder holding only one
//! opened left out. One suggested opens with a double click.

use std::path::{Path, PathBuf};

use egit_git::testing::Sandbox;
use egui_kittest::kittest::Queryable as _;

use super::screens::{harness, profile_loaded, render, settle};
use crate::app::{Action, EgitApp};
use crate::settings::Location;
use crate::theme::icon;

/// A repository at `name` under the sandbox with a commit made at `date`.
fn dated(sandbox: &Sandbox, name: &str, date: &str) -> PathBuf {
    let repo = sandbox.init(name).unwrap();
    std::fs::write(repo.join("a"), "a\n").unwrap();
    sandbox.run(&repo, "add", &["--", "a"]).unwrap();
    let git = sandbox
        .git_with([("GIT_COMMITTER_DATE", date), ("GIT_AUTHOR_DATE", date)])
        .unwrap();
    git.write("commit")
        .args(["--quiet", "--message", "Start"])
        .current_dir(&repo)
        .output()
        .unwrap();
    repo
}

/// Goes back to Home.
fn home(harness: &mut egui_kittest::Harness<'_, EgitApp>) {
    harness
        .get_by_label(&format!("{}  Home", icon::HOUSE))
        .click();
    harness.run_steps(2);
}

/// The repositories suggested, by name, once worked out.
fn suggested(app: &mut EgitApp) -> Option<Vec<String>> {
    app.state.home.suggested_names()
}

#[test]
fn repositories_beside_two_opened_are_suggested_the_most_recently_active_first() {
    let sandbox = Sandbox::new().unwrap();
    let a = dated(&sandbox, "work/a", "2024-01-01T10:00:00Z");
    let b = dated(&sandbox, "work/b", "2024-01-02T10:00:00Z");
    dated(&sandbox, "work/older", "2024-03-01T10:00:00Z");
    dated(&sandbox, "work/newer", "2024-05-01T10:00:00Z");
    sandbox.init("work/empty").unwrap();
    std::fs::create_dir_all(sandbox.path("work/notes")).unwrap();
    std::fs::write(sandbox.path("work/todo.txt"), "x\n").unwrap();
    // Alone in its folder with one opened: nothing suggested from there.
    let lone = dated(&sandbox, "play/lone", "2024-06-01T10:00:00Z");
    dated(&sandbox, "play/other", "2024-06-02T10:00:00Z");

    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    for (opened, repo) in [&a, &b, &lone].into_iter().enumerate() {
        harness
            .state_mut()
            .state
            .actions
            .push(Action::Open(Location::here(repo.clone())));
        settle(&mut harness, "the repository", move |app| {
            app.state.repos.len() > opened
        });
    }
    home(&mut harness);
    settle(&mut harness, "the suggestions", |app| {
        suggested(app).is_some_and(|names| names.len() == 3)
    });
    assert_eq!(
        suggested(harness.state_mut()).unwrap(),
        ["newer", "older", "empty"]
    );
    harness.get_by_label("Suggested repositories");
    render(&mut harness, "home-suggested");

    // A double click opens one.
    let newer: &Path = &sandbox.path("work/newer");
    let cell = harness
        .query_all_by_label("newer")
        .next()
        .unwrap()
        .rect()
        .center();
    harness.hover_at(cell);
    harness.step();
    for pressed in [true, false, true, false] {
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos: cell,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
    settle(&mut harness, "the suggestion opened", |app| {
        app.state
            .repos
            .iter()
            .any(|repo| repo.repository.work_tree() == newer)
    });
}
