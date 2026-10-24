//! The bridge (`bridge/claude/bridge.mjs`) run by Node against a stand-in for the Claude Agent
//! SDK (`tests/stand-in/sdk.mjs`): no account, no network. A conversation streams, a tool is
//! allowed (once, and from then on) and refused, a question is answered, a turn is interrupted,
//! the mode and model change, who is signed in is asked; sessions are listed and read.
//!
//! Skipped where Node.js isn't installed, unless `EGIT_REQUIRE_NODE` is set (CI sets it).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers;
// a skipped test says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::print_stderr
)]

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use egit_agents::protocol::{Answer, Event, Image, Mode, PermissionRequest, Request, Secrets};
use egit_agents::sessions::{list_sessions, session_history};
use egit_agents::{Bridge, ItemKind, Launch, Prompt, ToolResult, Transcript};

use egit_git::Wait as _;

/// How to run the bridge against the stand-in, where Node is installed.
fn launch() -> Option<Launch> {
    let Ok(node) = egit_agents::install::find_node() else {
        assert!(
            std::env::var_os("EGIT_REQUIRE_NODE").is_none(),
            "EGIT_REQUIRE_NODE is set, but Node.js isn't installed"
        );
        eprintln!("skipped: Node.js isn't installed");
        return None;
    };
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Some(Launch {
        node: node.node,
        bridge: crate_dir.join("bridge/claude/bridge.mjs"),
        env: vec![(
            "EGIT_CLAUDE_SDK".into(),
            crate_dir.join("tests/stand-in/sdk.mjs").into(),
        )],
        unset: Vec::new(),
        there: None,
    })
}

/// A conversation under test: its bridge, its events, and its transcript as the window keeps it.
struct Conversation {
    bridge: Bridge,
    events: Receiver<Event>,
    transcript: Transcript,
}

impl Conversation {
    fn start(launch: &Launch, mode: Mode) -> Self {
        Self::start_with(launch, mode, Secrets::default())
    }

    /// Starts a conversation whose Claude Code gets `env` (an account's key).
    fn start_with(launch: &Launch, mode: Mode, env: Secrets) -> Self {
        let (bridge, events) = Bridge::start(launch, || {}).unwrap();
        let mut conversation = Self {
            bridge,
            events,
            transcript: Transcript::default(),
        };
        assert_eq!(conversation.next(), Event::Ready);
        conversation
            .bridge
            .send(&Request::Start {
                cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                resume: None,
                model: None,
                mode,
                env,
            })
            .unwrap();
        conversation
    }

    /// The next event, its messages read into the transcript.
    fn next(&mut self) -> Event {
        let event = self
            .events
            .recv_timeout(Duration::from_secs(20))
            .expect("the bridge said nothing");
        if let Event::Message { message } = &event {
            self.transcript.message(message);
        }
        event
    }

    /// Sends a prompt.
    fn say(&mut self, text: &str) {
        self.transcript.prompt(Prompt::of(text));
        self.bridge
            .send(&Request::Send {
                text: text.to_owned(),
                images: Vec::new(),
            })
            .unwrap();
    }

    /// Reads events until the turn ends.
    fn until_turn_ends(&mut self) {
        while self.transcript.busy() {
            if let Event::Failed { message, .. }
            | Event::Ended {
                error: Some(message),
            } = self.next()
            {
                panic!("{message}");
            }
        }
    }

