//! Submodules, driven as someone meeting them in a fresh clone: the banner says they aren't set
//! up and mends them; their menu and page; one updated to its branch's latest, the commits that
//! brings, recorded here and shown in its change; and one added.

use std::path::{Path, PathBuf};

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, into_view, open_path, profile_loaded, render, settle, type_into,
    uncommitted, wait_for_label,
};
use crate::theme::icon;

/// A library, an app recording it at `libs/lib`, and a clone of the app (its submodule not set
/// up), all through `file://` (allowed).
fn cloned_app(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
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
    let app = sandbox.init("app").unwrap();
    sandbox.commit_file(&app, "f", "f\n", "Start").unwrap();
    let url = format!("file://{}", library.display());
    sandbox
        .run(&app, "submodule", &["--quiet", "add", &url, "libs/lib"])
        .unwrap();
    sandbox
        .run(&app, "commit", &["-qm", "Add the library"])
        .unwrap();
    let clone = sandbox.path("clone");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", app.to_str().unwrap(), clone.to_str().unwrap()],
        )
        .unwrap();
    (library, clone)
}

fn opened(sandbox: &Sandbox, repo: &Path) -> egui_kittest::Harness<'static, crate::app::EgitApp> {
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    harness
}

#[test]
fn a_fresh_clones_submodules_are_mended_followed_and_recorded() {
    let sandbox = Sandbox::new().unwrap();
    let (library, clone) = cloned_app(&sandbox);
    let mut harness = opened(&sandbox, &clone);

    // Not set up: said, and mended in a click.
    let banner = format!("{}  Submodules: 1 not set up", icon::PACKAGE);
    wait_for_label(&mut harness, &banner);
    render(&mut harness, "submodules-not-set-up");
    harness.get_by_label("Mend everything").click();
    wait_for_label(
        &mut harness,
        "Mended the submodules: addresses synced, each set up and at the commit recorded.",
    );
    assert_eq!(
        std::fs::read_to_string(clone.join("libs/lib/lib")).unwrap(),
        "one\n"
    );

    // Its branch moves on; updated to the latest from its page, it shows what that brings.
    sandbox
        .commit_file(&library, "lib", "two\n", "Two")
        .unwrap();
    harness.get_by_label("Submodules").click();
    wait_for_label(&mut harness, "Open the submodules page");
    harness.get_by_label("Open the submodules page").click();
    wait_for_label(&mut harness, "Every one set up and at the commit recorded.");
    harness
        .get_by_label("libs/lib, at the commit recorded")
        .click();
    wait_for_label(&mut harness, "Update to its branch's latest");
    into_view(&mut harness, "Update to its branch's latest");
    harness
        .get_by_label("Update to its branch's latest")
        .click();
    wait_for_label(&mut harness, "Two");
    // Its outcome under the page, then dismissed: the page's bottom is where it was.
    wait_for_label(
        &mut harness,
        "Brought libs/lib to their branches' latest: record the new commits here (stage them) \
         to keep them.",
    );
    harness
        .query_all_by_label("Dismiss")
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    harness.run_steps(2);
    render(&mut harness, "submodules-page");
    assert_eq!(
        std::fs::read_to_string(clone.join("libs/lib/lib")).unwrap(),
        "two\n"
    );

    // Recorded here: staged, and its change lists the commit it brings.
    into_view(&mut harness, "Record its commit here");
    harness.get_by_label("Record its commit here").click();
    settle(&mut harness, "the pointer staged", |_| {
        sandbox
            .run(&clone, "diff", &["--cached", "--name-only"])
            .is_ok_and(|out| out.stdout == b"libs/lib\n")
    });
    // Read again once staged: at the commit recorded now, so the banner goes.
    settle(&mut harness, "the submodules read again", |app| {
        app.state.repos[0]
            .submodule_states
            .ok_ref()
            .is_some_and(|standing| {
                standing
                    .states
                    .iter()
                    .all(|state| state.checkout == egit_git::Checkout::AtRecorded)
            })
    });
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Unstage libs/lib");
    // The staged list's row, not the sidebar's.
    harness
        .query_all_by_role_and_label(Role::Button, "libs/lib")
        .find(|row| row.rect().left() > 900.0)
        .unwrap()
        .click();
    wait_for_label(&mut harness, "The commit it brings:");
    render(&mut harness, "submodule-move");
}

#[test]
fn a_submodule_is_added_from_its_page() {
    let sandbox = Sandbox::new().unwrap();
    let (library, clone) = cloned_app(&sandbox);
    let mut harness = opened(&sandbox, &clone);
    harness.get_by_label("Repository").click();
    wait_for_label(&mut harness, "Submodules…");
    harness.get_by_label("Submodules…").click();
    wait_for_label(&mut harness, "Add a submodule…");
    harness.get_by_label("Add a submodule…").click();
    wait_for_label(&mut harness, "Add it");
    let url = format!("file://{}", library.display());
    type_into(&mut harness, Role::TextInput, "Address", &url);
    wait_for_label(&mut harness, "Add it");
    harness.get_by_label("Add it").click();
    wait_for_label(
        &mut harness,
        "Added library: .gitmodules and it are staged, to commit.",
    );
    assert!(clone.join("library/lib").exists());
}
