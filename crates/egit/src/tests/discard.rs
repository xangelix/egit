//! Discarding changes not staged, driven as a user drives it: a hunk from a file's diff (nothing
//! asked) and Undo putting it back; a file from its row's menu, asked first; an untracked file
//! deleted from its menu and brought back by Undo; and everything not staged at once, from a
//! row's menu.

use std::path::Path;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, drag, harness, history_shown, open_path, profile_loaded, render, settle,
    uncommitted, wait_for_label,
};
use crate::app::EgitApp;

/// Thirty lines, those in `changed` in capitals.
fn notes(changed: &[u32]) -> String {
    (1..=30)
        .map(|line| {
            if changed.contains(&line) {
                format!("LINE {line}\n")
            } else {
                format!("line {line}\n")
            }
        })
        .collect()
}

fn read(repo: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(repo.join(file)).ok()
}

/// Right-clicks the work list's row of `path`, then picks `item` in its menu.
fn row_menu(harness: &mut Harness<'_, EgitApp>, path: &str, item: &str) {
    wait_for_label(harness, path);
    harness
        .get_by_role_and_label(Role::Button, path)
        .click_secondary();
    wait_for_label(harness, item);
    harness.get_by_label(item).click();
}

#[test]
fn changes_not_staged_are_discarded_and_brought_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "notes.txt", &notes(&[]), "Write notes")
        .unwrap();
    sandbox
        .commit_file(&repo, "other.txt", "a\n", "Other")
        .unwrap();
    std::fs::write(repo.join("notes.txt"), notes(&[5, 25])).unwrap();
    std::fs::write(repo.join("other.txt"), "b\n").unwrap();
    std::fs::write(repo.join("todo.md"), "- later\n").unwrap();
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

    // A hunk, from the diff: the second one's change goes, the first stays; nothing asked.
    harness
        .get_by_role_and_label(Role::Button, "notes.txt")
        .click();
    settle(&mut harness, "the diff", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("notes.txt".to_owned(), true))
    });
    // The diff's lines: the first hunk (a header, three of context, `-line 5`, `+LINE 5`, three
    // more), then the second's header, three of context, `-line 25`.
    drag(&mut harness, (13, 0), (13, 3));
    render(&mut harness, "discard-bar");
    harness.get_by_label("Discard hunk").click();
    wait_for_label(&mut harness, "Discarded a hunk of notes.txt.");
    assert_eq!(read(&repo, "notes.txt"), Some(notes(&[5])));

    // Undo: back as it was.
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid discarding a hunk of notes.txt.");
    assert_eq!(read(&repo, "notes.txt"), Some(notes(&[5, 25])));

    // A whole file, from its row's menu: asked first.
    harness
        .get_by_role_and_label(Role::Button, "other.txt")
        .click_secondary();
    wait_for_label(&mut harness, "Discard changes…");
    render(&mut harness, "work-row-menu");
    harness.get_by_label("Discard changes…").click();
    wait_for_label(&mut harness, "Discard the changes to other.txt?");
    render(&mut harness, "discard-file");
    harness.get_by_label("Discard").click();
    wait_for_label(&mut harness, "Discarded the changes to other.txt.");
    assert_eq!(read(&repo, "other.txt").as_deref(), Some("a\n"));

    // An untracked file is deleted, and Undo brings it back.
    row_menu(&mut harness, "todo.md", "Discard changes…");
    wait_for_label(&mut harness, "Delete todo.md?");
    harness.get_by_label("Delete").click();
    wait_for_label(&mut harness, "Deleted todo.md.");
    assert_eq!(read(&repo, "todo.md"), None);
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid discarding todo.md.");
    assert_eq!(read(&repo, "todo.md").as_deref(), Some("- later\n"));

    // Everything not staged at once, from a row's menu.
    row_menu(&mut harness, "todo.md", "Discard all…");
    wait_for_label(&mut harness, "Discard the changes to 2 files?");
    harness.get_by_label("Discard").click();
    wait_for_label(&mut harness, "Nothing to stage.");
    assert_eq!(read(&repo, "notes.txt"), Some(notes(&[])));
    assert_eq!(read(&repo, "todo.md"), None);
}
