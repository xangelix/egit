//! The branch and commit menus' actions beyond switching and merging, driven as a user drives
//! them: pulling, pushing, and fast-forwarding branches other than the current one; upstreams
//! set and forgotten; a branch deleted here and on its remote at once; names and links copied;
//! a commit saved as a patch; worktrees made (one opened in a tab); and the graph narrowed to a
//! branch, or a branch pinned to its left.

use std::path::Path;

use egit_git::ObjectId;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    branchy, click_when_enabled, colleague_pushes, harness, history_of, history_shown, menu_item,
    open_path, profile_loaded, render, settle, shared_remote, type_into, wait_for_label,
};
use crate::app::EgitApp;
use crate::desktop::FileDialogs;
use crate::theme::icon;

fn badge(name: &str) -> String {
    format!("{} {name}", icon::GIT_BRANCH)
}

fn remote_badge(name: &str) -> String {
    format!("{} {name}", icon::CLOUD)
}

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

/// Whether the history shows `name`'s commit as it is now, and the configuration is read: a
/// count of rows wouldn't do (the background fetch may bring a colleague's commit in).
fn shown(sandbox: &Sandbox, repo: &Path, name: &str) -> impl Fn(&mut EgitApp) -> bool {
    let id = rev(sandbox, repo, name).unwrap();
    move |app| {
        let repo = &mut app.state.repos[0];
        !repo.config.is_pending()
            && !repo.history.is_loading()
            && repo.history.shown.row_of(&id, 0).is_some()
    }
}

/// Whether the last frame copied `text`.
fn copied(harness: &Harness<'_, EgitApp>, text: &str) -> bool {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .any(|command| *command == egui::OutputCommand::CopyText(text.to_owned()))
}

/// A clone sharing a remote with a colleague's: its `old` follows `origin/main` from where it
/// was, `loner` follows nothing; opened, the colleague's push not fetched yet.
fn opened_with_branches(
    sandbox: &Sandbox,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    Harness<'static, EgitApp>,
) {
    let (colleague, mine) = shared_remote(sandbox);
    for args in [
        &["branch", "--quiet", "--track", "old", "origin/main"][..],
        &["branch", "--quiet", "loner"],
    ] {
        sandbox.run(&mine, args[0], &args[1..]).unwrap();
    }
    colleague_pushes(sandbox, &colleague, "theirs", "x\n");
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &mine);
    settle(&mut harness, "the history", history_shown);
    // Not a test of it: it would read the history again under the screens rendered.
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    (colleague, mine, harness)
}

#[test]
fn branches_are_pulled_and_pushed_from_their_menus() {
    let sandbox = Sandbox::new().unwrap();
    let (_, mine, mut harness) = opened_with_branches(&sandbox);
    click_when_enabled(&mut harness, &format!("{}  Fetch", icon::ARROWS_CLOCKWISE));
    wait_for_label(&mut harness, "Fetched.");
    settle(&mut harness, "the fetched history", history_of(2));

    // A branch that isn't checked out, pulled by fast-forward; then the current one moved to
    // the remote's.
    menu_item(&mut harness, &badge("old"), "Pull old (fast-forward only)");
    wait_for_label(&mut harness, "Fast-forwarded to origin/main.");
    assert_eq!(
        rev(&sandbox, &mine, "old"),
        rev(&sandbox, &mine, "origin/main")
    );
    settle(&mut harness, "the history read again", history_of(2));
    menu_item(
        &mut harness,
        &remote_badge("origin/main"),
        "Fast-forward main to origin/main",
    );
    wait_for_label(&mut harness, "Fast-forwarded to origin/main.");
    assert_eq!(
        rev(&sandbox, &mine, "main"),
        rev(&sandbox, &mine, "origin/main")
    );

    // A branch pushed for the first time from its menu, then again.
    for step in ["one", "two"] {
        sandbox.run(&mine, "switch", &["--quiet", "loner"]).unwrap();
        sandbox.commit_file(&mine, step, "x\n", step).unwrap();
        sandbox.run(&mine, "switch", &["--quiet", "main"]).unwrap();
        settle(&mut harness, "the commit shown", |app| {
            app.state.repos[0]
                .history
                .shown
                .row_of(&rev(&sandbox, &mine, "loner").unwrap(), 0)
                == Some(0)
                && !app.state.repos[0].history.is_loading()
        });
        if step == "one" {
            menu_item(&mut harness, &badge("loner"), "Push loner…");
            wait_for_label(&mut harness, "Push loner for the first time");
            harness.get_by_label("Push").click();
            wait_for_label(&mut harness, "Pushed: loner is new on the remote.");
        } else {
            menu_item(&mut harness, &badge("loner"), "Push loner to origin/loner");
            wait_for_label(&mut harness, "Pushed to loner.");
        }
    }
    assert_eq!(
        rev(&sandbox, &mine, "loner"),
        rev(&sandbox, &mine, "refs/remotes/origin/loner")
    );
}

