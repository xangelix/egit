//! Undo, redo, and the reflog, driven as a user drives them: a commit taken back (its changes
//! staged again) and made again; a merge taken back and redone by the keys; a switch taken
//! back, and one that stashed changes in its way (popped again); branches, tags, and stashes
//! made and removed, each taken back; an undo refused once the repository moved on outside
//! egit; and the reflog listing where `HEAD` has been, a move's commit in the details (its
//! files opened in place), a commit no branch reaches kept by a branch made from it, and the
//! current branch reset to a move.

use std::path::Path;

use egit_git::ObjectId;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::patches::wait_for_text;
use super::screens::{
    click_when_enabled, double_click, harness, history_of, history_shown, menu_item, open_path,
    profile_loaded, render, settle, two_branches, type_into, uncommitted, wait_for_label,
};
use super::search::details_shown;
use crate::app::EgitApp;
use crate::theme::icon;

fn badge(name: &str) -> String {
    format!("{} {name}", icon::GIT_BRANCH)
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> Option<ObjectId> {
    let out = sandbox
        .run(repo, "rev-parse", &["--verify", "--quiet", name])
        .ok()?;
    ObjectId::from_hex(out.stdout.trim_ascii())
}

fn git_says(sandbox: &Sandbox, repo: &Path, args: &[&'static str]) -> String {
    let out = sandbox.run(repo, args[0], &args[1..]).unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Whether the history shows `HEAD`'s commit as it is now.
fn head_shown(sandbox: &Sandbox, repo: &Path) -> impl Fn(&mut EgitApp) -> bool {
    let id = rev(sandbox, repo, "HEAD").unwrap();
    move |app| {
        let repo = &mut app.state.repos[0];
        !repo.history.is_loading() && repo.history.shown.row_of(&id, 0).is_some()
    }
}

fn opened(sandbox: &Sandbox, repo: &Path) -> Harness<'static, EgitApp> {
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, repo);
    settle(&mut harness, "the history", history_shown);
    harness
}

fn is_disabled(harness: &Harness<'_, EgitApp>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn commits_merges_and_switches_are_taken_back_and_done_again() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = opened(&sandbox, &repo);
    assert!(is_disabled(&harness, "Undo"), "nothing done yet");
    assert!(is_disabled(&harness, "Redo"));

    // A commit taken back: its changes are staged again; then made again, the same commit.
    std::fs::write(repo.join("f"), "three\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage all");
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 1 file");
    type_into(&mut harness, Role::TextInput, "Summary", "Three");
    harness.get_by_label("Commit 1 file").click();
    settle(&mut harness, "the commit shown", history_of(4));
    let (two, three) = (
        rev(&sandbox, &repo, "HEAD~1").unwrap(),
        rev(&sandbox, &repo, "HEAD").unwrap(),
    );
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid the commit “Three”.");
    assert_eq!(rev(&sandbox, &repo, "main"), Some(two));
    assert_eq!(
        git_says(&sandbox, &repo, &["diff", "--cached", "--name-only"]),
        "f"
    );
    settle(&mut harness, "the commit gone", history_of(3));
    render(&mut harness, "undo-commit");
    click_when_enabled(&mut harness, "Redo");
    wait_for_label(&mut harness, "Redid the commit “Three”.");
    assert_eq!(rev(&sandbox, &repo, "main"), Some(three));
    settle(&mut harness, "the commit back", history_of(4));

    // A merge, taken back by ⌘Z (its files with it), and done again by ⇧⌘Z.
    menu_item(&mut harness, &badge("other"), "Merge other into main");
    wait_for_label(&mut harness, "Merged other.");
    let merge = rev(&sandbox, &repo, "HEAD").unwrap();
    assert!(repo.join("g").exists());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    wait_for_label(&mut harness, "Undid the merge of other.");
    assert_eq!(rev(&sandbox, &repo, "main"), Some(three));
    assert!(!repo.join("g").exists());
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    wait_for_label(&mut harness, "Redid the merge of other.");
    assert_eq!(rev(&sandbox, &repo, "main"), Some(merge));
    settle(&mut harness, "the merge shown", head_shown(&sandbox, &repo));

    // A switch, taken back.
    double_click(&mut harness, &badge("other"));
    wait_for_label(&mut harness, "Switched to other.");
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid the switch to other.");
    assert_eq!(
        git_says(&sandbox, &repo, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );

    // Changes in the way, stashed for a switch: taken back, they're where they were. The
    // click is aimed once the screen is still: the changes listed, the history read again.
    std::fs::write(repo.join("f"), "mine\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    settle(
        &mut harness,
        "the switch undone",
        head_shown(&sandbox, &repo),
    );
    menu_item(&mut harness, &badge("other"), "Switch to other");
    wait_for_label(&mut harness, "Your changes are in the way");
    harness.get_by_label("Stash and switch").click();
    wait_for_label(&mut harness, "Stashed your changes, and switched to other.");
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid the switch to other.");
    assert_eq!(
        git_says(&sandbox, &repo, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
    assert_eq!(std::fs::read_to_string(repo.join("f")).unwrap(), "mine\n");
    assert_eq!(git_says(&sandbox, &repo, &["stash", "list"]), "");
    sandbox.run(&repo, "checkout", &["--", "f"]).unwrap();
    // The work tree read again: the screens below are looked at control by control.
    settle(&mut harness, "a clean work tree", |app| {
        app.state.repos[0]
            .work_tree
            .status()
            .and_then(Result::ok)
            .is_some_and(|status| status.is_empty())
    });

    // Moved on outside egit: the merge can't be taken back, and nothing changes.
    sandbox
        .commit_file(&repo, "h", "outside\n", "Outside")
        .unwrap();
    let outside = rev(&sandbox, &repo, "HEAD");
    settle(
        &mut harness,
        "the outside commit",
        head_shown(&sandbox, &repo),
    );
    click_when_enabled(&mut harness, "Undo");
    wait_for_text(&mut harness, "It can't be taken back: things changed since");
    assert_eq!(rev(&sandbox, &repo, "HEAD"), outside);
    render(&mut harness, "undo-refused");
}

#[test]
fn branches_tags_and_stashes_are_brought_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = opened(&sandbox, &repo);
    let other = rev(&sandbox, &repo, "other");

    // A branch deleted (its commits merged nowhere), brought back where it was.
    menu_item(&mut harness, &badge("other"), "Delete other…");
    wait_for_label(&mut harness, "Delete other?");
    harness.get_by_label("Delete").click();
    wait_for_label(&mut harness, "Delete anyway");
    harness.get_by_label("Delete anyway").click();
    wait_for_label(&mut harness, "Deleted other.");
    assert_eq!(rev(&sandbox, &repo, "refs/heads/other"), None);
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid deleting other.");
    assert_eq!(rev(&sandbox, &repo, "refs/heads/other"), other);
    assert!(!is_disabled(&harness, "Redo"), "it can be deleted again");

    // A tag made, taken back; another deleted, brought back.
    sandbox.run(&repo, "tag", &["kept", "main"]).unwrap();
    wait_for_label(&mut harness, &format!("{} kept", icon::TAG));
    harness
        .get_by_role_and_label(Role::Button, "One")
        .click_secondary();
    wait_for_label(&mut harness, "Create a tag here…");
    harness.get_by_label("Create a tag here…").click();
    wait_for_label(&mut harness, "Create tag");
    type_into(&mut harness, Role::TextInput, "Name", "v1");
    harness.get_by_label("Create tag").click();
    wait_for_label(&mut harness, &format!("{} v1", icon::TAG));
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid making the tag v1.");
    assert_eq!(rev(&sandbox, &repo, "refs/tags/v1"), None);
    menu_item(
        &mut harness,
        &format!("{} kept", icon::TAG),
        "Delete the tag kept…",
    );
    wait_for_label(&mut harness, "Delete the tag kept?");
    harness.get_by_label("Delete").click();
    wait_for_label(&mut harness, "Deleted the tag kept.");
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid deleting the tag kept.");
    assert_eq!(
        rev(&sandbox, &repo, "refs/tags/kept"),
        rev(&sandbox, &repo, "main")
    );

    // Changes stashed, taken back (they're back in the work tree); a stash dropped, kept.
    let stash = |count: usize| match count {
        0 => format!("{}  Stash", icon::ARCHIVE),
        count => format!("{}  Stashes · {count}", icon::ARCHIVE),
    };
    std::fs::write(repo.join("f"), "changed\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&stash(0)).click();
    click_when_enabled(&mut harness, "Stash the changes…");
    wait_for_label(&mut harness, "Stash your changes");
    harness.get_by_label("Stash").click();
    wait_for_label(&mut harness, "Stashed your changes.");
    assert_eq!(std::fs::read_to_string(repo.join("f")).unwrap(), "two\n");
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid stashing the changes.");
    assert_eq!(
        std::fs::read_to_string(repo.join("f")).unwrap(),
        "changed\n"
    );
    assert_eq!(git_says(&sandbox, &repo, &["stash", "list"]), "");

    wait_for_label(&mut harness, &stash(0));
    harness.get_by_label(&stash(0)).click();
    click_when_enabled(&mut harness, "Stash the changes…");
    wait_for_label(&mut harness, "Stash your changes");
    harness.get_by_label("Stash").click();
    wait_for_label(&mut harness, &stash(1));
    let stashed = rev(&sandbox, &repo, "refs/stash");
    harness.get_by_label(&stash(1)).click();
    wait_for_label(&mut harness, "Drop…");
    harness.get_by_label("Drop…").click();
    wait_for_label(&mut harness, "Drop stash@{0}?");
    harness.get_by_label("Drop").click();
    wait_for_label(&mut harness, "Dropped stash@{0}.");
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(&mut harness, "Undid dropping stash@{0}.");
    assert_eq!(rev(&sandbox, &repo, "refs/stash"), stashed);
    wait_for_label(&mut harness, &stash(1));
}

#[test]
fn the_reflog_shows_where_head_has_been_and_keeps_what_nothing_reaches() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    sandbox.commit_file(&repo, "f", "three\n", "Three").unwrap();
    let three = rev(&sandbox, &repo, "HEAD").unwrap();
    // Three is left behind: no branch reaches it now.
    sandbox
        .run(&repo, "reset", &["--quiet", "--hard", "HEAD~1"])
        .unwrap();
    let two = rev(&sandbox, &repo, "HEAD").unwrap();
    let mut harness = opened(&sandbox, &repo);

    harness
        .get_by_label(&format!("{}  Reflog", icon::CLOCK_COUNTER_CLOCKWISE))
        .click();
    wait_for_label(&mut harness, "Reflog of HEAD");
    wait_for_label(&mut harness, "commit: Three");
    // The newest move is selected: the reset, to Two, which the history shows.
    assert!(!is_disabled(&harness, "Show the commit"));
    harness
        .get_by_role_and_label(Role::Button, "commit: Three")
        .click();
    // Its details beside it, though no branch reaches it.
    settle(&mut harness, "the move's commit's details", details_shown);
    assert!(
        is_disabled(&harness, "Show the commit"),
        "no branch reaches it"
    );
    render(&mut harness, "reflog");

    // Its files open in the reflog's place; Escape goes back to it.
    harness.get_by_role_and_label(Role::Button, "f").click();
    wait_for_label(&mut harness, "File history");
    assert!(harness.query_by_label("Reflog of HEAD").is_none());
    harness.key_press(Key::Escape);
    wait_for_label(&mut harness, "Reflog of HEAD");

    // Kept by a branch made there (without switching to it); then shown in the history.
    harness.get_by_label("Create a branch here…").click();
    wait_for_label(
        &mut harness,
        &format!("Create a branch at {}", three.abbreviated(8)),
    );
    type_into(&mut harness, Role::TextInput, "Name", "rescued");
    harness.get_by_label("Create").click();
    wait_for_label(&mut harness, "Made rescued.");
    assert_eq!(rev(&sandbox, &repo, "rescued"), Some(three));
    click_when_enabled(&mut harness, "Show the commit");
    wait_for_label(&mut harness, &badge("rescued"));
    assert!(
        harness.query_by_label("Reflog of HEAD").is_none(),
        "back to the history"
    );

    // The current branch reset to a move, keeping uncommitted changes; the reflog read again.
    menu_item(&mut harness, &badge("main"), "Show main's reflog");
    wait_for_label(&mut harness, "Reflog of main");
    wait_for_label(&mut harness, "commit: Three");
    harness
        .get_by_role_and_label(Role::Button, "commit: Three")
        .click();
    harness.run_steps(2);
    std::fs::write(repo.join("g"), "untracked\n").unwrap();
    let reset = "Reset main to here";
    harness.get_by_label(reset).click();
    let keep = "Keep: keep uncommitted changes (refused if in the way)";
    wait_for_label(&mut harness, keep);
    harness.get_by_label(keep).click();
    wait_for_label(&mut harness, &format!("Reset to {}.", three.abbreviated(8)));
    assert_eq!(rev(&sandbox, &repo, "main"), Some(three));
    assert!(repo.join("g").exists());
    wait_for_text(&mut harness, &format!("reset: moving to {three}"));
    // Taken back like any other action.
    click_when_enabled(&mut harness, "Undo");
    wait_for_label(
        &mut harness,
        &format!("Undid the reset to {}.", three.abbreviated(8)),
    );
    assert_eq!(rev(&sandbox, &repo, "main"), Some(two));
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label("Reflog of main").is_none());
}
