//! People, driven as a user meets them: co-authors picked from the recent authors or typed,
//! written as `Co-authored-by` trailers, shown in the details with their avatars, and taken
//! back into the form when the commit is amended.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_of, history_shown, open_path, profile_loaded, render, settle, type_into,
    uncommitted, wait_for_label,
};
use super::search::details_shown;

/// A repository of three commits, two by Ada Lovelace and one by Alan Turing.
fn by_two(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("repo").unwrap();
    for (file, author) in [
        ("a", "Ada Lovelace <ada@example.com>"),
        ("b", "Alan Turing <alan@example.com>"),
        ("c", "Ada Lovelace <ada@example.com>"),
    ] {
        std::fs::write(repo.join(file), "x\n").unwrap();
        sandbox.run(&repo, "add", &[file]).unwrap();
        sandbox
            .run(
                &repo,
                "commit",
                &["-m", file, &format!("--author={author}")],
            )
            .unwrap();
    }
    repo
}

#[test]
fn co_authors_are_picked_committed_shown_and_amended() {
    let sandbox = Sandbox::new().unwrap();
    let repo = by_two(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    std::fs::write(repo.join("a"), "pair\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage all");
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 1 file");
    type_into(&mut harness, Role::TextInput, "Summary", "Pair on it");

    // Offered from the recent authors, most commits first.
    harness.get_by_label("Add a co-author").click();
    wait_for_label(&mut harness, "Ada Lovelace <ada@example.com>");
    wait_for_label(&mut harness, "Alan Turing <alan@example.com>");
    render(&mut harness, "co-author-picker");
    harness
        .get_by_label("Ada Lovelace <ada@example.com>")
        .click();
    wait_for_label(&mut harness, "Remove Ada Lovelace as a co-author");
    // Someone new, typed.
    harness.get_by_label("Add a co-author").click();
    harness.run_steps(2);
    type_into(
        &mut harness,
        Role::TextInput,
        "Who wrote it with you",
        "Grace Hopper <grace@example.com>",
    );
    wait_for_label(&mut harness, "Grace Hopper <grace@example.com>");
    harness
        .get_by_label("Grace Hopper <grace@example.com>")
        .click();
    wait_for_label(&mut harness, "Remove Grace Hopper as a co-author");
    render(&mut harness, "commit-co-authors");

    harness.get_by_label("Commit 1 file").click();
    settle(&mut harness, "the commit shown", history_of(4));
    let message = sandbox
        .run(&repo, "log", &["-1", "--format=%B"])
        .unwrap()
        .stdout;
    let message = String::from_utf8(message).unwrap();
    assert!(message.starts_with("Pair on it\n\n"), "{message}");
    assert!(
        message.contains("Co-authored-by: Ada Lovelace <ada@example.com>\n")
            && message.contains("Co-authored-by: Grace Hopper <grace@example.com>\n"),
        "{message}"
    );

    // The details name them, each with an avatar.
    harness
        .get_by_role_and_label(Role::Button, "Pair on it")
        .click();
    settle(&mut harness, "the details", details_shown);
    wait_for_label(&mut harness, "Co-authors");
    wait_for_label(&mut harness, "Grace Hopper");
    render(&mut harness, "details-co-authors");

    // Amending takes them back into the form, out of the description.
    std::fs::write(repo.join("b"), "again\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage all");
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 1 file");
    harness.get_by_label("Amend").click();
    wait_for_label(&mut harness, "Remove Grace Hopper as a co-author");
    wait_for_label(&mut harness, "Remove Ada Lovelace as a co-author");
    let description = harness
        .get_by_role_and_label(Role::MultilineTextInput, "Description")
        .value()
        .unwrap_or_default();
    assert!(!description.contains("Co-authored-by"), "{description}");
    harness
        .get_by_label("Remove Ada Lovelace as a co-author")
        .click();
    harness.run_steps(2);
    harness.get_by_label("Amend the last commit").click();
    settle(&mut harness, "the amended commit", |_| {
        sandbox
            .run(&repo, "show", &["HEAD:b"])
            .is_ok_and(|out| out.stdout == b"again\n")
    });
    let message = sandbox
        .run(&repo, "log", &["-1", "--format=%B"])
        .unwrap()
        .stdout;
    let message = String::from_utf8(message).unwrap();
    assert_eq!(
        message.matches("Co-authored-by: Grace Hopper").count(),
        1,
        "{message}"
    );
    assert!(!message.contains("Ada Lovelace"), "{message}");
}
