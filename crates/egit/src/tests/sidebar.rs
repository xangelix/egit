//! The sidebar driven as a user drives it: what the repository has listed and counted (branches
//! folded into folders, tags folded at first, stashes, the worktree here), narrowed by the
//! filter; a branch's row showing its commit, a stash's showing its changes; a branch hidden
//! from the graph and shown again; remotes added (fetched), renamed, pointed elsewhere, fetched,
//! and removed; worktrees locked, unlocked, opened in a tab, and removed (asked again when they
//! have changes); and a submodule not set up yet, set up.

use std::path::{Path, PathBuf};

use egit_git::ObjectId;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::patches::wait_for_text;
use super::screens::{
    branchy, harness, history_shown, menu_item, open_path, profile_loaded, render, settle,
    type_into, wait_for_label,
};
use crate::app::EgitApp;
use crate::theme::icon;

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> Option<ObjectId> {
    let out = sandbox
        .run(repo, "rev-parse", &["--verify", "--quiet", name])
        .ok()?;
    ObjectId::from_hex(out.stdout.trim_ascii())
}

fn config(sandbox: &Sandbox, repo: &Path, key: &str) -> Option<String> {
    let out = sandbox.run(repo, "config", &["--get", key]).ok()?;
    Some(String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

fn opened(sandbox: &Sandbox, repo: &Path) -> Harness<'static, EgitApp> {
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    harness
}

/// A row of the sidebar (or anything else named so) by its name.
fn row<'h>(harness: &'h Harness<'_, EgitApp>, name: &'h str) -> egui_kittest::Node<'h> {
    harness.get_by_role_and_label(Role::Button, name)
}

fn shown(harness: &Harness<'_, EgitApp>, name: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Button, name)
        .is_some()
}

/// Replaces what a field holds with `text`.
fn retype(harness: &mut Harness<'_, EgitApp>, role: Role, label: &str, text: &str) {
    harness.get_by_role_and_label(role, label).focus();
    harness.run_steps(1);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.key_press(Key::Backspace);
    type_into(harness, role, label, text);
}

/// Whether the history shows `commit`, and is read whole.
fn has(commit: ObjectId) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        let repo = &mut app.state.repos[0];
        !repo.history.is_loading() && repo.history.shown.row_of(&commit, 0).is_some()
    }
}

#[test]
fn the_sidebar_lists_filters_shows_and_hides() {
    let sandbox = Sandbox::new().unwrap();
    let repo = branchy(&sandbox);
    for branch in ["feature/ui", "feature/api"] {
        sandbox.run(&repo, "branch", &[branch, "main"]).unwrap();
    }
    std::fs::write(repo.join("README"), "stashed\n").unwrap();
    sandbox
        .run(
            &repo,
            "stash",
            &["push", "--quiet", "--message", "Half done"],
        )
        .unwrap();
    let mut harness = opened(&sandbox, &repo);

    // Counted, folded into folders, tags folded at first; the stash and the worktree here.
    wait_for_label(&mut harness, "Branches: 5");
    for name in [
        "docs",
        "feature/",
        "feature/api",
        "feature/ui",
        "main",
        "parser",
    ] {
        assert!(shown(&harness, name), "{name}");
    }
    assert!(shown(&harness, "Tags: 1") && !shown(&harness, "v0.1"));
    wait_for_label(&mut harness, "stash@{0}: On main: Half done");
    assert!(
        harness
            .query_all_by_label_contains("main · ")
            .next()
            .is_some(),
        "the worktree here"
    );
    render(&mut harness, "sidebar");
    row(&harness, "feature/").click();
    harness.run_steps(2);
    assert!(!shown(&harness, "feature/ui"), "folded");
    row(&harness, "Tags: 1").click();
    wait_for_label(&mut harness, "v0.1");

    // Narrowed by the filter.
    type_into(&mut harness, Role::TextInput, "Filter", "pars");
    wait_for_label(&mut harness, "Branches: 1");
    assert!(shown(&harness, "parser") && !shown(&harness, "docs"));
    render(&mut harness, "sidebar-filtered");
    // Escape clears it.
    harness.key_press(Key::Escape);
    wait_for_label(&mut harness, "Branches: 5");

    // A branch's row selects its commit; a stash's shows its changes in the details.
    row(&harness, "parser").click();
    wait_for_label(&mut harness, "Make the parser answer");
    row(&harness, "stash@{0}: On main: Half done").click();
    wait_for_text(&mut harness, "On main: Half done");
    wait_for_label(&mut harness, "1 file changed");

    // Hidden from the graph: its own commit goes, with its badge; shown again.
    let docs = rev(&sandbox, &repo, "docs").unwrap();
    harness
        .get_by_role_and_label(Role::CheckBox, "Show docs in the graph")
        .click();
    wait_for_label(
        &mut harness,
        &format!("{}  1 branch hidden", icon::EYE_SLASH),
    );
    settle(&mut harness, "the history without docs", |app| {
        let repo = &mut app.state.repos[0];
        !repo.history.is_loading() && repo.history.shown.row_of(&docs, 0).is_none()
    });
    assert!(
        harness
            .query_by_label(&format!("{} docs", icon::GIT_BRANCH))
            .is_none(),
        "no badge"
    );
    render(&mut harness, "sidebar-hidden");
    harness
        .get_by_role_and_label(Role::CheckBox, "Show docs in the graph")
        .click();
    settle(&mut harness, "the history with docs", has(docs));
}

