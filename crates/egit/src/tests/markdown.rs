//! Markdown previewed, driven as a user drives it: a commit changing a README shows it before
//! and after, rendered; one adding a document shows it as added, one deleting it as it was; a
//! link goes to egit's opener; an image is a link to it, fetched by no one. A long changelog
//! changed far down opens at its first change, and steps to the next and back.

use std::fmt::Write as _;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};
use super::search::details_shown;
use crate::app::EgitApp;

/// Opens the changed file `path` of the commit `subject`, previewed.
fn previewed(harness: &mut Harness<'_, EgitApp>, subject: &str, path: &str) {
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Button, subject).click();
    settle(harness, "the details", details_shown);
    harness.get_by_role_and_label(Role::Button, path).click();
    wait_for_label(harness, "Preview");
    harness.get_by_label("Preview").click();
}

#[test]
fn markdown_files_are_previewed_before_and_after() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(
            &repo,
            "README.md",
            "# Parser\n\nReads *everything* quickly.\n\n- [ ] faster\n",
            "Start",
        )
        .unwrap();
    sandbox
        .commit_file(&repo, "OLD.md", "# Old notes\n\nGone soon.\n", "Notes")
        .unwrap();
    std::fs::write(
        repo.join("README.md"),
        "# Parser\n\nReads *everything* quickly.\n\n## Install\n\n\
         Run `cargo install parser`, or see [the site](https://example.com/site).\n\n\
         | Option | Meaning |\n|---|---|\n| `-q` | quiet |\n\n\
         ![The parser at work](shots/work.png)\n\n- [x] faster\n\nBack to [the install](#install).\n",
    )
    .unwrap();
    std::fs::write(repo.join("GUIDE.md"), "# Guide\n\nStart here.\n").unwrap();
    sandbox.run(&repo, "rm", &["--quiet", "OLD.md"]).unwrap();
    sandbox.run(&repo, "add", &["--all"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "-m", "Write the docs"])
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // Changed: before and after, side by side.
    previewed(&mut harness, "Write the docs", "README.md");
    wait_for_label(&mut harness, "Install");
    wait_for_label(&mut harness, "Before");
    wait_for_label(&mut harness, "After");
    assert!(
        harness.query_all_by_label("Parser").count() >= 2,
        "both sides"
    );
    // The image a link to it; nothing asked to load it.
    wait_for_label(
        &mut harness,
        &format!("{} The parser at work", crate::theme::icon::IMAGE),
    );
    render(&mut harness, "markdown-preview");
    // Wrapped over two lines, its middle is between them: clicked as a screen reader would.
    harness.get_by_label("the site").click_accesskit();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        ["https://example.com/site"]
    );
    // A heading's link scrolls to it, and opens nothing.
    harness.get_by_label("the install").click_accesskit();
    harness.run_steps(2);
    assert_eq!(harness.state().state.opener.recorded().len(), 1);

    // Added: the document alone; deleted: as it was.
    previewed(&mut harness, "Write the docs", "GUIDE.md");
    wait_for_label(&mut harness, "Start here.");
    assert!(harness.query_by_label("Before").is_none());
    previewed(&mut harness, "Write the docs", "OLD.md");
    wait_for_label(&mut harness, "Gone soon.");
    assert!(harness.query_by_label("After").is_none());
}

/// Whether `label` is drawn on screen (some of it, between the window's top and bottom).
fn on_screen(harness: &Harness<'_, EgitApp>, label: &str) -> bool {
    let bottom = harness.ctx.content_rect().bottom();
    harness
        .query_all_by_label(label)
        .any(|node| node.rect().bottom() > 0.0 && node.rect().top() < bottom)
}

#[test]
fn a_change_far_down_is_shown_and_stepped_to() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let changelog = |fixed: &str, added: bool| {
        let mut text = String::from("# Changelog\n\n");
        for version in (1..=80).rev() {
            if added && version == 20 {
                text.push_str("## v0.20.1\n\n- Patched the patch.\n\n");
            }
            let line = if version == 50 {
                fixed.to_owned()
            } else {
                format!("Fixed thing {version}.")
            };
            write!(text, "## v0.{version}.0\n\n- {line}\n\n").unwrap();
        }
        text
    };
    sandbox
        .commit_file(
            &repo,
            "CHANGELOG.md",
            &changelog("Fixed thing 50.", false),
            "Start",
        )
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "CHANGELOG.md",
            &changelog("Fixed thing 50 for good.", true),
            "Release",
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // It opens at the first change, far below the top: both sides of it in view.
    previewed(&mut harness, "Release", "CHANGELOG.md");
    wait_for_label(&mut harness, "Change 1 of 2");
    settle(&mut harness, "the previews measured", |app| {
        app.state.repos[0].history_view.previews_settled()
    });
    assert!(on_screen(&harness, "Fixed thing 50 for good."));
    assert!(on_screen(&harness, "Fixed thing 50."));
    assert!(!on_screen(&harness, "Changelog"));
    render(&mut harness, "markdown-changes");

    // The next: the section added, after.
    harness.get_by_label("Next change").click();
    harness.run_steps(4);
    wait_for_label(&mut harness, "Change 2 of 2");
    assert!(on_screen(&harness, "Patched the patch."));
    assert!(!on_screen(&harness, "Fixed thing 50 for good."));
    // And back, by the keys.
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::ArrowUp);
    harness.run_steps(4);
    wait_for_label(&mut harness, "Change 1 of 2");
    assert!(on_screen(&harness, "Fixed thing 50 for good."));
}
