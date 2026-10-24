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
    harness, harness_with, history_shown, open_path, profile_loaded, render, settle, wait_for_label,
};
use crate::app::EgitApp;

/// Node, and the bridge run against the stand-in SDK, where Node is installed.
pub(super) fn stand_in() -> Option<(egit_agents::install::Node, egit_agents::Launch)> {
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
        there: None,
    };
    Some((node, launch))
}

/// Writes `text` in the prompt and sends it with Enter.
pub(super) fn ask(harness: &mut Harness<'_, EgitApp>, text: &str) {
    let field = harness.get_by_role_and_label(Role::MultilineTextInput, "Ask Claude");
    field.focus();
    field.type_text(text);
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
}

/// The choice `label` of the question asked (its description under its words).
fn choice<'h>(harness: &'h Harness<'_, EgitApp>, label: &'h str) -> egui_kittest::Node<'h> {
    harness.get_by(move |node| {
        node.role() == Role::RadioButton
            && node
                .label()
                .is_some_and(|said| said.split('\n').next() == Some(label))
    })
}

/// Waits until Claude Code, started as the panel shows, has said what it offers (its models
/// change the composer's look as they land).
fn started(harness: &mut Harness<'_, EgitApp>) {
    settle(harness, "Claude Code started", |app| {
        app.state.repos[0]
            .agents
            .current()
            .is_some_and(|conversation| conversation.info.is_some())
    });
}

/// Drags the panel's edge as far right as it goes: the panel at its narrowest.
fn narrowed(harness: &mut Harness<'_, EgitApp>) {
    let edge = harness
        .get_by_role_and_label(Role::Label, "Agents")
        .rect()
        .left()
        - 12.0;
    let at = |x: f32| egui::pos2(x, 400.0);
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(egui::Event::PointerMoved(at(edge)));
    harness.step();
    harness.event(button(at(edge), true));
    harness.step();
    for step in 1..=10_u8 {
        harness.event(egui::Event::PointerMoved(at(
            f32::from(step).mul_add(30.0, edge)
        )));
        harness.step();
    }
    harness.event(button(at(edge + 300.0), false));
    harness.event(egui::Event::PointerMoved(at(300.0)));
    harness.run_steps(4);
}

/// Waits until the conversation's turn has ended.
pub(super) fn turn_ended(harness: &mut Harness<'_, EgitApp>) {
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
        .click_accesskit();
    wait_for_label(&mut harness, "Ask Claude to work in repo");
    started(&mut harness);
    ask(&mut harness, "hello");
    wait_for_label(&mut harness, "Hello");
    turn_ended(&mut harness);

    // A command, allowed for the rest of the conversation; its line opens to show it.
    ask(&mut harness, "run");
    wait_for_label(&mut harness, "Claude wants to run a command");
    render(&mut harness, "agents-permission");
    harness
        .get_by_role_and_label(Role::Button, "Allow for this conversation")
        .click_accesskit();
    turn_ended(&mut harness);
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Run echo hi")
        .click_accesskit();
    wait_for_label(&mut harness, "hi (always)");

    // A question, answered: first the panel hidden, the question marked on the Agents tool
    // and the tab, then shown again.
    ask(&mut harness, "ask");
    wait_for_label(&mut harness, "Claude asks");
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click_accesskit();
    harness.run_steps(2);
    let attention = harness.state().state.repos[0].agents.attention();
    assert_eq!(
        (attention.waiting, attention.words().as_str()),
        (1, "Claude is waiting for your answer")
    );
    render(&mut harness, "agents-attention");
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click_accesskit();
    wait_for_label(&mut harness, "Claude asks");
    choice(&harness, "Two").click_accesskit();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Answer")
        .click_accesskit();
    turn_ended(&mut harness);

    // The mode, changed from its menu, is what Claude Code runs in.
    harness.get_by_label_contains("Ask first").click_accesskit();
    harness.run_steps(2);
    harness.get_by_label("Plan").click_accesskit();
    harness.run_steps(2);
    assert!(harness.query_by_label_contains("Plan").is_some());
    ask(&mut harness, "mode");
    wait_for_label(&mut harness, "Mode plan, model stand-in");
    turn_ended(&mut harness);
    render(&mut harness, "agents");

    // An earlier conversation, gone on with.
    harness
        .get_by_role_and_label(Role::Button, "hello")
        .click_accesskit();
    // Long ago: its date (in the viewer's time zone).
    settle(&mut harness, "the earlier conversations", |app| {
        app.state.repos[0].agents.past.is_ok()
    });
    harness.run_steps(2);
    harness
        .get_by_label_contains("A newer conversation · 20")
        .click_accesskit();
    wait_for_label(&mut harness, "Earlier, in newer");
}