#[test]
fn remotes_are_set_up_from_the_sidebar() {
    let sandbox = Sandbox::new().unwrap();
    let upstream = sandbox.init("upstream").unwrap();
    sandbox.commit_file(&upstream, "f", "one\n", "One").unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "g", "g\n", "Mine").unwrap();
    let mut harness = opened(&sandbox, &repo);
    let url = format!("file://{}", upstream.display());

    // Added, its name offered, and fetched at once.
    wait_for_label(&mut harness, "No remotes");
    harness.get_by_label("Add a remote").click();
    wait_for_label(&mut harness, "Add remote");
    type_into(&mut harness, Role::TextInput, "Address (URL)", &url);
    harness.get_by_label("Add remote").click();
    wait_for_label(&mut harness, "Added origin, and fetched from it.");
    wait_for_label(&mut harness, "origin/main");
    assert_eq!(
        config(&sandbox, &repo, "remote.origin.url"),
        Some(url.clone())
    );
    render(&mut harness, "sidebar-remote");

    // Renamed.
    menu_item(&mut harness, "origin", "Rename…");
    wait_for_label(&mut harness, "Rename the remote origin");
    retype(&mut harness, Role::TextInput, "New name", "upstream");
    harness.get_by_label("Rename").click();
    wait_for_label(&mut harness, "Renamed origin to upstream.");
    wait_for_label(&mut harness, "upstream/main");
    assert!(rev(&sandbox, &repo, "refs/remotes/upstream/main").is_some());

    // Pushing elsewhere.
    let mirror = sandbox.path("mirror.git");
    let push = format!("file://{}", mirror.display());
    menu_item(&mut harness, "upstream", "Change its address…");
    wait_for_label(&mut harness, "Change upstream's address");
    type_into(
        &mut harness,
        Role::TextInput,
        "Pushes to (empty: where it fetches from)",
        &push,
    );
    harness.get_by_label("Save").click();
    wait_for_label(&mut harness, "Changed upstream's address.");
    assert_eq!(
        config(&sandbox, &repo, "remote.upstream.pushurl"),
        Some(push)
    );
    assert_eq!(config(&sandbox, &repo, "remote.upstream.url"), Some(url));

    // Fetched alone; then removed, its branches with it.
    sandbox.commit_file(&upstream, "f", "two\n", "Two").unwrap();
    menu_item(&mut harness, "upstream", "Fetch upstream");
    wait_for_label(&mut harness, "Fetched from upstream.");
    assert_eq!(
        rev(&sandbox, &repo, "refs/remotes/upstream/main"),
        rev(&sandbox, &upstream, "main")
    );
    menu_item(&mut harness, "upstream", "Remove…");
    wait_for_label(&mut harness, "Remove the remote upstream?");
    harness.get_by_label("Remove").click();
    wait_for_label(&mut harness, "Removed upstream.");
    wait_for_label(&mut harness, "No remotes");
    assert_eq!(rev(&sandbox, &repo, "refs/remotes/upstream/main"), None);
}

