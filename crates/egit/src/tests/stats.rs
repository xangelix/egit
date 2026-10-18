//! The Stats page, driven as a user drives it: opened from the top bar's "Stats: N commits",
//! its figures and charts drawn once the commits are read and their lines counted (authors
//! through the mailmap, files by language), kept when closed and shown again at once.

use egit_git::testing::Sandbox;
use egui_kittest::kittest::Queryable as _;

use super::patches::wait_for_text;
use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};

/// Commits `file` (as `contents`) by `author` on `date`.
fn commit_on(
    sandbox: &Sandbox,
    repo: &std::path::Path,
    (file, contents): (&str, &str),
    author: &str,
    date: &str,
) {
    let path = repo.join(file);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::write(&path, contents).unwrap();
    sandbox.run(repo, "add", &["--", file]).unwrap();
    sandbox
        .run(
            repo,
            "commit",
            &[
                "--quiet",
                "-m",
                &format!("Change {file}"),
                &format!("--author={author}"),
                &format!("--date={date}"),
            ],
        )
        .unwrap();
}

#[test]
fn the_stats_page_draws_the_history_in_figures_and_charts() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    std::fs::write(
        repo.join(".mailmap"),
        "Ada Lovelace <ada@example.com> <ada@old.example>\n",
    )
    .unwrap();
    let ada = "Ada Lovelace <ada@example.com>";
    let old_ada = "Ada <ada@old.example>";
    let grace = "Grace Hopper <grace@example.com>";
    let commits = [
        (
            ("src/main.rs", "fn main() {}\n"),
            ada,
            "2024-01-08T09:15:00+0100",
        ),
        (
            ("README.md", "# Parser\n"),
            grace,
            "2024-01-09T14:30:00-0500",
        ),
        (
            ("src/lib.rs", "pub fn parse() {}\n"),
            old_ada,
            "2024-02-14T10:00:00+0100",
        ),
        (
            ("Cargo.toml", "[package]\n"),
            ada,
            "2024-02-15T10:30:00+0100",
        ),
        (
            ("src/lib.rs", "pub fn parse() -> bool {\n    true\n}\n"),
            grace,
            "2024-04-02T16:45:00-0500",
        ),
        (
            ("docs/guide.md", "Read me.\n"),
            ada,
            "2024-06-21T22:05:00+0100",
        ),
        (
            ("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n"),
            ada,
            "2025-03-03T08:00:00+0100",
        ),
    ];
    for (file, author, date) in commits {
        commit_on(&sandbox, &repo, file, author, date);
    }
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    harness.get_by_label("Stats: 7 commits").click();
    wait_for_label(&mut harness, "Commits each month");
    // Ada under both of her names is one author.
    wait_for_label(&mut harness, "Ada Lovelace");
    assert!(harness.query_all_by_label("Ada").next().is_none());
    wait_for_label(&mut harness, "authors");
    wait_for_text(&mut harness, "in every commit");
    wait_for_label(&mut harness, "Rust");
    render(&mut harness, "stats");
    // The whole page at once, in a window wide enough for cards in pairs (each pair as tall as
    // each other) and tall enough.
    harness.set_size(egui::vec2(1_900.0, 2_000.0));
    harness.run_steps(4);
    render(&mut harness, "stats-whole");
    // A bar's figures, in a tooltip at the pointer.
    let chart = harness
        .query_all_by_label("Commits each month")
        .max_by(|a, b| a.rect().height().total_cmp(&b.rect().height()))
        .unwrap()
        .rect();
    harness.hover_at(egui::pos2(chart.left() + 70.0, chart.center().y));
    harness.run_steps(3);
    assert!(
        harness
            .query_all_by_label_contains("2024: ")
            .next()
            .is_some(),
        "the month's figures"
    );
    render(&mut harness, "stats-hover");
    harness.set_size(egui::vec2(1_180.0, 800.0));
    harness.run_steps(3);

    // Closed and opened again: shown at once, nothing read again.
    harness.get_by_label("Stats: 7 commits").click();
    wait_for_label(&mut harness, "Change docs/guide.md");
    harness.get_by_label("Stats: 7 commits").click();
    harness.run_steps(2);
    assert!(
        harness
            .query_all_by_label("Commits each month")
            .next()
            .is_some()
    );
}
