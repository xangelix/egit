//! What egit and the bridge say to each other, a JSON object a line.
//!
//! [`Request`]s egit writes, [`Event`]s the bridge answers with. The SDK's own messages pass through as they are
//! ([`Event::Message`]), read by [`crate::Transcript`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How Claude Code asks before using tools (its permission mode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Asks before anything not allowed already.
    #[default]
    Default,
    /// Edits files without asking; asks for the rest.
    AcceptEdits,
    /// Reads and plans, changing nothing, until the plan is approved.
    Plan,
    /// Claude Code's own classifier decides what needs asking.
    Auto,
    /// Asks for nothing: everything is allowed.
    BypassPermissions,
    /// Refuses whatever isn't allowed already, asking nothing.
    DontAsk,
}

impl Mode {
    /// The modes offered, in order.
    pub const OFFERED: [Self; 5] = [
        Self::Default,
        Self::AcceptEdits,
        Self::Plan,
        Self::Auto,
        Self::BypassPermissions,
    ];

    /// Its name, as people know it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Default => "Ask first",
            Self::AcceptEdits => "Accept edits",
            Self::Plan => "Plan",
            Self::Auto => "Auto",
            Self::BypassPermissions => "Bypass permissions",
            Self::DontAsk => "Don't ask",
        }
    }

    /// The mode Claude Code names `name` (`acceptEdits`), if it is one.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        serde_json::from_value(Value::String(name.to_owned())).ok()
    }
}

/// A line egit writes to the bridge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    /// Starts the conversation (once, first): in `cwd`, a new one or `resume`d.
    Start {
        /// The folder it works in.
        cwd: String,
        /// The session to go on with, if any.
        resume: Option<String>,
        /// The model (`None`: Claude Code's default).
        model: Option<String>,
        /// The permission mode to start in.
        mode: Mode,
        /// An account's variables for Claude Code (its API key), where the bridge runs on
        /// another computer: sent, so no command line there shows them.
        #[serde(skip_serializing_if = "Secrets::is_empty")]
        env: Secrets,
    },
    /// A prompt.
    Send {
        /// What the user wrote.
        text: String,
        /// Images that go with it.
        images: Vec<Image>,
    },
    /// The answer to a [`Event::Permission`].
    Answer(Answer),
    /// Stops the turn under way.
    Interrupt,
    /// Changes the permission mode.
    Mode {
        /// The new mode.
        mode: Mode,
    },
    /// Changes the model (`None`: Claude Code's default).
    Model {
        /// The new model.
        model: Option<String>,
    },
    /// Asks for [`Event::Info`].
    Info,
}

/// Variables kept from view: serialized as an object of them, but never shown in debug output,
/// and wiped from memory when dropped.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secrets(pub Vec<(String, zeroize::Zeroizing<String>)>);

impl Secrets {
    /// Whether it holds none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, _)| name))
            .finish()
    }
}

impl Serialize for Secrets {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value.as_str())?;
        }
        map.end()
    }
}

/// An image sent with a prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    /// Its type (`image/png`, `image/jpeg`, `image/gif`, `image/webp`).
    pub media_type: String,
    /// Its bytes, in base64.
    pub data: String,
}

