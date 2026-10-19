//! The Agents panel driven as a user drives it, against a stand-in for the Claude Agent SDK
//! (`egit-agents`' `tests/stand-in/sdk.mjs`: no account, no network): shown from the top bar, a
//! prompt sent with Enter and its answer streamed in, a command allowed for the conversation
//! (its line opened to show it), a question answered, the mode changed, and an earlier
//! conversation gone on with. Skipped without Node.js unless `EGIT_REQUIRE_NODE` is set.

use std::path::PathBuf;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};
use crate::app::EgitApp;

/// Node, and the bridge run against the stand-in SDK, where Node is installed.
fn stand_in() -> Option<(egit_agents::install::Node, egit_agents::Launch)> {
    let Ok(node) = egit_agents::install::find_node() else {
        assert!(
            std::env::var_os("EGIT_REQUIRE_NODE").is_none(),
            "EGIT_REQUIRE_NODE is set, but Node.js isn't installed"
        );
        return None;
    };
    let agents = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../egit-agents");
    let launch = egit_agents::Launch {
        node: node.node.clone(),
        bridge: agents.join("bridge/claude/bridge.mjs"),
        env: vec![(
            "EGIT_CLAUDE_SDK".into(),
            agents.join("tests/stand-in/sdk.mjs").into(),
        )],
        unset: Vec::new(),
    };
    Some((node, launch))
}

/// Writes `text` in the prompt and sends it with Enter.
fn ask(harness: &mut Harness<'_, EgitApp>, text: &str) {
    let field = harness.get_by_role_and_label(Role::MultilineTextInput, "Ask Claude");
    field.focus();
    field.type_text(text);
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
}

/// Waits until the conversation's turn has ended.
fn turn_ended(harness: &mut Harness<'_, EgitApp>) {
    settle(harness, "the turn's end", |app| {
        app.state.repos[0]
            .agents
            .current()
            .is_some_and(|conversation| !conversation.transcript.busy())
    });
}

#[test]
fn a_conversation_with_claude_runs_in_the_panel() {
    let Some((node, launch)) = stand_in() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    let mut harness = harness(&sandbox);
    harness.state_mut().state.agents.stand_in(node, launch);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // Shown from the top bar; a prompt sent with Enter, its answer streamed in.
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click();
    wait_for_label(&mut harness, "Ask Claude to work in repo");
    ask(&mut harness, "hello");
    wait_for_label(&mut harness, "Hello");
    turn_ended(&mut harness);

    // A command, allowed for the rest of the conversation; its line opens to show it.
    ask(&mut harness, "run");
    wait_for_label(&mut harness, "Claude wants to run");
    render(&mut harness, "agents-permission");
    harness
        .get_by_role_and_label(Role::Button, "Allow for this conversation")
        .click();
    turn_ended(&mut harness);
    harness.run_steps(2);
    harness.get_by_label_contains("Run  echo hi").click();
    wait_for_label(&mut harness, "hi (always)");

    // A question, answered.
    ask(&mut harness, "ask");
    wait_for_label(&mut harness, "Claude asks");
    harness.get_by_label("Two").click();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Answer")
        .click();
    turn_ended(&mut harness);

    // The mode, changed from its menu, is what Claude Code runs in.
    harness.get_by_label_contains("Ask first").click();
    harness.run_steps(2);
    harness.get_by_label("Plan").click();
    harness.run_steps(2);
    assert!(harness.query_by_label_contains("Plan").is_some());
    ask(&mut harness, "mode");
    wait_for_label(&mut harness, "Mode plan, model stand-in");
    turn_ended(&mut harness);
    render(&mut harness, "agents");

    // An earlier conversation, gone on with.
    harness.get_by_role_and_label(Role::Button, "hello").click();
    // Long ago: its date (in the viewer's time zone).
    settle(&mut harness, "the earlier conversations", |app| {
        app.state.repos[0].agents.past.is_ok()
    });
    harness.run_steps(2);
    harness
        .get_by_label_contains("A newer conversation · 20")
        .click();
    wait_for_label(&mut harness, "Earlier, in newer");
}
