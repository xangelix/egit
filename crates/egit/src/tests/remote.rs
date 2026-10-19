//! A repository on another computer, driven as a user drives one here: this computer playing
//! the other, through an sshd of the tests' own on the loopback address (Linux; skipped where
//! sshd isn't installed, unless `EGIT_REQUIRE_SSHD` is set). Opened from Home's form (`~` the
//! home folder there), its history and work tree read there, a change's diff shown, staged,
//! and committed there, a commit made there behind egit's back followed by the session looking
//! there, and a fetch cancelled while its remote hasn't answered.

use std::path::Path;
use std::time::{Duration, Instant};

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, harness, history_of, profile_loaded, render, settle, show_panel, type_into,
    uncommitted, wait_for_label,
};
use crate::app::{Action, EgitApp, SshSettings};
use crate::repo::FollowingThere;
use crate::settings::Location;

/// The harness, reaching other computers through the tests' sshd as the sandbox's git does,
/// or `None` where there is no sshd.
fn reaching(sandbox: &Sandbox) -> Option<Harness<'static, EgitApp>> {
    let host = match egit_git::testing::ssh_host() {
        Ok(host) => host,
        Err(error) => {
            assert!(
                std::env::var_os("EGIT_REQUIRE_SSHD").is_none(),
                "EGIT_REQUIRE_SSHD is set, but the tests' sshd couldn't start: {error}"
            );
            return None;
        }
    };
    let mut harness = harness(sandbox);
    let state = &mut harness.state_mut().state;
    state.git_options = sandbox.options();
    state.ssh = SshSettings {
        control: host.control.clone(),
        options: host.options,
    };
    Some(harness)
}

/// Whether `inotifywait` is installed (here is there); when it must be and isn't, fails.
fn inotifywait_installed() -> bool {
    let found = std::process::Command::new(egit_git::INOTIFYWAIT)
        .arg("--help")
        .output()
        .is_ok_and(|output| !output.stdout.is_empty() || !output.stderr.is_empty());
    assert!(
        found || std::env::var_os("EGIT_REQUIRE_INOTIFYWAIT").is_none(),
        "EGIT_REQUIRE_INOTIFYWAIT is set, but inotifywait isn't installed"
    );
    found
}

#[test]
fn a_repository_on_another_computer_is_driven_as_one_here() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    // In the home folder there (git's `HOME` there is the sandbox's).
    let repo = sandbox.init("home/repo").unwrap();
    sandbox
        .commit_file(&repo, "notes", "one\n", "Start")
        .unwrap();
    sandbox
        .commit_file(&repo, "notes", "two\n", "Second")
        .unwrap();
    std::fs::write(repo.join("notes"), "three\n").unwrap();
    let Some(mut harness) = reaching(&sandbox) else {
        return;
    };
    settle(&mut harness, "the profile", profile_loaded);

    // Opened from Home: the computer, and the folder there, `~` its home folder there.
    type_into(
        &mut harness,
        Role::TextInput,
        "On another computer",
        "127.0.0.1",
    );
    type_into(&mut harness, Role::TextInput, "folder", "~/repo");
    harness
        .get_by_role_and_label(
            Role::Button,
            &format!("{}  Open there", crate::theme::icon::GLOBE),
        )
        .click();
    settle(&mut harness, "the history there", history_of(2));
    {
        let repo = &harness.state().state.repos[0];
        assert!(repo.git.host().ssh().is_some(), "{:?}", repo.git);
        assert_eq!(repo.title, "repo · 127.0.0.1");
        assert_eq!(repo.git.home_there(), Some(sandbox.home()));
    }
    // Remembered with its computer.
    let recent = harness.state().state.settings.recent[0].location();
    assert_eq!(recent.host.as_deref(), Some("127.0.0.1"));

    // Its work tree read there; a change's diff, then the change staged and committed there.
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage all");
    harness.get_by_role_and_label(Role::Button, "notes").click();
    settle(&mut harness, "the diff there", |app| {
        let view = &mut app.state.repos[0].history_view;
        view.file_shown() == Some(("notes".to_owned(), true)) && view.diff_lines() == Some(3)
    });
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 1 file");
    type_into(&mut harness, Role::TextInput, "Summary", "Third, from afar");
    render(&mut harness, "remote");
    harness.get_by_label("Commit 1 file").click();
    settle(&mut harness, "the commit there", history_of(3));
    let last = sandbox
        .run(&repo, "log", &["-1", "--format=%s"])
        .unwrap()
        .stdout;
    assert_eq!(String::from_utf8(last).unwrap().trim(), "Third, from afar");

    // A commit made there behind egit's back: the session looking there sees its refs move.
    sandbox
        .commit_file(&repo, "more", "four\n", "Fourth, behind its back")
        .unwrap();
    settle(&mut harness, "the commit made outside", history_of(4));

    told_at_once(&mut harness, &repo);

    cancels_a_fetch(&mut harness, &sandbox, &repo);
}

