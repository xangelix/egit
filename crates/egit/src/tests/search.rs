//! Searching the history: the search bar driven as a user drives it (by message, author, path,
//! and name; stepping through what it found both ways), and egit's own reading of messages and
//! authors held to what git's `rev-list --grep` and `--author` find.

// Made-up histories are counted out plainly.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egit_git::testing::Sandbox;
use egit_git::{CancelToken, CommitFilter, ObjectId, Repository, find_commits};
use egit_gix::GixRepository;
use egit_text::matches_case;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    branchy, harness, harness_with, history_shown, open_path, profile_loaded, render, settle,
    wait_for_label,
};
use crate::app::EgitApp;
use crate::history::{History, Objects, Scope};
use crate::search::{self, Asked, Found, SearchKind};
use crate::theme::icon;

/// The search bar's name (its field's and its kind's).
fn bar_label() -> String {
    format!("{}  Search", icon::MAGNIFYING_GLASS)
}

/// Types `query` into the search bar, its field emptied first.
fn search_for(harness: &mut Harness<'_, EgitApp>, query: &str) {
    let label = bar_label();
    harness
        .get_by_role_and_label(Role::TextInput, &label)
        .click();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, &bar_label())
        .type_text(query);
    harness.run_steps(2);
}

/// Picks what the search looks at.
fn search_by(harness: &mut Harness<'_, EgitApp>, kind: &str) {
    harness
        .get_by_role_and_label(Role::ComboBox, &bar_label())
        .click();
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Button, kind).click();
    harness.run_steps(2);
}

/// Whether the search ended, having found `commits`.
fn found(commits: u64) -> impl Fn(&mut EgitApp) -> bool {
    move |app| app.state.repos[0].history_view.search_found() == Some(commits)
}

/// Whether the selected commit's details, and their lines counted, are shown.
pub(super) fn details_shown(app: &mut EgitApp) -> bool {
    let view = &mut app.state.repos[0].history_view;
    view.details_shown() && view.counts_shown()
}

/// The subject of the commit selected.
fn selected(harness: &Harness<'_, EgitApp>, sandbox: &Sandbox, repo: &Path) -> String {
    let app = harness.state();
    let row = app.state.repos[0].history_view.selected_row().unwrap();
    let id = app.state.repos[0].history.shown.id(row).unwrap();
    let subject = sandbox
        .run(repo, "log", &["-1", "--format=%s", &id.to_string()])
        .unwrap();
    String::from_utf8(subject.stdout).unwrap().trim().to_owned()
}

#[test]
fn the_history_is_searched_by_message_author_path_and_name() {
    let sandbox = Sandbox::new().unwrap();
    let repo = branchy(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // ⌘F opens the bar; the first of what it finds is selected as it comes.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    search_for(&mut harness, "parser");
    wait_for_label(&mut harness, "1 of 3");
    let first = selected(&harness, &sandbox, &repo);
    harness.key_press(egui::Key::Enter);
    wait_for_label(&mut harness, "2 of 3");
    harness.key_press(egui::Key::Enter);
    wait_for_label(&mut harness, "3 of 3");
    harness.key_press(egui::Key::Enter);
    wait_for_label(&mut harness, "1 of 3");
    assert_eq!(
        selected(&harness, &sandbox, &repo),
        first,
        "it wraps around"
    );
    harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    wait_for_label(&mut harness, "3 of 3");
    harness.get_by_label("Previous").click();
    wait_for_label(&mut harness, "2 of 3");
    let subjects: BTreeSet<String> = (0..3)
        .map(|_| {
            harness.get_by_label("Next").click();
            harness.run_steps(2);
            selected(&harness, &sandbox, &repo)
        })
        .collect();
    assert_eq!(
        subjects,
        BTreeSet::from(
            ["Add a parser", "Make the parser answer", "Merge the parser"].map(str::to_owned)
        )
    );
    settle(&mut harness, "the commit's details", details_shown);
    render(&mut harness, "history-search");

    // A capital matches case.
    search_for(&mut harness, "Parser");
    wait_for_label(&mut harness, "No commits match");

    search_by(&mut harness, "Author");
    search_for(&mut harness, "tests@egit");
    settle(&mut harness, "every commit found", found(6));

    // The commits that changed a file, as `git log` lists them: the side it came from.
    search_by(&mut harness, "Path");
    search_for(&mut harness, "parser.rs");
    settle(&mut harness, "the file's commits found", found(2));

    // A commit by an abbreviated name, and by a tag.
    search_by(&mut harness, "Commit");
    let greet = sandbox
        .run(&repo, "rev-parse", &["--short=6", "main~1"])
        .unwrap();
    search_for(
        &mut harness,
        String::from_utf8(greet.stdout).unwrap().trim(),
    );
    wait_for_label(&mut harness, "1 of 1");
    assert_eq!(selected(&harness, &sandbox, &repo), "Greet the world");
    search_for(&mut harness, "v0.1");
    settle(&mut harness, "the tag's commit", |app| {
        app.state.repos[0].history_view.selected_row()
            == app.state.repos[0].history.shown.row_of(&tagged(&repo), 0)
    });

    // Escape ends it.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(
        harness
            .query_by_role_and_label(Role::TextInput, &bar_label())
            .is_none()
    );

    // In the light theme too.
    let mut harness = harness_with(sandbox.git().clone(), sandbox.home(), egui::Theme::Light);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    search_for(&mut harness, "parser");
    wait_for_label(&mut harness, "1 of 3");
    settle(&mut harness, "the commit's details", details_shown);
    render(&mut harness, "history-search-light");
}

