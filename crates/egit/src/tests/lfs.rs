//! Git LFS, driven as a user meets it: a repository keeping files in LFS that isn't set up
//! here says so and sets it up; its menu and page; a pattern tracked and untracked; its files
//! listed. Skipped without git-lfs, unless `EGIT_REQUIRE_LFS` is set.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, type_into, wait_for_label,
};
use crate::theme::icon;

/// Whether git-lfs is installed; without it the test is skipped, unless it is required.
fn available(sandbox: &Sandbox) -> bool {
    let found = egit_git::lfs_version(sandbox.git()).unwrap().is_some();
    assert!(
        found || std::env::var_os("EGIT_REQUIRE_LFS").is_none(),
        "git-lfs is needed to test Git LFS"
    );
    found
}

#[test]
fn lfs_is_set_up_tracked_and_listed() {
    let sandbox = Sandbox::new().unwrap();
    if !available(&sandbox) {
        return;
    }
    let repo = sandbox.init("repo").unwrap();
    // Tracked, but not set up here (the sandbox has no global filters).
    sandbox.run(&repo, "lfs", &["track", "*.bin"]).unwrap();
    sandbox.run(&repo, "add", &[".gitattributes"]).unwrap();
    sandbox
        .run(&repo, "commit", &["-m", "Track binaries"])
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    let banner = format!(
        "{}  This repository keeps large files in Git LFS, but it isn't set up here.",
        icon::WARNING
    );
    wait_for_label(&mut harness, &banner);
    render(&mut harness, "lfs-not-set-up");
    harness.get_by_label("Set up Git LFS here").click();
    wait_for_label(
        &mut harness,
        "Git LFS is set up here: its files check out whole, and pushes push them.",
    );
    let process = sandbox
        .run(&repo, "config", &["--local", "filter.lfs.process"])
        .unwrap()
        .stdout;
    assert_ne!(process.trim_ascii(), b"");
    settle(&mut harness, "LFS read again", |app| {
        app.state.repos[0]
            .lfs
            .ok_ref()
            .is_some_and(|lfs| lfs.set_up && lfs.trouble().is_none())
    });
    harness.run_steps(2);
    assert!(harness.query_by_label(&banner).is_none(), "mended");

    // A file kept in LFS, committed: listed on the page.
    std::fs::write(repo.join("data.bin"), [0_u8, 1, 2, 3, 255].repeat(1000)).unwrap();
    sandbox.run(&repo, "add", &["data.bin"]).unwrap();
    sandbox.run(&repo, "commit", &["-m", "Data"]).unwrap();
    harness.get_by_label("LFS").click();
    wait_for_label(&mut harness, "Patterns, files, and locks…");
    harness.get_by_label("Patterns, files, and locks…").click();
    wait_for_label(&mut harness, "Files held: 1");
    wait_for_label(&mut harness, "data.bin");
    render(&mut harness, "lfs-page");

    // A pattern tracked, then untracked.
    type_into(&mut harness, Role::TextInput, "Track", "*.psd");
    harness.get_by_role_and_label(Role::Button, "Track").click();
    wait_for_label(&mut harness, "*.psd");
    let attributes = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
    assert!(attributes.contains("*.psd filter=lfs"), "{attributes}");
    harness.run_steps(2);
    let stops = harness.query_all_by_label("Stop tracking").count();
    assert_eq!(stops, 2);
    harness
        .query_all_by_label("Stop tracking")
        .last()
        .unwrap()
        .click();
    settle(&mut harness, "the pattern untracked", |_| {
        std::fs::read_to_string(repo.join(".gitattributes"))
            .is_ok_and(|attributes| !attributes.contains("*.psd"))
    });
}