#[test]
fn conversations_run_as_the_accounts_added() {
    let Some((node, launch)) = stand_in() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    // A helper keeping credentials in a file, as the user's would in their keychain.
    let kept = sandbox.path("credentials");
    let helper = format!("store --file={}", kept.display());
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "credential.helper", &helper],
        )
        .unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    let mut harness = harness(&sandbox);
    harness.state_mut().state.agents.stand_in(node, launch);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // Two accounts, added in Preferences: a folder, and an API key the helper keeps.
    harness
        .state_mut()
        .state
        .actions
        .push(crate::app::Action::ShowPreferences);
    wait_for_label(&mut harness, "Add the account");
    let type_in = |harness: &mut Harness<'_, EgitApp>, field: &str, text: &str| {
        let node = harness.get_by_role_and_label(Role::TextInput, field);
        node.focus();
        node.type_text(text);
        harness.run_steps(2);
    };
    // Below the fold: clicked as a screen reader clicks.
    type_in(&mut harness, "Account's name", "work");
    harness.get_by_label("Add the account").click_accesskit();
    wait_for_label(&mut harness, "Forget the account");
    type_in(&mut harness, "Account's name", "ci");
    harness.get_by_label("An API key").click_accesskit();
    harness.run_steps(2);
    let key = harness.get_by_role_and_label(Role::PasswordInput, "API key");
    key.focus();
    key.type_text("sk-test");
    harness.run_steps(2);
    harness.get_by_label("Add the account").click_accesskit();
    settle(&mut harness, "both accounts", |app| {
        app.state.settings.agent_accounts.len() == 2
    });
    assert!(std::fs::read_to_string(&kept).unwrap().contains("sk-test"));
    // Scrolled down to the card, rendered.
    harness
        .get_by_label("In effect at once, and kept for next time.")
        .hover();
    for _ in 0..60 {
        if harness.get_by_label("Add the account").rect().bottom() < 640.0 {
            break;
        }
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -40.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_steps(4);
    render(&mut harness, "agents-accounts");

    // Back to the repository (its tab the second).
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num2);
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click_accesskit();
    wait_for_label(&mut harness, "Ask Claude to work in repo");

    // As the API key: given to Claude Code, from the helper.
    harness
        .get_by_role_and_label(Role::Button, "Claude Code's own")
        .click_accesskit();
    harness.run_steps(2);
    harness.get_by_label("ci").click_accesskit();
    harness.run_steps(2);
    ask(&mut harness, "who");
    wait_for_label(&mut harness, "Key sk-test, folder none");
    turn_ended(&mut harness);

    // A new conversation keeps the account; as the folder: its own, no key.
    harness
        .get_by_role_and_label(Role::Button, "New conversation")
        .click_accesskit();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "ci")
        .click_accesskit();
    harness.run_steps(2);
    harness.get_by_label("work").click_accesskit();
    harness.run_steps(2);
    ask(&mut harness, "who");
    let folder = sandbox.home().join(".claude-work");
    wait_for_label(
        &mut harness,
        &format!("Key none, folder {}", folder.display()),
    );
}

/// A repository open with the agents' panel shown, against the stand-in (`None`: Claude not set
/// up), in `theme`.
fn panel_open(
    sandbox: &Sandbox,
    stand_in: Option<(egit_agents::install::Node, egit_agents::Launch)>,
    theme: egui::Theme,
) -> Harness<'static, EgitApp> {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    let mut harness = harness_with(sandbox.git().clone(), sandbox.home(), theme);
    if let Some((node, launch)) = stand_in {
        harness.state_mut().state.agents.stand_in(node, launch);
    }
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click_accesskit();
    harness.run_steps(2);
    harness
}

