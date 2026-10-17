//! The history at a large repository's scale, measured: how soon the first rows show, how long
//! the whole history takes, and how long frames take while jumping all over it and scrolling;
//! then searching it (messages, authors, a path, a name): how soon the first commit is found,
//! how long it takes, and the memory it holds while it runs and after; and how soon it is shown
//! narrowed to the current branch and pinned, then whole again.
//!
//! Not run by default, as it needs a large repository (Linux's, say) and a release build:
//!
//! ```text
//! EGIT_SCALE_REPO=~/src/linux cargo test -p egit --release -- --ignored --nocapture scale
//! ```
//!
//! Frames are measured on the CPU, without rendering (tessellation and the GPU come on top).
//! Where screens are rendered, the top and the middle of the history are saved as
//! `target/gui-screens/scale-top.png` and `scale-middle.png`.

// A measuring tool: it prints what it measures, worked out from small known numbers.
#![allow(
    clippy::print_stdout,
    clippy::integer_division,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used
)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egit_git::{ConfigPlace, ConfigSnapshot, Git, GitOptions, Repository};
use egui_kittest::Harness;

use super::screens::{rendering, screens};
use crate::app::{Action, EgitApp};
use crate::history::{Names, Scope};
use crate::search::{self, Asked, SearchKind};
use crate::settings::Settings;
use crate::views::prompts::Prompts;

