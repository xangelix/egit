//! Stashes in the graph: each a row of its own on the commit it was
//! made on, where its time puts it among the commits (a "stash@{n}" badge, its words and the
//! branch it was made on, a dashed square for its node), counted apart from the commits. Its
//! details (what it is, what it was made on) and its menu apply, pop, or drop it; one dropped
//! from the middle of the list, which moves no ref, leaves the graph too.

use std::path::{Path, PathBuf};

use egit_git::Wait as _;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, menu_item, open_path, profile_loaded, render, settle, wait_for_label,
};
use crate::app::EgitApp;

/// Runs `git <subcommand> <args>` in `repo` at `seconds` past the epoch.
fn at(sandbox: &Sandbox, repo: &Path, seconds: u32, subcommand: &'static str, args: &[&str]) {
    let date = format!("@{seconds} +0000");
    sandbox
        .git_with([("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)])
        .unwrap()
        .write(subcommand)
        .args(args)
        .current_dir(repo)
        .output()
        .wait()
        .unwrap();
}

/// The rows the history shows, by their names (a stash's "stash@{n}: …").
fn rows(app: &EgitApp) -> Vec<String> {
    let history = &app.state.repos[0].history;
    (0..history.shown.rows())
        .filter_map(|row| history.shown.id(row))
        .map(|id| {
            let subject = history
                .summaries
                .peek(&id)
                .map_or_else(String::new, |summary| summary.subject.to_string());
            history
                .shown
                .stashes()
                .index_of(&id)
                .map_or(subject, |index| format!("stash@{{{index}}}"))
        })
        .collect()
}

/// Two commits and three stashes: "Half a parser" made on the first, then "Try lexing" and "A
/// third" on the second, each at a time of its own.
fn stashed(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    let commit = |seconds, file: &str, message: &str| {
        std::fs::write(repo.join(file), message).unwrap();
        at(sandbox, &repo, seconds, "add", &["--", file]);
        at(
            sandbox,
            &repo,
            seconds,
            "commit",
            &["--quiet", "-m", message],
        );
    };
    let stash = |seconds, file: &str, message: &str| {
        std::fs::write(repo.join(file), message.to_lowercase()).unwrap();
        at(
            sandbox,
            &repo,
            seconds,
            "stash",
            &["push", "--quiet", "-m", message],
        );
    };
    commit(1_000, "lexer.rs", "Add the lexer");
    stash(1_500, "lexer.rs", "Half a parser");
    commit(2_000, "parser.rs", "Add the parser");
    stash(3_000, "parser.rs", "Try lexing");
    stash(4_000, "lexer.rs", "A third");
    repo
}

#[test]
fn stashes_show_in_the_graph_and_are_applied_popped_and_dropped_from_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = stashed(&sandbox);

    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Two commits, three stashes, each where its time puts it, above what it was made on; the
    // count says commits.
    wait_for_label(&mut harness, "Stats: 2 commits");
    wait_for_label(&mut harness, "stash@{0}: A third");
    wait_for_label(&mut harness, "stash@{1}: Try lexing");
    wait_for_label(&mut harness, "stash@{2}: Half a parser");
    settle(&mut harness, "the rows in order", |app| {
        rows(app)
            == [
                "stash@{0}",
                "stash@{1}",
                "Add the parser",
                "stash@{2}",
                "Add the lexer",
            ]
    });
    render(&mut harness, "stashes-graph");

    // Selected, it shows its changes beside the history, as a commit does.
    harness.get_by_label("stash@{2}: Half a parser").click();
    wait_for_label(&mut harness, "lexer.rs");
    render(&mut harness, "stashes-selected");

    // Applied from its details: its change is back in the work tree, the stash kept.
    harness.get_by_label("stash@{1}: Try lexing").click();
    wait_for_label(&mut harness, "parser.rs");
    harness.get_by_label("Apply").click();
    settle(&mut harness, "the stash applied", |_| {
        std::fs::read_to_string(repo.join("parser.rs")).unwrap() == "try lexing"
    });
    wait_for_label(&mut harness, "stash@{1}: Try lexing");
    at(&sandbox, &repo, 5_000, "restore", &["--", "parser.rs"]);
    // The applied change's uncommitted row gone, so the rows aimed at below don't move.
    settle(&mut harness, "the work tree clean", |app| {
        app.state.repos[0]
            .work_tree
            .status()
            .and_then(Result::ok)
            .is_some_and(|status| status.entries().next().is_none())
    });
    harness.run_steps(2);
    // A commit selected: the menus below are the only ones offering what they do.
    harness
        .get_by_role_and_label(Role::Button, "Add the parser")
        .click();
    // Its details, not a stash's (whose own Pop and Drop… would show beside the menus').
    wait_for_label(&mut harness, "Parent");
    harness.run_steps(2);

    // Dropped from the middle of the list (no ref moves): gone from the graph too, the others
    // numbered anew.
    menu_item(&mut harness, "stash@{1}: Try lexing", "Drop…");
    wait_for_label(&mut harness, "Drop stash@{1}?");
    harness.get_by_label("Drop").click();
    settle(&mut harness, "the stash dropped", |app| {
        rows(app) == ["stash@{0}", "Add the parser", "stash@{1}", "Add the lexer"]
    });
    wait_for_label(&mut harness, "stash@{1}: Half a parser");

    // Popped: applied and gone.
    menu_item(&mut harness, "stash@{0}: A third", "Pop");
    settle(&mut harness, "the stash popped", |app| {
        rows(app) == ["Add the parser", "stash@{0}", "Add the lexer"]
    });
    assert_eq!(
        std::fs::read_to_string(repo.join("lexer.rs")).unwrap(),
        "a third"
    );
    wait_for_label(&mut harness, "Stats: 2 commits");
}