impl Image {
    /// An image of `media_type` from its bytes.
    #[must_use]
    pub fn of(media_type: &str, bytes: &[u8]) -> Self {
        use base64::Engine as _;
        Self {
            media_type: media_type.to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

/// Whether a tool may run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Answer {
    /// The [`PermissionRequest::request`] answered.
    pub request: String,
    /// Allowed, or refused.
    pub allow: bool,
    /// Allowed from now on, as Claude Code suggested (its rules for the conversation).
    pub always: bool,
    /// The tool's input as it should run (a question's answers added), when not as asked.
    pub input: Option<Value>,
    /// Why it was refused, for Claude.
    pub message: Option<String>,
    /// Refused, and the turn stopped.
    pub interrupt: bool,
}

/// A line the bridge writes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum Event {
    /// The bridge is ready for [`Request::Start`].
    Ready,
    /// One of the SDK's messages, as it gave it.
    Message {
        /// The message.
        message: Value,
    },
    /// A tool waits for permission.
    Permission(PermissionRequest),
    /// A permission request no longer needs answering (its turn was stopped).
    Withdrawn {
        /// The [`PermissionRequest::request`] withdrawn.
        request: String,
    },
    /// Who is signed in, and what can be chosen.
    Info(Info),
    /// A request failed.
    Failed {
        /// The request's op, if it could be read.
        #[serde(default)]
        op: Option<String>,
        /// Why.
        message: String,
    },
    /// The conversation ended (on its own: egit didn't close it), and why if it failed.
    Ended {
        /// Why it failed, if it did.
        #[serde(default)]
        error: Option<String>,
    },
}

/// A tool waiting for permission.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    /// Its id, for the [`Answer`].
    pub request: String,
    /// The tool (`Bash`, `Edit`, `AskUserQuestion`, `ExitPlanMode`, an MCP server's).
    pub tool: String,
    /// What it would be given.
    pub input: Value,
    /// Rules Claude Code suggests for allowing it from now on.
    #[serde(default)]
    pub suggestions: Vec<Value>,
    /// Why Claude Code asks, if it says.
    #[serde(default)]
    pub reason: Option<String>,
    /// A path outside the folders allowed that it would reach.
    #[serde(default)]
    pub blocked_path: Option<String>,
    /// The tool call's id in the transcript.
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The subagent asking, if one is.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Who is signed in, and the models and commands there are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Info {
    /// Who is signed in.
    pub account: Account,
    /// The models there are.
    pub models: Vec<Model>,
    /// The slash commands there are.
    pub commands: Vec<Command>,
}

/// The account a conversation runs as, as Claude Code says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    /// Their email.
    pub email: Option<String>,
    /// Their organization.
    pub organization: Option<String>,
    /// Their plan (`max`, `pro`, `team`).
    pub subscription_type: Option<String>,
    /// Where the login's token comes from.
    pub token_source: Option<String>,
    /// Where an API key comes from, when one is used.
    pub api_key_source: Option<String>,
}

/// A model that can be chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Model {
    /// What to ask for (`sonnet`, an id).
    pub value: String,
    /// Its name, as people know it.
    pub display_name: String,
    /// What it is good for.
    pub description: String,
}

/// A slash command.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Command {
    /// Without its `/`.
    pub name: String,
    /// What it does.
    pub description: String,
    /// What it takes (`<file>`).
    pub argument_hint: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_events_read_as_the_bridge_reads_them() {
        let start = Request::Start {
            cwd: "/work".to_owned(),
            resume: None,
            model: None,
            mode: Mode::AcceptEdits,
            env: Secrets::default(),
        };
        assert_eq!(
            serde_json::to_string(&start).unwrap(),
            r#"{"op":"start","cwd":"/work","resume":null,"model":null,"mode":"acceptEdits"}"#
        );
        // An account's key: sent, never shown.
        let keyed = Secrets(vec![(
            "ANTHROPIC_API_KEY".to_owned(),
            zeroize::Zeroizing::new("sk-test".to_owned()),
        )]);
        assert_eq!(format!("{keyed:?}"), r#"["ANTHROPIC_API_KEY"]"#);
        assert_eq!(
            serde_json::to_string(&keyed).unwrap(),
            r#"{"ANTHROPIC_API_KEY":"sk-test"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Interrupt).unwrap(),
            r#"{"op":"interrupt"}"#
        );
        let answer = Request::Answer(Answer {
            request: "r1".to_owned(),
            allow: true,
            always: false,
            input: None,
            message: None,
            interrupt: false,
        });
        assert!(
            serde_json::to_string(&answer)
                .unwrap()
                .starts_with(r#"{"op":"answer","request":"r1","allow":true"#)
        );

        let event: Event = serde_json::from_str(
            r#"{"event":"permission","request":"r1","tool":"Bash","input":{"command":"ls"},"suggestions":[],"reason":null,"blockedPath":null,"toolUseId":"t1","agentId":null,"later":1}"#,
        )
        .unwrap();
        let Event::Permission(asked) = event else {
            panic!("{event:?}");
        };
        assert_eq!(
            (asked.tool.as_str(), asked.tool_use_id.as_deref()),
            ("Bash", Some("t1"))
        );
        assert_eq!(
            serde_json::from_str::<Event>(r#"{"event":"ended","error":null}"#).unwrap(),
            Event::Ended { error: None }
        );
        assert_eq!(
            Mode::parse("bypassPermissions"),
            Some(Mode::BypassPermissions)
        );
        assert_eq!(Mode::parse("sideways"), None);
        assert_eq!(
            serde_json::to_string(&Image::of("image/png", b"png")).unwrap(),
            r#"{"mediaType":"image/png","data":"cG5n"}"#
        );
    }
}
