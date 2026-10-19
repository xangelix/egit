//! A repository on another computer, driven as a user drives one here: this computer playing
//! the other, through an sshd of the tests' own on the loopback address (Linux; skipped where
//! sshd isn't installed, unless `EGIT_REQUIRE_SSHD` is set). Opened from Home's form, its
//! history and work tree read there, a change staged and committed there, and a commit made
//! there behind egit's back followed by the session looking there.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_of, profile_loaded, render, settle, type_into, uncommitted, wait_for_label,
};
use crate::app::{Action, EgitApp, SshSettings};
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

#[test]
fn a_repository_on_another_computer_is_driven_as_one_here() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
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

    // Opened from Home: the computer, and the folder there.
    type_into(
        &mut harness,
        Role::TextInput,
        "On another computer",
        "127.0.0.1",
    );
    type_into(
        &mut harness,
        Role::TextInput,
        "folder",
        &repo.display().to_string(),
    );
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
    }
    // Remembered with its computer.
    let recent = harness.state().state.settings.recent[0].location();
    assert_eq!(recent.host.as_deref(), Some("127.0.0.1"));

    // Its work tree read there; a change staged and committed there.
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage all");
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
