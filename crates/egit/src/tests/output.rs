//! Hooks seen from the window: a commit's pre-commit hook running (the commit form says so,
//! the output opens and shows its lines as they come, and closes after it went well), refusing
//! (the problem, the output kept open), skipped (`--no-verify`: the box staying ticked for the
//! tab, a merge skipping its own), and cancelled part-way (no commit made); the Overview lists
//! the hooks.

#![cfg(unix)]

use std::path::Path;

use egit_git::testing::{Sandbox, set_mode};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use egit_git::Checks;

use super::screens::{
    harness, history_shown, menu_item, open_path, profile_loaded, render, settle, type_into,
    uncommitted, wait_for_label,
};
use crate::app::EgitApp;
use crate::theme::icon;

/// Makes `script` the repository's pre-commit hook.
fn pre_commit(repo: &Path, script: &str) {
    hook(repo, "pre-commit", script);
}

/// Makes `script` the repository's hook `name`.
fn hook(repo: &Path, name: &str, script: &str) {
    let path = repo.join(".git/hooks").join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    set_mode(&path, 0o755).unwrap();
}

/// `HEAD`'s subject.
fn head(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox.run(repo, "log", &["-1", "--format=%s"]).unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Changes `file`, stages it in the window, and types `summary`.
fn ready_to_commit(harness: &mut Harness<'_, EgitApp>, repo: &Path, file: &str, summary: &str) {
    std::fs::write(repo.join(file), summary).unwrap();
    wait_for_label(harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(harness, "Stage all");
    harness.get_by_label("Stage all").click();
    wait_for_label(harness, "Commit 1 file");
    type_into(harness, Role::TextInput, "Summary", summary);
}

/// Steps until no node is named `label` (the output closed, say), for 20 s at most: what waits
/// on the clock (the output closes 1.5 s after a run went well) waits on the real one.
fn wait_gone(harness: &mut Harness<'_, EgitApp>, label: &str) {
    let now = std::time::Instant::now();
    let deadline = now
        .checked_add(std::time::Duration::from_secs(20))
        .unwrap_or(now);
    while std::time::Instant::now() < deadline {
        if harness.query_all_by_label(label).next().is_none() {
            return;
        }
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("{label:?} stayed");
}

#[test]
fn a_commit_s_hooks_show_as_they_run_refuse_are_skipped_and_cancelled() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "x\n", "First").unwrap();
    pre_commit(&repo, "echo 'Checking style…'; sleep 3; echo 'All good'");
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // The form says which hooks run; committing, they run, their lines show as they come.
    ready_to_commit(&mut harness, &repo, "b", "Checked");
    wait_for_label(&mut harness, "pre-commit run first");
    harness.get_by_label("Commit 1 file").click();
    wait_for_label(&mut harness, "Checking style…");
    assert!(
        harness
            .query_all_by_label_contains("Committing: running pre-commit")
            .next()
            .is_some()
    );
    // Gone well: made, and the output it opened for closes again.
    wait_for_label(&mut harness, "All good");
    wait_gone(&mut harness, "All good");
    assert_eq!(head(&sandbox, &repo), "Checked");
    assert!(
        harness
            .query_all_by_label_contains("Committing · done in")
            .next()
            .is_some()
    );

    // Refused: the problem, the output open with what the hook said.
    pre_commit(&repo, "echo 'lint: 2 problems'; exit 1");
    ready_to_commit(&mut harness, &repo, "c", "Refused");
    harness.get_by_label("Commit 1 file").click();
    wait_for_label(&mut harness, "lint: 2 problems");
    settle(&mut harness, "the commit refused", |app| {
        matches!(app.state.repos[0].committing.read_as_ref(), Some(Err(_)))
    });
    harness.run_steps(4);
    assert!(
        harness
            .query_all_by_label("lint: 2 problems")
            .next()
            .is_some(),
        "kept open"
    );
    render(&mut harness, "output-refused");
    assert_eq!(head(&sandbox, &repo), "Checked");

    // Skipped: made without it (the output hidden, for the form's room).
    harness.get_by_label("Hide the output").click();
    // (The form's problem says what the hook said too: the panel's own last line goes.)
    wait_gone(&mut harness, "git commit ended: exit status 1.");
    harness.get_by_label("Skip hooks").click();
    // Ticked, a line under it says what won't run (the button moves down).
    wait_for_label(
        &mut harness,
        "pre-commit won't run, for commits, merges, rebases, and patches here",
    );
    harness.get_by_label("Commit 1 file").click();
    settle(&mut harness, "the commit made", |app| {
        app.state.repos[0].commits_made == 2
    });
    assert_eq!(head(&sandbox, &repo), "Refused");

    // Ticked it stays, for the tab, and a merge skips its own checks too.
    a_merge_skips_its_checks(&mut harness, &sandbox, &repo);

    // Cancelled part-way: no commit.
    // Longer than any screen's checks take: only Cancel ends it.
    pre_commit(&repo, "echo 'Running the tests'; sleep 300");
    ready_to_commit(&mut harness, &repo, "d", "Never");
    harness.get_by_label("Commit 1 file").click();
    wait_for_label(&mut harness, "Running the tests");
    render(&mut harness, "output-running");
    // The output bar's Cancel (the form, beside, has one too).
    harness
        .query_all_by_label("Cancel")
        .min_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .unwrap()
        .click();
    wait_for_label(&mut harness, "Committing · cancelled");
    assert_eq!(head(&sandbox, &repo), "Merge branch 'side'");

    // The Overview lists the hook.
    harness.get_by_label("Repository").click();
    wait_for_label(&mut harness, "Overview");
    harness.get_by_label("Overview").click();
    wait_for_label(&mut harness, "pre-commit");
    render(&mut harness, "overview-hooks");
}

/// With "Skip hooks" ticked (and the form cleared after a commit): it stays so, the warning
/// saying what won't run; a merge goes past a `pre-merge-commit` that refuses; unticked, the
/// hooks run again.
fn a_merge_skips_its_checks(harness: &mut Harness<'_, EgitApp>, sandbox: &Sandbox, repo: &Path) {
    assert_eq!(harness.state().state.repos[0].tasks.checks(), Checks::Skip);
    let skipped = "pre-commit and pre-merge-commit won't run, for commits, merges, rebases, and \
                   patches here";
    hook(repo, "pre-merge-commit", "echo 'no merges'; exit 1");
    sandbox
        .run(repo, "switch", &["--quiet", "--create", "side", "HEAD~1"])
        .unwrap();
    // Made past the hook that refuses.
    std::fs::write(repo.join("e"), "side\n").unwrap();
    sandbox.run(repo, "add", &["e"]).unwrap();
    sandbox
        .run(repo, "commit", &["--quiet", "--no-verify", "-m", "Side"])
        .unwrap();
    sandbox.run(repo, "switch", &["--quiet", "main"]).unwrap();
    // Read again once the window is back (the hooks folder isn't watched).
    harness.state_mut().state.repos[0].refresh_hooks(sandbox.git());
    wait_for_label(harness, skipped);
    render(harness, "commit-hooks-skipped");
    let badge = format!("{} side", icon::GIT_BRANCH);
    wait_for_label(harness, &badge);
    menu_item(harness, &badge, "Merge side into main (no fast-forward)");
    wait_for_label(harness, "Merged side, with a merge commit.");
    assert_eq!(head(sandbox, repo), "Merge branch 'side'");
    std::fs::remove_file(repo.join(".git/hooks/pre-merge-commit")).unwrap();
    harness.state_mut().state.repos[0].refresh_hooks(sandbox.git());
    harness.get_by_label("Skip hooks").click();
    settle(harness, "the hooks run again", |app| {
        app.state.repos[0].tasks.checks() == Checks::Run
    });
}
