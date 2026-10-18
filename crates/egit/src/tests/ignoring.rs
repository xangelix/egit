//! Ignoring files git doesn't track, driven as a user drives it: a file ignored from its row's
//! menu in `.gitignore` (made for it), taken back by Undo; then every file of an extension, in
//! `info/exclude`; a tracked file offered no such thing.

use std::path::Path;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, harness, history_shown, open_path, profile_loaded, render, settle,
    uncommitted, wait_for_label,
};
use crate::app::EgitApp;

/// The files git lists as untracked, sorted.
fn untracked(sandbox: &Sandbox, repo: &Path) -> Vec<String> {
    let out = sandbox
        .run(repo, "ls-files", &["--others", "--exclude-standard"])
        .unwrap();
    let mut listed: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    listed.sort();
    listed
}

/// Right-clicks the work list's row of `path` and opens its Ignore submenu.
fn ignore_menu(harness: &mut Harness<'_, EgitApp>, path: &str) {
    wait_for_label(harness, path);
    harness
        .get_by_role_and_label(Role::Button, path)
        .click_secondary();
    wait_for_label(harness, "Ignore");
    harness.get_by_label("Ignore").click();
    wait_for_label(harness, "This file");
}

/// Picks the `nth` item named `words` (the first under .gitignore, the second under
/// info/exclude).
fn pick(harness: &Harness<'_, EgitApp>, words: &str, nth: usize) {
    let item = harness.get_all_by_label(words).nth(nth).unwrap();
    item.click();
}

#[test]
fn untracked_files_are_ignored_from_their_menu_and_taken_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "notes.txt", "one\n", "Start")
        .unwrap();
    std::fs::write(repo.join("notes.txt"), "two\n").unwrap();
    std::fs::write(repo.join("build.log"), "x\n").unwrap();
    std::fs::create_dir_all(repo.join("logs")).unwrap();
    std::fs::write(repo.join("logs/a.log"), "x\n").unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, &uncommitted(3));
    harness.get_by_label(&uncommitted(3)).click();
    wait_for_label(&mut harness, "Not staged");

    // A tracked file's menu offers no ignoring: it would change nothing.
    wait_for_label(&mut harness, "notes.txt");
    harness
        .get_by_role_and_label(Role::Button, "notes.txt")
        .click_secondary();
    wait_for_label(&mut harness, "Copy path");
    assert!(harness.query_by_label("Ignore").is_none());
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);

    // The file alone, in a .gitignore made for it; Undo takes it away again.
    ignore_menu(&mut harness, "logs/a.log");
    wait_for_label(&mut harness, "Every .log file");
    wait_for_label(&mut harness, "Its folder, logs/");
    render(&mut harness, "ignore-menu");
    // The audit's pointer, resting on each control, closed it.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    ignore_menu(&mut harness, "logs/a.log");
    pick(&harness, "This file", 0);
    wait_for_label(
        &mut harness,
        "Ignored logs/a.log in .gitignore: commit it to share that.",
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".gitignore")).unwrap(),
        "/logs/a.log\n"
    );
    assert_eq!(untracked(&sandbox, &repo), [".gitignore", "build.log"]);
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid ignoring logs/a.log.");
    assert!(!repo.join(".gitignore").exists());
    assert_eq!(untracked(&sandbox, &repo), ["build.log", "logs/a.log"]);

    // Every .log file, here only.
    ignore_menu(&mut harness, "build.log");
    pick(&harness, "Every .log file", 1);
    wait_for_label(
        &mut harness,
        "Ignored every .log file here only (info/exclude).",
    );
    assert_eq!(untracked(&sandbox, &repo), Vec::<String>::new());
    let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert!(exclude.ends_with("\n*.log\n"), "{exclude:?}");
    wait_for_label(&mut harness, &uncommitted(1));
}