#[test]
fn worktrees_and_submodules_are_managed_from_the_sidebar() {
    let sandbox = Sandbox::new().unwrap();
    // Submodules over `file://` are refused unless allowed (git 2.38.1 on).
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.file.allow", "always"],
        )
        .unwrap();
    let library = sandbox.init("library").unwrap();
    sandbox
        .commit_file(&library, "lib", "one\n", "One")
        .unwrap();
    let origin = sandbox.init("origin").unwrap();
    sandbox.commit_file(&origin, "f", "f\n", "Start").unwrap();
    let url = format!("file://{}", library.display());
    sandbox
        .run(
            &origin,
            "submodule",
            &["--quiet", "add", &url, "vendor/lib"],
        )
        .unwrap();
    sandbox
        .run(
            &origin,
            "commit",
            &["--quiet", "--message", "Add the library"],
        )
        .unwrap();
    // A clone, its submodule not set up yet, and a worktree beside it.
    let repo = sandbox.path("repo");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", origin.to_str().unwrap(), repo.to_str().unwrap()],
        )
        .unwrap();
    let side: PathBuf = sandbox.path("side");
    sandbox
        .run(
            &repo,
            "worktree",
            &["add", "--quiet", "-b", "side", side.to_str().unwrap()],
        )
        .unwrap();
    let mut harness = opened(&sandbox, &repo);
    wait_for_label(&mut harness, "Worktrees: 2");
    let side_row = harness
        .query_all_by_label_contains("side · ")
        .next()
        .and_then(|node| node.accesskit_node().label())
        .unwrap();

    // Locked, then unlocked.
    menu_item(&mut harness, &side_row, "Lock…");
    wait_for_label(&mut harness, "Lock");
    type_into(
        &mut harness,
        Role::TextInput,
        "Why (optional)",
        "on a drive",
    );
    harness.get_by_label("Lock").click();
    wait_for_text(&mut harness, "Locked the worktree at");
    settle(&mut harness, "the lock read", |app| {
        app.state.repos[0]
            .worktrees
            .ok_ref()
            .is_some_and(|worktrees| worktrees.iter().any(|worktree| worktree.locked.is_some()))
    });
    menu_item(&mut harness, &side_row, "Unlock");
    wait_for_text(&mut harness, "Unlocked the worktree at");

    // Opened in a tab, which comes to the front; then back to this one.
    menu_item(&mut harness, &side_row, "Open in a tab");
    settle(&mut harness, "the worktree's tab", |app| {
        app.state.repos.len() == 2
    });
    wait_for_label(&mut harness, &format!("{}  side", icon::GIT_BRANCH));
    harness
        .get_by_label(&format!("{}  repo", icon::GIT_BRANCH))
        .click();
    wait_for_label(&mut harness, "Worktrees: 2");

    // Removed: asked again when it has changes.
    std::fs::write(side.join("f"), "changed\n").unwrap();
    menu_item(&mut harness, &side_row, "Remove…");
    wait_for_text(&mut harness, "Remove the worktree at");
    wait_for_label(&mut harness, "Remove");
    harness.get_by_label("Remove").click();
    wait_for_label(&mut harness, "Remove anyway");
    render(&mut harness, "sidebar-worktree-changes");
    harness.get_by_label("Remove anyway").click();
    wait_for_text(&mut harness, "Removed the worktree at");
    assert!(!side.exists());

    // The submodule, not set up yet, set up (aimed at once the worktrees are read again: the
    // rows below them move up).
    wait_for_label(&mut harness, "Worktrees: 1");
    wait_for_label(&mut harness, "vendor/lib");
    menu_item(&mut harness, "vendor/lib", "Set up vendor/lib");
    wait_for_label(&mut harness, "Updated vendor/lib.");
    assert_eq!(
        std::fs::read_to_string(repo.join("vendor/lib/lib")).unwrap(),
        "one\n"
    );
}