#[test]
fn upstreams_are_set_names_copied_and_branches_deleted_everywhere() {
    let sandbox = Sandbox::new().unwrap();
    let (colleague, mine, mut harness) = opened_with_branches(&sandbox);
    // `loner`, with a commit, pushed and following its remote's.
    sandbox.run(&mine, "switch", &["--quiet", "loner"]).unwrap();
    sandbox.commit_file(&mine, "one", "x\n", "one").unwrap();
    sandbox
        .run(
            &mine,
            "push",
            &["--quiet", "--set-upstream", "origin", "loner"],
        )
        .unwrap();
    sandbox.run(&mine, "switch", &["--quiet", "main"]).unwrap();
    settle(
        &mut harness,
        "the pushed branch read",
        shown(&sandbox, &mine, "loner"),
    );

    // Its upstream changed, then forgotten.
    menu_item(&mut harness, &badge("loner"), "Set upstream…");
    wait_for_label(&mut harness, "Set loner's upstream");
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    type_into(&mut harness, Role::TextInput, "Upstream", "origin/main");
    render(&mut harness, "set-upstream");
    harness.get_by_label("Set upstream").click();
    wait_for_label(&mut harness, "loner now follows origin/main.");
    assert_eq!(
        config(&sandbox, &mine, "branch.loner.merge").as_deref(),
        Some("refs/heads/main")
    );
    settle(&mut harness, "the configuration read again", |app| {
        !app.state.repos[0].config.is_pending()
    });
    menu_item(&mut harness, &badge("loner"), "Set upstream…");
    wait_for_label(&mut harness, "Forget origin/main");
    harness.get_by_label("Forget origin/main").click();
    wait_for_label(&mut harness, "loner follows no upstream now.");
    assert_eq!(config(&sandbox, &mine, "branch.loner.merge"), None);

    // Its name copied.
    menu_item(&mut harness, &badge("loner"), "Copy branch name");
    harness.run_steps(1);
    assert!(copied(&harness, "loner"));

    // Deleted here and on the remote at once: with a commit nowhere else, asked again.
    sandbox
        .run(
            &mine,
            "branch",
            &["--quiet", "--set-upstream-to", "origin/loner", "loner"],
        )
        .unwrap();
    sandbox.run(&mine, "switch", &["--quiet", "loner"]).unwrap();
    sandbox.commit_file(&mine, "three", "x\n", "three").unwrap();
    sandbox.run(&mine, "switch", &["--quiet", "main"]).unwrap();
    settle(
        &mut harness,
        "the commit and the upstream read",
        shown(&sandbox, &mine, "loner"),
    );
    menu_item(
        &mut harness,
        &badge("loner"),
        "Delete loner and origin/loner…",
    );
    wait_for_label(&mut harness, "Delete loner and origin/loner?");
    render(&mut harness, "delete-everywhere");
    harness.get_by_label("Delete").click();
    wait_for_label(&mut harness, "Delete anyway");
    harness.get_by_label("Delete anyway").click();
    wait_for_label(&mut harness, "Deleted loner here and on origin.");
    assert_eq!(rev(&sandbox, &mine, "refs/heads/loner"), None);
    let remote = sandbox
        .run(&colleague, "ls-remote", &["--heads", "origin", "loner"])
        .unwrap();
    assert!(remote.stdout.is_empty(), "gone on the remote");
}

#[test]
fn worktrees_patches_and_the_graph_s_scope_come_from_the_menus() {
    let sandbox = Sandbox::new().unwrap();
    let repo = branchy(&sandbox);
    let mut harness = harness(&sandbox);
    let patches = sandbox.path("patches");
    std::fs::create_dir(&patches).unwrap();
    harness.state_mut().state.files = FileDialogs::in_folder(patches.clone());
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // A commit saved as a patch.
    harness
        .get_by_role_and_label(Role::Button, "Add a parser")
        .click_secondary();
    wait_for_label(&mut harness, "Save as a patch…");
    harness.get_by_label("Save as a patch…").click();
    let saved = patches.join("0001-Add-a-parser.patch");
    settle(&mut harness, "the patch saved", |_| saved.exists());
    let patch = std::fs::read_to_string(&saved).unwrap();
    assert!(patch.contains("Subject: [PATCH] Add a parser"), "{patch}");

    // Only the parser's history, then every branch's again.
    menu_item(&mut harness, &badge("parser"), "Show only parser");
    settle(&mut harness, "the parser's history", history_of(3));
    wait_for_label(
        &mut harness,
        &format!("{}  Showing only parser", icon::FUNNEL),
    );
    render(&mut harness, "history-only-parser");
    harness.get_by_label("Show every branch").click();
    settle(&mut harness, "every branch's history", history_of(6));

    // The parser pinned to the left: its tip and first parents in the first lane.
    menu_item(&mut harness, &badge("parser"), "Pin parser to the left");
    wait_for_label(
        &mut harness,
        &format!("{}  parser pinned to the left", icon::PUSH_PIN),
    );
    settle(&mut harness, "the history read again", history_of(6));
    let tip = rev(&sandbox, &repo, "parser").unwrap();
    settle(&mut harness, "the parser in the first lane", |app| {
        let history = &app.state.repos[0].history.shown;
        history.row_of(&tip, 0).is_some_and(|row| {
            history
                .graph()
                .iter()
                .find_map(|chunk| {
                    let index = row.checked_sub(chunk.first_row())?;
                    chunk.row(usize::try_from(index).ok()?)
                })
                .is_some_and(|(row, _)| row.lane == 0)
        })
    });
    render(&mut harness, "history-pinned");
    harness.get_by_label("Unpin").click();
    settle(&mut harness, "unpinned", |app| {
        app.state.repos[0].history.shown.scope().pinned.is_none()
            && !app.state.repos[0].history.is_loading()
    });

    // A worktree from a branch, opened in a tab of its own.
    menu_item(
        &mut harness,
        &badge("parser"),
        "Create a worktree from parser…",
    );
    wait_for_label(&mut harness, "Create a worktree from parser");
    let tree = sandbox.path("parser tree");
    let folder = "Folder (a new or empty one)";
    harness
        .get_by_role_and_label(Role::TextInput, folder)
        .click();
    harness.run_steps(1);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    type_into(
        &mut harness,
        Role::TextInput,
        folder,
        tree.to_str().unwrap(),
    );
    render(&mut harness, "add-worktree");
    harness.get_by_label("Create worktree").click();
    settle(&mut harness, "the worktree's tab", |app| {
        app.state.repos.len() == 2
    });
    assert!(tree.join("parser.rs").exists());
}
