//! Patches driven as a user drives them: a series looked at and applied as commits (signed
//! off), a series stopping at a conflict (how far it got, the patch it stopped at, skipped,
//! aborted), a pasted diff applied as changes, and patches made: a commit's copied, commits
//! saved as a series, the uncommitted changes copied and saved.

use std::path::{Path, PathBuf};

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_of, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};
use crate::app::EgitApp;
use crate::desktop::FileDialogs;
use crate::theme::icon;

fn subjects(sandbox: &Sandbox, repo: &Path, range: &str) -> Vec<String> {
    let out = sandbox.run(repo, "log", &["--format=%s", range]).unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `main` (`f`: a b c); `topic` from it, by Ada, changing b, then c, then adding `g`, saved as
/// patches in `patches/` (then deleted: only the patches carry it).
fn series(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "a\nb\nc\n", "Base")
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "topic"])
        .unwrap();
    for (file, contents, subject) in [
        ("f", "a\nB\nc\n", "Change b"),
        ("f", "a\nB\nC\n", "Change c"),
        ("g", "new\n", "Add g"),
    ] {
        std::fs::write(repo.join(file), contents).unwrap();
        sandbox.run(&repo, "add", &[file]).unwrap();
        sandbox
            .run(
                &repo,
                "commit",
                &[
                    "--quiet",
                    "--author",
                    "Ada Lovelace <ada@example.org>",
                    "--message",
                    subject,
                ],
            )
            .unwrap();
    }
    let patches = sandbox.path("patches");
    sandbox
        .run(
            &repo,
            "format-patch",
            &["--quiet", "-o", patches.to_str().unwrap(), "main"],
        )
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .run(
            &repo,
            "branch",
            &["--quiet", "--delete", "--force", "topic"],
        )
        .unwrap();
    (repo, patches)
}

/// The app, `repo` opened, its file dialogs answered by `folder`.
fn opened(sandbox: &Sandbox, repo: &Path, folder: &Path) -> Harness<'static, EgitApp> {
    let mut harness = harness(sandbox);
    harness.state_mut().state.files = FileDialogs::in_folder(folder.to_owned());
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, repo);
    settle(&mut harness, "the history", history_shown);
    harness
}

fn patches_menu(harness: &mut Harness<'_, EgitApp>, item: &str) {
    // By the control, not a point on screen: the top bar may settle its look the frame after
    // a task ends.
    harness.get_by_label("Patches").click_accesskit();
    wait_for_label(harness, item);
    harness.get_by_label(item).click();
}