/// The commit `v0.1` tags in `repo` (read without the harness's git).
fn tagged(repo: &Path) -> ObjectId {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "v0.1^{commit}"])
        .current_dir(repo)
        .output()
        .unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// A repository of `commits` commits whose messages and authors come from small sets in
/// assorted case (made by `git fast-import`).
fn assorted(sandbox: &Sandbox, commits: u32) -> PathBuf {
    const WORDS: [&str; 10] = [
        "fix", "Fix", "FIX", "memory", "Memory", "leak", "LEAK", "parser", "(v1.2)", "Zoë",
    ];
    const AUTHORS: [&str; 4] = [
        "Ada Lovelace <ada@example.org>",
        "ada LOVELACE <ADA@EXAMPLE.ORG>",
        "Bo Diddley <bo@example.org>",
        "Zoë Example <zoe@example.org>",
    ];
    let repo = sandbox.init("assorted").unwrap();
    let mut seed = 0x5eed_u64;
    let mut random = |bound: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % u64::try_from(bound).unwrap()).unwrap()
    };
    let mut stream = String::new();
    for index in 0..commits {
        let mut message = String::new();
        for line in 0..=random(3) {
            if line > 0 {
                message.push('\n');
            }
            for _ in 0..=random(4) {
                let _ = write!(message, "{} ", WORDS[random(WORDS.len())]);
            }
        }
        message.push('\n');
        let author = AUTHORS[random(AUTHORS.len())];
        let _ = write!(
            stream,
            "commit refs/heads/main\nauthor {author} {} +0000\ncommitter C <c@egit.invalid> {} \
             +0000\ndata {}\n{message}\n",
            1_700_000_000 + index,
            1_700_000_000 + index,
            message.len()
        );
    }
    sandbox
        .git()
        .write("fast-import")
        .arg("--quiet")
        .current_dir(&repo)
        .stdin(stream.into_bytes())
        .output()
        .unwrap();
    repo
}

/// The rows `found` holds, top first.
fn rows_of(found: &Found) -> Vec<u64> {
    let mut rows = Vec::new();
    let mut at = None;
    while let Some(row) = found.next(at) {
        if at.is_some_and(|at| row <= at) {
            break;
        }
        rows.push(row);
        at = Some(row);
    }
    rows
}

#[test]
fn messages_and_authors_read_here_are_those_git_finds() {
    let sandbox = Sandbox::new().unwrap();
    let repo = assorted(&sandbox, 300);
    assert!(
        GixRepository::open(&repo).is_some(),
        "egit reads the commits itself"
    );
    let git = sandbox.git();
    let repository = Arc::new(Repository::discover(git, &repo).unwrap());
    let ctx = egui::Context::default();
    let mut history = History::load(
        git,
        &repository,
        &Objects::default(),
        &Scope::default(),
        &ctx,
    );
    let started = Instant::now();
    while history.is_loading() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the walk ended"
        );
        history.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(history.rows(), 300);
    let index = Arc::new(OnceLock::new());
    for (kind, query) in [
        (SearchKind::Message, "fix"),
        (SearchKind::Message, "Fix"),
        (SearchKind::Message, "memory leak"),
        (SearchKind::Message, "(v1.2) "),
        (SearchKind::Message, "Zoë"),
        (SearchKind::Message, "nowhere"),
        (SearchKind::Author, "ada"),
        (SearchKind::Author, "Lovelace"),
        (SearchKind::Author, "@example.org>"),
        (SearchKind::Author, "Zoë Ex"),
    ] {
        let search = search::start(
            Asked {
                git,
                repository: &repository,
                names: history.names(),
                index: &index,
                every_tree: true,
                kind,
                query,
            },
            &ctx,
        );
        let started = Instant::now();
        while !search.found().is_done() {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the search ended"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(search.found().problem(), None);
        let ours: BTreeSet<ObjectId> = rows_of(search.found())
            .into_iter()
            .map(|row| history.id(row).unwrap())
            .collect();
        let fold_case = !matches_case(query);
        let filter = if kind == SearchKind::Message {
            CommitFilter::Message {
                text: query,
                fold_case,
            }
        } else {
            CommitFilter::Author {
                text: query,
                fold_case,
            }
        };
        let mut theirs = BTreeSet::new();
        find_commits(git, &repository, filter, &CancelToken::new(), |id| {
            theirs.insert(id);
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(ours, theirs, "{kind:?} {query:?}");
        assert_eq!(search.found().count(), u64::try_from(ours.len()).unwrap());
    }
}