/// Steps until `done` holds, or panics after `limit`.
pub(super) fn step_until(
    harness: &mut Harness<'_, EgitApp>,
    limit: Duration,
    done: impl Fn(&mut EgitApp) -> bool,
) {
    let start = Instant::now();
    while !done(harness.state_mut()) {
        assert!(start.elapsed() < limit, "timed out");
        harness.step();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Starts the peak resident memory over from what the process holds now (Linux's
/// `clear_refs`), so what a measurement set up beforehand isn't counted.
pub(super) fn reset_peak_memory() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// The process's peak resident memory, in MB.
pub(super) fn peak_memory_mb() -> f64 {
    memory_mb("VmHWM:")
}

/// One of the process's `/proc/self/status` memory figures (`field`, with its colon), in MB.
fn memory_mb(field: &str) -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|value| value.trim().trim_end_matches(" kB").parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Frame times, slowest first.
fn report(what: &str, mut frames: Vec<Duration>) -> Duration {
    frames.sort_unstable_by(|a, b| b.cmp(a));
    let p99 = frames.get(frames.len() / 100).copied().unwrap_or_default();
    let total: Duration = frames.iter().sum();
    println!(
        "{what}: {} frames, slowest {:?}, p99 {p99:?}, mean {:?}",
        frames.len(),
        frames.first().copied().unwrap_or_default(),
        total / u32::try_from(frames.len().max(1)).unwrap()
    );
    p99
}

#[test]
#[ignore = "needs a large repository: set EGIT_SCALE_REPO"]
fn a_large_history_opens_at_once_and_scrolls_within_a_frame() {
    let Some(repo) = std::env::var_os("EGIT_SCALE_REPO").map(PathBuf::from) else {
        println!("EGIT_SCALE_REPO isn't set");
        return;
    };
    let git = Git::new(GitOptions::default()).unwrap();
    let mut harness =
        Harness::builder()
            .with_size([1600.0, 1000.0])
            .build_eframe(move |creation| {
                EgitApp::with_git(
                    &creation.egui_ctx,
                    git,
                    Settings::default(),
                    None,
                    Prompts::none(),
                )
            });
    harness.run_steps(2);
    let start = Instant::now();
    harness
        .state_mut()
        .state
        .actions
        .push(Action::Open(repo.clone()));
    harness.step();
    step_until(&mut harness, Duration::from_secs(60), |app| {
        app.state
            .repos
            .first()
            .is_some_and(|repo| repo.history.shown.rows() > 0)
    });
    println!("first rows after {:?}", start.elapsed());
    step_until(&mut harness, Duration::from_secs(600), |app| {
        !app.state.repos[0].history.is_loading()
    });
    let rows = harness.state().state.repos[0].history.shown.rows();
    println!("{rows} rows after {:?}", start.elapsed());
    if rendering() {
        for (row, name) in [(0, "scale-top"), (rows / 2, "scale-middle")] {
            harness.state_mut().state.repos[0]
                .history_view
                .scroll_to(row);
            harness.run_steps(3);
            step_until(&mut harness, Duration::from_secs(10), |app| {
                !app.state.repos[0].history.summaries.is_reading()
            });
            harness.run_steps(3);
            let image = harness.render().unwrap();
            image.save(screens().join(format!("{name}.png"))).unwrap();
        }
    }

    // Jumps all over the history: each replays a chunk and reads new summaries.
    let mut frames = Vec::new();
    for jump in 0..400_u64 {
        let row = rows.saturating_sub(1) * jump / 399;
        harness.state_mut().state.repos[0]
            .history_view
            .scroll_to(row);
        for _ in 0..3 {
            let frame = Instant::now();
            harness.step();
            frames.push(frame.elapsed());
        }
    }
    let jumps = report("jumping", frames);

    // Wheel scrolling down from the middle.
    harness.state_mut().state.repos[0]
        .history_view
        .scroll_to(rows / 2);
    harness.run_steps(3);
    harness.hover_at(egui::pos2(800.0, 500.0));
    let mut frames = Vec::new();
    for _ in 0..600 {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        let frame = Instant::now();
        harness.step();
        frames.push(frame.elapsed());
    }
    let scrolling = report("scrolling", frames);
    // Packs gitoxide read are mapped into the process: resident, but page cache, as they were
    // in `cat-file`'s process when git read them.
    println!(
        "peak memory {:.0} MB; now {:.0} MB allocated, {:.0} MB of files mapped",
        peak_memory_mb(),
        memory_mb("RssAnon:"),
        memory_mb("RssFile:")
    );
    if !cfg!(debug_assertions) {
        let budget = Duration::from_millis(8);
        assert!(jumps < budget && scrolling < budget, "frames over 8 ms");
    }
    let names = harness.state().state.repos[0].history.shown.names();
    searches(&repo, &names);
    scoped(&mut harness, &repo);
}

/// The history narrowed to the current branch and pinned, then every branch's again: how soon
/// each read again takes over.
fn scoped(harness: &mut Harness<'_, EgitApp>, repo: &std::path::Path) {
    let git = Git::new(GitOptions::default()).unwrap();
    let branch = git
        .read("symbolic-ref")
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .unwrap()
        .stdout;
    let branch = bstr::BString::from(branch.trim_ascii());
    for (what, scope) in [
        (
            "only the current branch, pinned",
            Scope {
                only: vec![branch.clone()],
                pinned: Some(branch),
            },
        ),
        ("every branch again", Scope::default()),
    ] {
        let started = Instant::now();
        harness.state_mut().state.repos[0].set_scope(&git, scope);
        harness.step();
        step_until(harness, Duration::from_secs(600), |app| {
            !app.state.repos[0].history.is_loading()
        });
        let rows = harness.state().state.repos[0].history.shown.rows();
        println!("{what}: {rows} rows after {:?}", started.elapsed());
    }
}

/// Searches the history, each kind in turn, printing how soon the first commit was found, how
/// long it all took, and the memory held at its end and once it was dropped.
fn searches(repo: &std::path::Path, names: &Names) {
    let git = Git::new(GitOptions::default()).unwrap();
    let repository = Arc::new(Repository::discover(&git, repo).unwrap());
    let config =
        ConfigSnapshot::read(&git, ConfigPlace::Repository(repository.work_tree())).unwrap();
    let index = Arc::new(OnceLock::new());
    let ctx = egui::Context::default();
    let abbreviated = names
        .id(names.rows() / 2)
        .unwrap()
        .abbreviated(9)
        .to_string();
    for (kind, query) in [
        (SearchKind::Message, "memory leak"),
        (SearchKind::Message, "xyzzy-nowhere"),
        (SearchKind::Author, "torvalds"),
        (SearchKind::Path, "MAINTAINERS"),
        (SearchKind::Commit, abbreviated.as_str()),
        (SearchKind::Commit, "HEAD~1000"),
    ] {
        reset_peak_memory();
        let started = Instant::now();
        let search = search::start(
            Asked {
                git: &git,
                repository: &repository,
                names: names.clone(),
                index: &index,
                every_tree: config.has_every_tree(),
                kind,
                query,
            },
            &ctx,
        );
        let mut first = None;
        while !search.found().is_done() {
            if first.is_none() && search.found().count() > 0 {
                first = Some(started.elapsed());
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        let all = started.elapsed();
        let outcome = search.found().problem().map_or_else(
            || format!("{} found", search.found().count()),
            |problem| problem.title,
        );
        // The repository gitoxide read for it is closed by now: its packs unmapped.
        println!(
            "search {kind:?} {query:?}: {outcome}, the first after {}, all in {all:?}; peak \
             {:.0} MB, after it {:.0} MB allocated, {:.0} MB of files mapped",
            first.map_or_else(|| "-".to_owned(), |first| format!("{first:?}")),
            peak_memory_mb(),
            memory_mb("RssAnon:"),
            memory_mb("RssFile:")
        );
    }
}
