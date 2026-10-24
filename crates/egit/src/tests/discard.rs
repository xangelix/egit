//! Discarding changes not staged, driven as a user drives it: a hunk from a file's diff (nothing
//! asked) and Undo putting it back; a right click in the diff staging a line's hunk, and in the
//! staged diff unstaging it; a file from its row's menu, asked first; an untracked file
//! deleted from its menu and brought back by Undo; and everything not staged at once, from a
//! row's menu. Then one file's changes stashed from its menu (Undo putting them back), and a
//! conflict resolved whole as the incoming side has it, from its menu.

use std::path::Path;

use egit_git::Uncommitted;
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

/// Right-clicks line `line` of the open file's diff (press and release in one frame).
fn right_click_line(harness: &mut Harness<'_, EgitApp>, line: u32) {
    let spot = harness.state_mut().state.repos[0]
        .history_view
        .file_spot(line, 2)
        .unwrap();
    harness.hover_at(spot);
    harness.run_steps(1);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: spot,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.run_steps(2);
}

/// Right-clicks the diff's `line`, then clicks the menu's `item` once it shows beside the bar's
/// button of the same words (lower: by the pointer), after `before`. A right click landing in
/// a frame that lays the diff out again opens nothing: it is made again.
fn menu_beside_the_bar(
    harness: &mut Harness<'_, EgitApp>,
    (line, item): (u32, &str),
    before: impl FnOnce(&mut Harness<'_, EgitApp>),
) {
    // Under load a right click may land before the diff has: tried again, a while each.
    'tries: for _ in 0..5 {
        right_click_line(harness, line);
        for _ in 0..200 {
            if harness.query_all_by_label(item).count() > 1 {
                break 'tries;
            }
            harness.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    before(harness);
    harness
        .query_all_by_label(item)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    harness.run_steps(2);
}

/// What the index holds that `HEAD` doesn't, as a patch.
fn staged(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox
        .run(repo, "diff", &["--cached", "--no-color"])
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// Steps until `count` buttons are labelled `label`: the work lists read again (a while at
/// most; what follows says what didn't come).
fn wait_for_rows(harness: &mut Harness<'_, EgitApp>, label: &str, count: usize) {
    for _ in 0..2_000 {
        if harness
            .query_all_by_role_and_label(Role::Button, label)
            .count()
            == count
        {
            break;
        }
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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

    // A right click on a changed line (nothing selected): its menu stages the line's hunk,
    // and the staged side's unstages it. Once the diff is read again after the undo (both
    // hunks: nine lines each), or the click lands on nothing.
    settle(&mut harness, "the diff read again", |app| {
        app.state.repos[0].history_view.diff_lines() == Some(18)
    });
    menu_beside_the_bar(&mut harness, (5, "Stage hunk"), |harness| {
        render(harness, "diff-lines-menu");
    });
    settle(&mut harness, "the hunk staged", |_| {
        staged(&sandbox, &repo).contains("+LINE 5")
    });
    // Its row in the staged list (below the other), once the status is read again: until
    // then the only row is the unstaged one, whose diff has nine lines too.
    wait_for_rows(&mut harness, "notes.txt", 2);
    let staged_row = harness
        .query_all_by_role_and_label(Role::Button, "notes.txt")
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap();
    staged_row.click();
    settle(&mut harness, "the staged diff", |app| {
        let view = &mut app.state.repos[0].history_view;
        view.work_side() == Some(Uncommitted::Staged) && view.diff_lines() == Some(9)
    });
    menu_beside_the_bar(&mut harness, (5, "Unstage hunk"), |_| {});
    settle(&mut harness, "the hunk unstaged", |_| {
        staged(&sandbox, &repo).is_empty()
    });

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
    // The list read again without it, so the rows aimed at below don't move.
    wait_for_rows(&mut harness, "other.txt", 0);
    harness.run_steps(2);

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

#[test]
fn one_files_changes_are_stashed_from_its_menu() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "notes.txt", "one\n", "Write notes")
        .unwrap();
    sandbox
        .commit_file(&repo, "other.txt", "a\n", "Other")
        .unwrap();
    std::fs::write(repo.join("notes.txt"), "two\n").unwrap();
    std::fs::write(repo.join("other.txt"), "b\n").unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, &uncommitted(2));
    harness.get_by_label(&uncommitted(2)).click();
    wait_for_label(&mut harness, "Not staged");

    // notes.txt's changes alone go into the stash; other.txt's stay.
    row_menu(&mut harness, "notes.txt", "Stash this file…");
    wait_for_label(&mut harness, "Stash the changes to notes.txt");
    render(&mut harness, "stash-one-file");
    // The dialog's (the top bar has a Stash menu too).
    harness
        .get_all_by_role_and_label(Role::Button, "Stash")
        .last()
        .unwrap()
        .click();
    wait_for_label(&mut harness, "Stashed the changes to notes.txt.");
    assert_eq!(read(&repo, "notes.txt").as_deref(), Some("one\n"));
    assert_eq!(read(&repo, "other.txt").as_deref(), Some("b\n"));
    let listed = sandbox.run(&repo, "stash", &["list"]).unwrap().stdout;
    assert_eq!(String::from_utf8(listed).unwrap().lines().count(), 1);

    // Undo puts them back, the stash gone.
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid stashing the changes.");
    assert_eq!(read(&repo, "notes.txt").as_deref(), Some("two\n"));
    let listed = sandbox.run(&repo, "stash", &["list"]).unwrap().stdout;
    assert_eq!(listed, b"");
}

#[test]
fn a_conflict_is_resolved_whole_from_its_menu() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "base\n", "Base").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "f", "theirs\n", "Theirs")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.commit_file(&repo, "f", "ours\n", "Ours").unwrap();
    // The merge stops at the conflict (git exits 1).
    assert!(sandbox.run(&repo, "merge", &["--quiet", "side"]).is_err());
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Not staged");

    // The incoming side taken whole, without the merge editor: resolved and staged.
    wait_for_label(&mut harness, "f");
    harness
        .get_by_role_and_label(Role::Button, "f")
        .click_secondary();
    wait_for_label(&mut harness, "Take the incoming side");
    render(&mut harness, "conflict-row-menu");
    harness.get_by_label("Take the incoming side").click();
    settle(&mut harness, "the conflict resolved", |app| {
        app.state.repos[0]
            .work_tree
            .status()
            .and_then(Result::ok)
            .is_some_and(|status| {
                status
                    .entries()
                    .all(|entry| entry.kind != egit_git::EntryKind::Unmerged)
            })
    });
    assert_eq!(read(&repo, "f").as_deref(), Some("theirs\n"));
    let unmerged = sandbox
        .run(&repo, "diff", &["--name-only", "--diff-filter=U"])
        .unwrap()
        .stdout;
    assert_eq!(unmerged, b"");
    let staged = sandbox
        .run(&repo, "diff", &["--cached", "--name-only"])
        .unwrap()
        .stdout;
    assert_eq!(staged, b"f\n");
}
