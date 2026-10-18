//! Scrolling, frame by frame: what scrolls moves one way while the wheel turns one way, and
//! nothing moves once it stops (at an end, or anywhere), in the lists that scroll inside the
//! panels: a commit's files, the files to stage and the panel around them.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::screens::{
    harness, history_shown, open_path, profile_loaded, settle, uncommitted, wait_for_label,
};
use crate::app::EgitApp;

/// Where every element labelled with a name `named` keeps is, by name: its top.
fn tops(harness: &Harness<'_, EgitApp>, named: &dyn Fn(&str) -> bool) -> Vec<(String, f32)> {
    harness
        .root()
        .children_recursive()
        .filter_map(|node| {
            let access = node.accesskit_node();
            let label = access.label()?;
            named(&label).then(|| (format!("{:?} {label}", access.role()), node.rect().top()))
        })
        .collect()
}

/// Turns the wheel down over `at` `turns` times, then lets it settle; returns, for every frame,
/// where the elements `named` stood (their tops, by name).
fn wheel(
    harness: &mut Harness<'_, EgitApp>,
    at: egui::Pos2,
    turns: usize,
    named: &dyn Fn(&str) -> bool,
) -> Vec<Vec<(String, f32)>> {
    harness.hover_at(at);
    harness.run_steps(2);
    let mut frames = Vec::new();
    for turn in 0..turns.saturating_add(20) {
        if turn < turns {
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -40.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.step();
        frames.push(tops(harness, named));
    }
    frames
}

/// Problems with `frames`: an element moving back up while the wheel turned down, or moving
/// at all in the last frames (the wheel stopped).
fn jitter(frames: &[Vec<(String, f32)>]) -> Vec<String> {
    let mut problems = Vec::new();
    for (index, pair) in frames.windows(2).enumerate() {
        for (name, top) in &pair[1] {
            if let Some((_, before)) = pair[0].iter().find(|(other, _)| other == name)
                && top - before > 0.5
            {
                problems.push(format!(
                    "frame {index}: {name} moved down {:.1}",
                    top - before
                ));
            }
        }
    }
    let still = &frames[frames.len().saturating_sub(10)..];
    for pair in still.windows(2) {
        for (name, top) in &pair[1] {
            if let Some((_, before)) = pair[0].iter().find(|(other, _)| other == name)
                && (top - before).abs() > 0.5
            {
                problems.push(format!("stopped, yet {name} moved {:.1}", top - before));
            }
        }
    }
    problems
}

/// Whether the wheel moved the list (its first row then isn't its first file) and took it to its
/// end (`last` shown).
fn reached_end(frames: &[Vec<(String, f32)>], first: &str, last: &str) -> bool {
    let shown = |frame: &Vec<(String, f32)>, name: &str| {
        frame.iter().any(|(shown, _)| shown.ends_with(name))
    };
    frames.first().is_some_and(|frame| shown(frame, first))
        && frames
            .last()
            .is_some_and(|frame| !shown(frame, first) && shown(frame, last))
}

/// A repository with one commit changing 300 files, and 300 untracked files besides.
fn many_files(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("many").unwrap();
    for at in 0..300 {
        std::fs::write(repo.join(format!("tracked-{at:03}.txt")), "one\n").unwrap();
    }
    sandbox.run(&repo, "add", &["--all"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Many"])
        .unwrap();
    for at in 0..300 {
        std::fs::write(repo.join(format!("new-{at:03}.txt")), "new\n").unwrap();
    }
    repo
}

#[test]
fn lists_scroll_without_jitter() {
    let sandbox = Sandbox::new().unwrap();
    let repo = many_files(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // A commit's 300 files, scrolled in their box.
    harness.get_by_role_and_label(Role::Button, "Many").click();
    wait_for_label(&mut harness, "300 files changed");
    harness.run_steps(4);
    let files = |name: &str| name.starts_with("tracked-");
    let first = harness
        .get_by_role_and_label(Role::Label, "tracked-000.txt")
        .rect()
        .center();
    let frames = wheel(&mut harness, first, 260, &files);
    let problems = jitter(&frames);
    assert!(
        problems.is_empty(),
        "a commit's files:\n{}",
        problems.join("\n")
    );
    assert!(reached_end(&frames, "tracked-000.txt", "tracked-299.txt"));

    // The work tree's 300 new files, scrolled in their list, and past its end.
    harness.get_by_label(&uncommitted(300)).click();
    wait_for_label(&mut harness, "Stage new-000.txt");
    harness.run_steps(4);
    let staging = |name: &str| name.starts_with("new-") || name == "Summary";
    let first = harness
        .get_by_role_and_label(Role::Label, "new-000.txt")
        .rect()
        .center();
    let frames = wheel(&mut harness, first, 260, &staging);
    let problems = jitter(&frames);
    assert!(
        problems.is_empty(),
        "the files to stage:\n{}",
        problems.join("\n")
    );
    assert!(reached_end(&frames, "new-000.txt", "new-299.txt"));

    // The panel around them, wheeled from its commit form: it all fits, so nothing moves.
    let form = harness
        .get_by_role_and_label(Role::TextInput, "Summary")
        .rect()
        .center();
    let frames = wheel(&mut harness, form, 60, &staging);
    let problems = jitter(&frames);
    assert!(problems.is_empty(), "the panel:\n{}", problems.join("\n"));
}