    /// Reads events until a permission request.
    fn until_asked(&mut self) -> PermissionRequest {
        loop {
            match self.next() {
                Event::Permission(asked) => return asked,
                Event::Failed { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
    }

    fn answer(&self, answer: Answer) {
        self.bridge.send(&Request::Answer(answer)).unwrap();
    }

    /// The last tool call's result.
    fn last_result(&self) -> Option<ToolResult> {
        self.transcript
            .items()
            .iter()
            .rev()
            .find_map(|item| match &item.kind {
                ItemKind::Tool(call) => Some(call.result.clone()),
                _ => None,
            })
            .flatten()
    }

    /// The last thing Claude wrote.
    fn last_text(&self) -> Option<&str> {
        self.transcript
            .items()
            .iter()
            .rev()
            .find_map(|item| match &item.kind {
                ItemKind::Text(text) => Some(text.as_str()),
                _ => None,
            })
    }
}

fn allow(request: &str, always: bool) -> Answer {
    Answer {
        request: request.to_owned(),
        allow: true,
        always,
        input: None,
        message: None,
        interrupt: false,
    }
}

#[test]
fn a_conversation_streams_asks_and_is_interrupted() {
    let Some(launch) = launch() else {
        return;
    };
    let mut conversation = Conversation::start(&launch, Mode::Default);

    conversation.say("hello");
    conversation.until_turn_ends();
    assert_eq!(conversation.transcript.session(), Some("stand-in-session"));
    assert_eq!(conversation.last_text(), Some("Hello"));
    assert!((conversation.transcript.cost_usd() - 0.5).abs() < f64::EPSILON);

    // Allowed from now on: Claude Code's suggestions go back with it.
    conversation.say("run");
    let asked = conversation.until_asked();
    assert_eq!(asked.tool, "Bash");
    assert_eq!(asked.input["command"], "echo hi");
    assert_eq!(asked.tool_use_id.as_deref(), Some("tool-run"));
    conversation.answer(allow(&asked.request, true));
    conversation.until_turn_ends();
    assert_eq!(
        conversation.last_result(),
        Some(ToolResult {
            text: "hi (always)".to_owned(),
            error: false
        })
    );

    // Refused, with a reason Claude reads.
    conversation.say("run");
    let asked = conversation.until_asked();
    conversation.answer(Answer {
        allow: false,
        message: Some("Not now.".to_owned()),
        ..allow(&asked.request, false)
    });
    conversation.until_turn_ends();
    assert_eq!(
        conversation.last_result(),
        Some(ToolResult {
            text: "Not now.".to_owned(),
            error: true
        })
    );

    // A question, answered by the input given back.
    conversation.say("ask");
    let asked = conversation.until_asked();
    assert_eq!(asked.tool, "AskUserQuestion");
    let mut input = asked.input.clone();
    input["answers"] = serde_json::json!({"Which one?": "Two"});
    conversation.answer(Answer {
        input: Some(input),
        ..allow(&asked.request, false)
    });
    conversation.until_turn_ends();
    assert_eq!(
        conversation.last_result().map(|result| result.text),
        Some(r#"{"Which one?":"Two"}"#.to_owned())
    );

    // Interrupted.
    conversation.say("wait");
    conversation.bridge.send(&Request::Interrupt).unwrap();
    conversation.until_turn_ends();
    let ended = conversation
        .transcript
        .items()
        .last()
        .map(|item| &item.kind);
    assert!(
        matches!(ended, Some(ItemKind::Turn(end)) if end.error.as_deref() == Some("interrupted")),
        "{ended:?}"
    );

    // The mode and model, then who is signed in.
    for request in [
        Request::Mode { mode: Mode::Plan },
        Request::Model {
            model: Some("other".to_owned()),
        },
    ] {
        conversation.bridge.send(&request).unwrap();
    }
    conversation.say("mode");
    conversation.until_turn_ends();
    assert_eq!(conversation.last_text(), Some("Mode plan, model other"));
    conversation.bridge.send(&Request::Info).unwrap();
    let info = loop {
        if let Event::Info(info) = conversation.next() {
            break info;
        }
    };
    assert_eq!(
        info.account.email.as_deref(),
        Some("tester@example.invalid")
    );
    assert_eq!(info.models[0].display_name, "Stand-in");
    assert_eq!(info.commands[0].name, "review");
}

#[test]
fn what_it_offers_is_said_once_started_and_images_go_with_a_prompt() {
    let Some(launch) = launch() else {
        return;
    };
    let mut conversation = Conversation::start(&launch, Mode::Default);
    // Unasked, before any prompt.
    let info = loop {
        if let Event::Info(info) = conversation.next() {
            break info;
        }
    };
    let commands: Vec<&str> = info
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert_eq!(commands, ["review", "compact", "init"]);
    conversation.transcript.prompt(Prompt {
        text: "images".to_owned(),
        attachments: vec!["a.png".to_owned(), "b.jpg".to_owned()],
    });
    conversation
        .bridge
        .send(&Request::Send {
            text: "images".to_owned(),
            images: vec![
                Image::of("image/png", b"png"),
                Image::of("image/jpeg", b"jpeg"),
            ],
        })
        .unwrap();
    conversation.until_turn_ends();
    assert_eq!(
        conversation.last_text(),
        Some("Got 2 images: image/png, image/jpeg")
    );
}

#[test]
fn the_bridge_runs_on_another_computer_its_key_sent() {
    let Some(mut launch) = launch() else {
        return;
    };
    // This computer playing the other, through the tests' sshd (skipped without one).
    let Ok(host) = egit_git::testing::ssh_host() else {
        assert!(
            std::env::var_os("EGIT_REQUIRE_SSHD").is_none(),
            "EGIT_REQUIRE_SSHD is set, but the tests' sshd couldn't start"
        );
        return;
    };
    let sandbox = egit_git::testing::Sandbox::new().unwrap();
    let there = egit_git::Git::new(egit_git::GitOptions {
        host: egit_git::Host::Ssh(host),
        ..sandbox.options()
    })
    .wait()
    .unwrap();
    launch.there = Some(there);
    let key = Secrets(vec![(
        "ANTHROPIC_API_KEY".to_owned(),
        zeroize::Zeroizing::new("sk-there".to_owned()),
    )]);
    let mut conversation = Conversation::start_with(&launch, Mode::Default, key);
    conversation.say("who");
    conversation.until_turn_ends();
    assert_eq!(conversation.last_text(), Some("Key sk-there, folder none"));
    // Asked once, there too.
    let dir = std::env::temp_dir();
    let sessions = list_sessions(&launch, &dir, 10).wait().unwrap();
    assert_eq!(sessions.len(), 2);
}

#[test]
fn the_bridge_runs_through_a_relay_its_key_sent() {
    let Some(mut launch) = launch() else {
        return;
    };
    // This computer's relay, as egit in a browser reaches it (skipped without one).
    if !egit_git::testing::through_a_relay() {
        return;
    }
    let sandbox = egit_git::testing::Sandbox::new().unwrap();
    let relay = egit_git::testing::relay().unwrap();
    launch.there = Some(
        sandbox
            .git_through(&relay, Vec::<(&str, &str)>::new())
            .unwrap(),
    );
    let key = Secrets(vec![(
        "ANTHROPIC_API_KEY".to_owned(),
        zeroize::Zeroizing::new("sk-relayed".to_owned()),
    )]);
    let mut conversation = Conversation::start_with(&launch, Mode::Default, key);
    conversation.say("who");
    conversation.until_turn_ends();
    assert_eq!(
        conversation.last_text(),
        Some("Key sk-relayed, folder none")
    );
    let dir = std::env::temp_dir();
    let sessions = list_sessions(&launch, &dir, 10).wait().unwrap();
    assert_eq!(sessions.len(), 2);
}

#[test]
fn sessions_are_listed_and_read() {
    let Some(launch) = launch() else {
        return;
    };
    let dir = std::env::temp_dir();
    let sessions = list_sessions(&launch, &dir, 50).wait().unwrap();
    let ids: Vec<&str> = sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    assert_eq!(ids, ["newer", "older"]);
    assert_eq!(sessions[0].git_branch.as_deref(), Some("main"));

    let messages = session_history(&launch, &dir, "newer").wait().unwrap();
    let mut transcript = Transcript::default();
    transcript.history(&messages);
    let kinds: Vec<ItemKind> = transcript
        .items()
        .iter()
        .map(|item| item.kind.clone())
        .collect();
    assert_eq!(
        kinds,
        [
            ItemKind::Prompt(Prompt::of("Before")),
            ItemKind::Text("Earlier, in newer".to_owned())
        ]
    );
}