/// Where inotify-tools is installed (else changes there are looked for every few seconds):
/// told at once there, a file changed there shows, and the Overview says how changes are
/// followed.
fn told_at_once(harness: &mut Harness<'static, EgitApp>, repo: &Path) {
    if !inotifywait_installed() {
        return;
    }
    settle(harness, "told at once there", |app| {
        app.state.repos[0].following_there() == Some(FollowingThere::AtOnce)
    });
    std::fs::write(repo.join("notes"), "five\n").unwrap();
    settle(harness, "the change there", |app| {
        app.state.repos[0]
            .work_tree
            .status()
            .and_then(Result::ok)
            .is_some_and(|status| status.entries().count() == 1)
    });
    show_panel(harness, "Overview");
    wait_for_label(harness, FollowingThere::AtOnce.words());
    render(harness, "remote-overview");
    harness
        .get_by_role_and_label(
            Role::Button,
            &format!("{}  History", crate::theme::icon::ARROW_LEFT),
        )
        .click();
    harness.run_steps(2);
}

/// A fetch from a remote answering only after a long pause (git's `ext::` transport runs a
/// command as the remote), cancelled: git's group there is asked to stop, then the session.
fn cancels_a_fetch(harness: &mut Harness<'static, EgitApp>, sandbox: &Sandbox, repo: &Path) {
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.ext.allow", "always"],
        )
        .unwrap();
    let slow = format!(
        "ext::sh -c sleep% 60;% exec% {}% upload-pack% {}",
        sandbox.git().program().display(),
        repo.display()
    );
    sandbox
        .run(repo, "remote", &["add", "slow", &slow])
        .unwrap();
    click_when_enabled(
        harness,
        &format!("{}  Fetch", crate::theme::icon::ARROWS_CLOCKWISE),
    );
    wait_for_label(harness, "Cancel");
    let cancelled = Instant::now();
    // The task bar's, at the top (the output's, below, has one too).
    harness
        .query_all_by_label("Cancel")
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    wait_for_label(harness, "Cancelled.");
    assert!(cancelled.elapsed() < Duration::from_secs(20));
}

#[test]
fn the_agents_run_on_the_computer_the_repository_is_on() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let Some((node, launch)) = super::agents::stand_in() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    let Some(mut harness) = reaching(&sandbox) else {
        return;
    };
    settle(&mut harness, "the profile", profile_loaded);
    let there = Location {
        host: Some("127.0.0.1".to_owned()),
        path: repo,
    };
    harness.state_mut().state.actions.push(Action::Open(there));
    settle(&mut harness, "the history there", history_of(1));
    // Claude's setup there: the stand-in, run through the session (as the installed bridge
    // would be, from its cache folder there).
    let git = harness.state().state.repos[0].git.clone();
    let mut setup = crate::agents::AgentsSetup::there(git);
    setup.stand_in(node, launch);
    let state = &mut harness.state_mut().state;
    state.agents_there.insert("127.0.0.1".to_owned(), setup);
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click_accesskit();
    wait_for_label(&mut harness, "Ask Claude to work in repo, on 127.0.0.1");
    super::agents::ask(&mut harness, "hello");
    wait_for_label(&mut harness, "Hello");
    super::agents::turn_ended(&mut harness);
    // Its conversations are those Claude Code keeps there.
    let conversation = harness.state_mut().state.repos[0].agents.current();
    assert!(conversation.is_some_and(|conversation| conversation.transcript.session().is_some()));
}