#[test]
fn every_state_of_the_panel_is_drawn() {
    let Some((node, launch)) = stand_in() else {
        return;
    };
    // Before Claude is set up: what it takes.
    let sandbox = Sandbox::new().unwrap();
    let mut harness = panel_open(&sandbox, None, egui::Theme::Dark);
    wait_for_label(&mut harness, "Set up Claude");
    render(&mut harness, "agents-setup");

    for (theme, suffix) in [(egui::Theme::Dark, ""), (egui::Theme::Light, "-light")] {
        let sandbox = Sandbox::new().unwrap();
        let mut harness = panel_open(&sandbox, Some((node.clone(), launch.clone())), theme);
        wait_for_label(&mut harness, "Ask Claude to work in repo");
        started(&mut harness);
        render(&mut harness, &format!("agents-empty{suffix}"));

        // A long turn: thinking, Markdown, and each kind of tool line, some opened.
        ask(&mut harness, "rich");
        turn_ended(&mut harness);
        if let Some(conversation) = harness.state_mut().state.repos[0].agents.current() {
            conversation
                .view
                .open(&["t-edit", "t-todo", "t-agent", "t-lint"]);
            conversation.view.follow();
        }
        harness.run_steps(4);
        render(&mut harness, &format!("agents-rich{suffix}"));

        // An edit waiting for permission, its change as a diff; then the panel at its
        // narrowest.
        ask(&mut harness, "edit");
        wait_for_label(&mut harness, "Allow once");
        render(&mut harness, &format!("agents-edit{suffix}"));
        if suffix.is_empty() {
            narrowed(&mut harness);
            render(&mut harness, "agents-narrow");
        }
        harness
            .get_by_role_and_label(Role::Button, "Allow once")
            .click_accesskit();
        turn_ended(&mut harness);
    }

    // A plan to approve, a question to answer, and Claude at work.
    let sandbox = Sandbox::new().unwrap();
    let mut harness = panel_open(&sandbox, Some((node, launch)), egui::Theme::Dark);
    wait_for_label(&mut harness, "Ask Claude to work in repo");
    started(&mut harness);
    ask(&mut harness, "plan");
    wait_for_label(&mut harness, "Approve and accept edits");
    render(&mut harness, "agents-plan");
    harness
        .get_by_role_and_label(Role::Button, "Approve and accept edits")
        .click_accesskit();
    turn_ended(&mut harness);
    ask(&mut harness, "ask");
    wait_for_label(&mut harness, "Claude asks");
    render(&mut harness, "agents-question");
    choice(&harness, "One").click_accesskit();
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Answer")
        .click_accesskit();
    turn_ended(&mut harness);
    ask(&mut harness, "wait");
    harness.run_steps(4);
    render(&mut harness, "agents-working");
    harness
        .get_by_role_and_label(Role::Button, &format!("{}  Stop", crate::theme::icon::STOP))
        .click_accesskit();
    turn_ended(&mut harness);
}

#[test]
fn commands_images_and_files_go_with_a_conversation() {
    let Some((node, launch)) = stand_in() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    sandbox
        .commit_file(&repo, "src/parser.rs", "fn parse() {}\n", "Start")
        .unwrap();
    let images = sandbox.path("images");
    std::fs::create_dir_all(&images).unwrap();
    image::RgbImage::new(4, 3)
        .save(images.join("shot.png"))
        .unwrap();
    let mut harness = harness(&sandbox);
    harness.state_mut().state.agents.stand_in(node, launch);
    harness.state_mut().state.files = crate::desktop::FileDialogs::in_folder(images);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    harness
        .get_by_role_and_label(Role::Button, "Agents")
        .click_accesskit();
    wait_for_label(&mut harness, "Ask Claude to work in repo");
    started(&mut harness);

    // A slash command, offered as it is typed and completed by Tab.
    let field = harness.get_by_role_and_label(Role::MultilineTextInput, "Ask Claude");
    field.focus();
    field.type_text("/re");
    wait_for_label(&mut harness, "/review");
    render(&mut harness, "agents-commands");
    harness.key_press(egui::Key::Tab);
    harness.run_steps(2);
    let draft = |harness: &mut Harness<'_, EgitApp>| {
        harness.state_mut().state.repos[0]
            .agents
            .current()
            .map(|conversation| conversation.view.draft.clone())
    };
    assert_eq!(draft(&mut harness).as_deref(), Some("/review "));
    assert!(harness.query_by_label("/review").is_none());
    if let Some(conversation) = harness.state_mut().state.repos[0].agents.current() {
        conversation.view.draft.clear();
    }

    // An image attached from the paperclip goes with the prompt, and shows under it.
    harness
        .get_by_role_and_label(Role::Button, "Attach images…")
        .click_accesskit();
    wait_for_label(&mut harness, "Remove shot.png");
    render(&mut harness, "agents-attached");
    ask(&mut harness, "images");
    wait_for_label(&mut harness, "Got 1 images: image/png");
    turn_ended(&mut harness);
    assert!(harness.query_by_label("Remove shot.png").is_none());

    // A file Claude read: unchanged, it opens in its app; changed, its changes show.
    ask(&mut harness, "rich");
    turn_ended(&mut harness);
    let file = harness.state().state.repos[0]
        .repository
        .work_tree()
        .join("src/parser.rs");
    let show = |harness: &mut Harness<'_, EgitApp>| {
        harness
            .get_all_by_role_and_label(Role::Button, "Show src/parser.rs")
            .next()
            .unwrap()
            .click_accesskit();
        harness.run_steps(2);
    };
    show(&mut harness);
    settle(&mut harness, "the file opened", |app| {
        app.state.opener.revealed().last() == Some(&(crate::desktop::Reveal::Open, file.clone()))
    });
    std::fs::write(&file, "fn parse() { 1 }\n").unwrap();
    harness.state_mut().state.repos[0].work_tree.refresh_now();
    settle(&mut harness, "the change read", |app| {
        app.state.repos[0].work_tree.is_settled()
            && app.state.repos[0]
                .work_tree
                .status()
                .and_then(Result::ok)
                .is_some_and(|status| status.entries().count() == 1)
    });
    show(&mut harness);
    settle(&mut harness, "its changes shown", |app| {
        app.state.repos[0].history_view.file_split().is_some()
    });
    render(&mut harness, "agents-file");
}
