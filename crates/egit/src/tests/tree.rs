//! Changed files as a tree of their folders: chosen in a list's header (and kept), folders
//! folded and unfolded, a folder staged whole, and a commit's files in the details alike.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_of, history_shown, open_path, profile_loaded, render, settle, type_into,
    uncommitted, wait_for_label,
};
use super::search::details_shown;

#[test]
fn changed_files_show_as_a_tree_of_their_folders() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "README", "x\n", "Start")
        .unwrap();
    for path in [
        "src/core/a.c",
        "src/core/b.c",
        "src/main.rs",
        "docs/guide/intro.md",
    ] {
        let file = repo.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "new\n").unwrap();
    }
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for_label(&mut harness, &uncommitted(4));
    harness.get_by_label(&uncommitted(4)).click();
    wait_for_label(&mut harness, "Stage all");

    harness
        .get_by_label("Show the files in a tree of their folders")
        .click();
    // A lone folder holding a folder shows as one; folders count their files.
    wait_for_label(&mut harness, "Stage the folder docs/guide");
    wait_for_label(&mut harness, "Stage the folder src");
    wait_for_label(&mut harness, "Stage the folder src/core");
    render(&mut harness, "work-tree-as-a-tree");

    // Folded, a folder's files are gone from the list; unfolded, back.
    harness.get_by_label("src/core/, 2 files").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Stage src/core/a.c").is_none());
    harness.get_by_label("src/core/, 2 files").click();
    wait_for_label(&mut harness, "Stage src/core/a.c");

    // A folder staged whole.
    harness.get_by_label("Stage the folder src").click();
    wait_for_label(&mut harness, "Unstage the folder src");
    let staged = sandbox
        .run(&repo, "diff", &["--cached", "--name-only"])
        .unwrap()
        .stdout;
    assert_eq!(staged, b"src/core/a.c\nsrc/core/b.c\nsrc/main.rs\n");

    // Committed, its files in the details show as a tree too.
    type_into(&mut harness, Role::TextInput, "Summary", "Add the sources");
    harness.get_by_label("Commit 3 files").click();
    settle(&mut harness, "the commit shown", history_of(2));
    harness
        .get_by_role_and_label(Role::Button, "Add the sources")
        .click();
    settle(&mut harness, "the details", details_shown);
    wait_for_label(&mut harness, "src/core/, 2 files");
    render(&mut harness, "details-as-a-tree");

    // Back to a list: kept for every list.
    harness.get_by_label("Show the files as a list").click();
    wait_for_label(&mut harness, "src/core/a.c");
    assert!(
        !crate::preferences::Preferences::of(&harness.ctx)
            .files
            .lists
            .is_tree()
    );
}