/// Steps until a label holds `text`.
pub(super) fn wait_for_text(harness: &mut Harness<'_, EgitApp>, text: &str) {
    for _ in 0..2_000 {
        harness.step();
        if harness.query_all_by_label_contains(text).next().is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("timed out waiting for a label holding {text:?}");
}

/// The text the last task copied (the clipboard took it in the frame it ended).
fn copied(harness: &mut Harness<'_, EgitApp>) -> Option<String> {
    match harness.state_mut().state.repos[0].tasks.result.ok_ref() {
        Some(crate::tasks::Outcome::Copied(text)) => Some(text.to_string()),
        _ => None,
    }
}

#[test]
fn a_series_is_looked_at_then_applied_as_commits() {
    let sandbox = Sandbox::new().unwrap();
    let (repo, patches) = series(&sandbox);
    let mut harness = opened(&sandbox, &repo, &patches);

    patches_menu(&mut harness, "Apply patches from files…");
    wait_for_label(&mut harness, "Apply patches to main");
    wait_for_label(&mut harness, "3 commits by mail, changing 2 files");
    harness.get_by_label("A series: each commit is checked as it is applied.");
    harness.get_by_label("Change b");
    harness.get_by_label("Add g");
    render(&mut harness, "patches-apply");
    harness
        .get_by_label("Add a Signed-off-by line to each")
        .click();
    harness.run_steps(1);
    harness.get_by_label("Apply to main").click();
    wait_for_label(&mut harness, "Applied 3 commits.");
    assert_eq!(
        subjects(&sandbox, &repo, "main~3..main"),
        ["Add g", "Change c", "Change b"]
    );
    let message = sandbox
        .run(&repo, "log", &["-1", "--format=%an%n%b"])
        .unwrap();
    let message = String::from_utf8(message.stdout).unwrap();
    assert!(message.starts_with("Ada Lovelace"), "{message}");
    assert!(message.contains("Signed-off-by: egit tests"), "{message}");
}

#[test]
fn a_series_stopping_at_a_conflict_is_shown_skipped_and_aborted() {
    let sandbox = Sandbox::new().unwrap();
    let (repo, patches) = series(&sandbox);
    sandbox
        .commit_file(&repo, "f", "a\nb\nX\n", "Change c differently")
        .unwrap();
    let mut harness = opened(&sandbox, &repo, &patches);

    patches_menu(&mut harness, "Apply patches from files…");
    wait_for_label(&mut harness, "Apply to main");
    harness.get_by_label("Apply to main").click();
    let stopped = |at: &str| format!("{}  Applying patches stopped at {at}", icon::WARNING);
    wait_for_label(&mut harness, &stopped("1 of 3: Change b"));
    render(&mut harness, "patches-stopped");

    // The patch it stopped at, to look at.
    harness.get_by_label("Show the patch").click();
    wait_for_label(&mut harness, "The patch git am stopped at");
    wait_for_label(&mut harness, "Changes to 1 file");
    harness
        .get_by_label(&format!("{}  History", icon::ARROW_LEFT))
        .click();
    harness.run_steps(2);

    // Skipped: the next conflicts too. Aborted: as it was.
    harness.get_by_label("Skip").click();
    wait_for_label(&mut harness, &stopped("2 of 3: Change c"));
    harness.get_by_label("Abort…").click();
    wait_for_label(&mut harness, "Abort");
    harness.get_by_role_and_label(Role::Button, "Abort").click();
    settle(&mut harness, "the operation gone", |app| {
        app.state.repos[0]
            .status
            .ok_ref()
            .is_some_and(|status| status.operations.is_empty())
    });
    assert_eq!(
        subjects(&sandbox, &repo, "main~1..main"),
        ["Change c differently"]
    );
}

#[test]
fn a_pasted_diff_applies_as_changes_and_changes_become_patches() {
    let sandbox = Sandbox::new().unwrap();
    let (repo, _) = series(&sandbox);
    let saved = sandbox.path("saved");
    std::fs::create_dir(&saved).unwrap();
    std::fs::write(repo.join("f"), "a\nb\nc\nd\n").unwrap();
    sandbox.run(&repo, "add", &["f"]).unwrap();
    let mut harness = opened(&sandbox, &repo, &saved);
    settle(&mut harness, "the staged change read", |app| {
        app.state.repos[0]
            .work_tree
            .status()
            .and_then(Result::ok)
            .is_some_and(|status| !status.is_empty())
    });

    // The staged change, copied as a patch; then gone, and pasted back.
    patches_menu(&mut harness, "Copy the staged changes as a patch");
    wait_for_label(&mut harness, "Copied the staged changes as a patch.");
    let patch = copied(&mut harness).expect("a patch copied");
    assert!(
        patch.contains("diff --git a/f b/f") && patch.contains("+d"),
        "{patch}"
    );
    patches_menu(&mut harness, "Save every change as a patch…");
    wait_for_text(&mut harness, "Saved the patch to ");
    let every = std::fs::read_to_string(saved.join("changes.patch")).unwrap();
    assert_eq!(every, patch, "nothing else changed");
    sandbox.run(&repo, "reset", &["--quiet", "--hard"]).unwrap();

    patches_menu(&mut harness, "Paste a patch to apply…");
    wait_for_label(&mut harness, "Apply a pasted patch to main");
    harness
        .get_by_role_and_label(
            Role::MultilineTextInput,
            "Paste a patch: commits by mail (git format-patch's), or a diff.",
        )
        .click();
    harness.run_steps(1);
    harness.event(egui::Event::Paste(patch));
    wait_for_label(&mut harness, "Changes to 1 file");
    wait_for_label(
        &mut harness,
        &format!("{}  Applies cleanly here.", icon::CHECK_CIRCLE),
    );
    render(&mut harness, "patches-pasted");
    harness.get_by_label("Apply to main").click();
    wait_for_label(&mut harness, "Applied the changes, staged.");
    let staged = sandbox.run(&repo, "diff", &["--cached"]).unwrap();
    assert!(String::from_utf8(staged.stdout).unwrap().contains("+d"));

    // A commit's patch copied; the commits from one up to HEAD saved as a series.
    settle(&mut harness, "the history", history_of(1));
    harness
        .get_by_role_and_label(Role::Button, "Base")
        .click_secondary();
    wait_for_label(&mut harness, "Copy as a patch");
    harness.get_by_label("Copy as a patch").click();
    wait_for_label(
        &mut harness,
        &format!("Copied {}'s patch.", rev8(&sandbox, &repo)),
    );
    assert!(
        copied(&mut harness)
            .unwrap()
            .contains("Subject: [PATCH] Base")
    );
    harness
        .get_by_role_and_label(Role::Button, "Base")
        .click_secondary();
    wait_for_label(&mut harness, "Save this and later commits as patches…");
    harness
        .get_by_label("Save this and later commits as patches…")
        .click();
    wait_for_text(&mut harness, "Saved 1 patch to ");
    assert!(saved.join("0001-Base.patch").exists());
}

/// `main`'s commit's name, as the history shows it.
fn rev8(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox
        .run(repo, "rev-parse", &["--short=8", "main"])
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
