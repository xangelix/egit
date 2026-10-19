//! A conversation as it is shown: the SDK's messages (live, or a past session's) read into
//! [`Item`]s in order. Prompts, what Claude writes and thinks (streamed as it comes, then made
//! whole), tool calls with their results, each turn's end, and notices.
//!
//! Messages are read from JSON as the SDK gives them, taking what is shown and ignoring the
//! rest (new kinds of message included), so a newer SDK adds fields without breaking anything.
//! Each item notes when it last changed ([`Item::changed`]), so a view measures again only
//! what did.

use std::collections::HashMap;

use serde_json::Value;

use crate::protocol::Mode;

/// A conversation's items, and what it has said about itself.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    items: Vec<Item>,
    /// Each tool call's item, by its id.
    tools: HashMap<String, usize>,
    /// The item a streamed block is written into, until its whole message comes.
    live: Option<Live>,
    session: Option<String>,
    model: Option<String>,
    mode: Option<Mode>,
    busy: bool,
    /// Bumped by every change.
    version: u64,
    /// What the turns so far cost, as Claude Code estimates it.
    cost_usd: f64,
}

/// A streamed block under way.
#[derive(Clone, Debug)]
struct Live {
    index: usize,
    parent: Option<String>,
    thinking: bool,
}

/// One thing in a conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// What it is.
    pub kind: ItemKind,
    /// The tool call (a subagent's) it belongs to, if any.
    pub parent: Option<String>,
    /// The [`Transcript::version`] when it last changed.
    pub changed: u64,
}

/// What an item is.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// What the user asked.
    Prompt(String),
    /// What Claude wrote (Markdown).
    Text(String),
    /// What Claude thought.
    Thinking(String),
    /// A tool Claude used.
    Tool(ToolCall),
    /// A turn's end.
    Turn(TurnEnd),
    /// Something to know: a compaction, a retry, a limit, a refusal, a failure.
    Notice(Notice),
}

/// A tool call and, once it ran, its result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    /// Its id.
    pub id: String,
    /// The tool (`Read`, `Bash`, `Edit`, an MCP server's).
    pub name: String,
    /// What it was given.
    pub input: Value,
    /// What it gave back.
    pub result: Option<ToolResult>,
}

/// What a tool gave back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    /// Its text.
    pub text: String,
    /// Whether it failed (or was refused).
    pub error: bool,
}

/// How a turn ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnEnd {
    /// What it cost, as Claude Code estimates it (not a bill).
    pub cost_usd: f64,
    /// How long it took.
    pub duration_ms: u64,
    /// The model's turns in it.
    pub turns: u64,
    /// Tokens read (cached ones too), across its models.
    pub input_tokens: u64,
    /// Tokens written, across its models.
    pub output_tokens: u64,
    /// Why it failed, if it did.
    pub error: Option<String>,
}

/// Something worth knowing about the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// How much it matters.
    pub level: Level,
    /// What happened.
    pub text: String,
}

/// How much a notice matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// For information.
    Info,
    /// Something to watch.
    Warning,
    /// Something failed.
    Error,
}

impl Transcript {
    /// Its items, in order.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Bumped by every change: a view does nothing again while it stays the same.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// The session's id, once Claude Code has said it.
    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// The model in use, as Claude Code last said.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The permission mode, as Claude Code last said.
    #[must_use]
    pub const fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// Whether a turn is under way.
    #[must_use]
    pub const fn busy(&self) -> bool {
        self.busy
    }

    /// What the turns so far cost, as Claude Code estimates it.
    #[must_use]
    pub const fn cost_usd(&self) -> f64 {
        self.cost_usd
    }

    /// A prompt the user sent: a turn begins.
    pub fn prompt(&mut self, text: &str) {
        self.live = None;
        self.push(ItemKind::Prompt(text.to_owned()), None);
        self.busy = true;
    }

    /// A notice of egit's own (the bridge failing, say).
    pub fn notice(&mut self, level: Level, text: impl Into<String>) {
        self.push(
            ItemKind::Notice(Notice {
                level,
                text: text.into(),
            }),
            None,
        );
    }

    /// The conversation ended: no turn is under way any more.
    pub fn ended(&mut self) {
        self.busy = false;
        self.live = None;
        self.bump();
    }

    /// Reads one of the SDK's live messages.
    pub fn message(&mut self, message: &Value) {
        let parent = text(message, "parent_tool_use_id").map(str::to_owned);
        match (text(message, "type"), text(message, "subtype")) {
            (Some("system"), Some("init")) => {
                self.session = text(message, "session_id").map(str::to_owned);
                self.model = text(message, "model").map(str::to_owned);
                self.mode = text(message, "permissionMode").and_then(Mode::parse);
                self.bump();
            }
            (Some("system"), Some("status")) => {
                if let Some(mode) = text(message, "permissionMode").and_then(Mode::parse) {
                    self.mode = Some(mode);
                    self.bump();
                }
            }
            (Some("system"), Some("compact_boundary")) => {
                let before = message
                    .pointer("/compact_metadata/pre_tokens")
                    .and_then(Value::as_u64);
                let text = before.map_or_else(
                    || "The conversation was compacted.".to_owned(),
                    |tokens| format!("The conversation was compacted (it held {tokens} tokens)."),
                );
                self.notice(Level::Info, text);
            }
            (Some("system"), Some("api_retry")) => {
                let attempt = number(message, "attempt");
                let most = number(message, "max_retries");
                self.notice(
                    Level::Warning,
                    format!("Claude didn't answer: trying again ({attempt} of {most})."),
                );
            }
            (Some("system"), Some("permission_denied")) => {
                if let Some(said) = text(message, "message") {
                    self.notice(Level::Warning, said);
                }
            }
            (Some("stream_event"), _) => {
                if let Some(event) = message.get("event") {
                    self.stream(event, parent);
                }
            }
            (Some("assistant"), _) => {
                if let Some(content) = message.pointer("/message/content") {
                    self.assistant(content, parent.as_deref());
                }
                if message.get("error").is_some_and(|error| !error.is_null()) {
                    let said = message
                        .pointer("/message/content/0/text")
                        .and_then(Value::as_str)
                        .unwrap_or("Claude couldn't answer.");
                    self.notice(Level::Error, said);
                }
            }
            (Some("user"), _) => {
                // Prompts are noted as sent; replays and echoes of them aren't shown twice.
                if let Some(content) = message.pointer("/message/content") {
                    self.results(content);
                }
            }
            (Some("result"), _) => self.turn_end(message),
            (Some("rate_limit_event"), _) => self.rate_limit(message),
            (Some("auth_status"), _) => {
                if let Some(error) = text(message, "error") {
                    self.notice(Level::Error, error);
                }
            }
            _ => {}
        }
    }

    /// Reads a past session's messages (`getSessionMessages`): its prompts, what Claude wrote
    /// and did.
    pub fn history(&mut self, messages: &[Value]) {
        for message in messages {
            let parent = text(message, "parent_tool_use_id").map(str::to_owned);
            let Some(content) = message.pointer("/message/content") else {
                continue;
            };
            match text(message, "type") {
                Some("assistant") => self.assistant(content, parent.as_deref()),
                Some("user") => {
                    if let Some(prompt) = content.as_str() {
                        if !is_internal(prompt) {
                            self.push(ItemKind::Prompt(prompt.to_owned()), parent);
                        }
                    } else {
                        for block in blocks(content) {
                            if text(block, "type") == Some("text")
                                && let Some(prompt) = text(block, "text")
                                && !is_internal(prompt)
                            {
                                self.push(ItemKind::Prompt(prompt.to_owned()), parent.clone());
                            }
                        }
                        self.results(content);
                    }
                }
                _ => {}
            }
        }
        self.busy = false;
    }

    /// A raw stream event (`includePartialMessages`): text and thinking as they come.
    fn stream(&mut self, event: &Value, parent: Option<String>) {
        match text(event, "type") {
            Some("content_block_start") => {
                let thinking = match event.pointer("/content_block/type").and_then(Value::as_str) {
                    Some("text") => false,
                    Some("thinking") => true,
                    _ => return,
                };
                let kind = if thinking {
                    ItemKind::Thinking(String::new())
                } else {
                    ItemKind::Text(String::new())
                };
                let index = self.push(kind, parent.clone());
                self.live = Some(Live {
                    index,
                    parent,
                    thinking,
                });
            }
            Some("content_block_delta") => {
                let piece = event
                    .pointer("/delta/text")
                    .or_else(|| event.pointer("/delta/thinking"))
                    .and_then(Value::as_str);
                let Some((piece, index)) = piece.zip(self.live.as_ref().map(|live| live.index))
                else {
                    return;
                };
                self.version = self.version.saturating_add(1);
                if let Some(item) = self.items.get_mut(index) {
                    if let ItemKind::Text(written) | ItemKind::Thinking(written) = &mut item.kind {
                        written.push_str(piece);
                    }
                    item.changed = self.version;
                }
            }
            _ => {}
        }
    }

    /// An assistant message's blocks: a streamed block made whole, or new items.
    fn assistant(&mut self, content: &Value, parent: Option<&str>) {
        for block in blocks(content) {
            match text(block, "type") {
                Some("text") => {
                    let written = text(block, "text").unwrap_or_default().to_owned();
                    self.whole(ItemKind::Text(written), false, parent);
                }
                Some("thinking") => {
                    let thought = text(block, "thinking").unwrap_or_default().to_owned();
                    self.whole(ItemKind::Thinking(thought), true, parent);
                }
                Some("tool_use") => {
                    let id = text(block, "id").unwrap_or_default().to_owned();
                    let call = ToolCall {
                        id: id.clone(),
                        name: text(block, "name").unwrap_or_default().to_owned(),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                        result: None,
                    };
                    let index = self.push(ItemKind::Tool(call), parent.map(str::to_owned));
                    self.tools.insert(id, index);
                }
                _ => {}
            }
        }
    }

    /// A whole text or thinking block: into the streamed item it finishes, or a new one.
    fn whole(&mut self, kind: ItemKind, thinking: bool, parent: Option<&str>) {
        let streamed = self
            .live
            .take()
            .filter(|live| live.thinking == thinking && live.parent.as_deref() == parent);
        match streamed {
            Some(live) => {
                self.version = self.version.saturating_add(1);
                if let Some(item) = self.items.get_mut(live.index) {
                    item.kind = kind;
                    item.changed = self.version;
                }
            }
            None => {
                self.push(kind, parent.map(str::to_owned));
            }
        }
    }

    /// Tool results in a user message, each given to its call.
    fn results(&mut self, content: &Value) {
        for block in blocks(content) {
            if text(block, "type") != Some("tool_result") {
                continue;
            }
            let Some(&index) = text(block, "tool_use_id").and_then(|id| self.tools.get(id)) else {
                continue;
            };
            let result = ToolResult {
                text: result_text(block.get("content")),
                error: block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            self.version = self.version.saturating_add(1);
            if let Some(item) = self.items.get_mut(index) {
                if let ItemKind::Tool(call) = &mut item.kind {
                    call.result = Some(result);
                }
                item.changed = self.version;
            }
        }
    }

    /// A turn's result.
    fn turn_end(&mut self, message: &Value) {
        let models = message
            .get("modelUsage")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|models| models.values());
        let (input_tokens, output_tokens) =
            models.fold((0_u64, 0_u64), |(read, written), usage| {
                let input = [
                    "inputTokens",
                    "cacheReadInputTokens",
                    "cacheCreationInputTokens",
                ]
                .iter()
                .map(|key| usage.get(*key).and_then(Value::as_u64).unwrap_or(0))
                .fold(0_u64, u64::saturating_add);
                (
                    read.saturating_add(input),
                    written.saturating_add(
                        usage
                            .get("outputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    ),
                )
            });
        let failed = text(message, "subtype").is_some_and(|subtype| subtype != "success")
            || message.get("is_error").and_then(Value::as_bool) == Some(true);
        let error = failed.then(|| {
            let errors: Vec<&str> = message
                .get("errors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if errors.is_empty() {
                text(message, "result")
                    .unwrap_or("The turn failed.")
                    .to_owned()
            } else {
                errors.join("\n")
            }
        });
        let end = TurnEnd {
            cost_usd: message
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            duration_ms: number(message, "duration_ms"),
            turns: number(message, "num_turns"),
            input_tokens,
            output_tokens,
            error,
        };
        self.cost_usd += end.cost_usd;
        self.live = None;
        self.busy = false;
        self.push(ItemKind::Turn(end), None);
    }

    /// A rate limit reached, or near.
    fn rate_limit(&mut self, message: &Value) {
        let info = message.get("rate_limit_info");
        let status = info.and_then(|info| text(info, "status"));
        let level = match status {
            Some("rejected") => Level::Error,
            Some("allowed_warning") => Level::Warning,
            _ => return,
        };
        let text = if level == Level::Error {
            "The account's usage limit is reached."
        } else {
            "The account is close to its usage limit."
        };
        self.notice(level, text);
    }

    /// Adds an item; returns its index.
    fn push(&mut self, kind: ItemKind, parent: Option<String>) -> usize {
        self.version = self.version.saturating_add(1);
        self.items.push(Item {
            kind,
            parent,
            changed: self.version,
        });
        self.items.len().saturating_sub(1)
    }

    const fn bump(&mut self) {
        self.version = self.version.saturating_add(1);
    }
}

impl ToolCall {
    /// What it does, in a few words, and what to: `("Read", "src/main.rs")`, `("Run", "cargo
    /// test")`. The path or command as given; the first line of a command.
    #[must_use]
    pub fn summary(&self) -> (&str, String) {
        let field = |name: &str| text(&self.input, name).unwrap_or_default();
        let first_line = |text: &str| text.lines().next().unwrap_or_default().to_owned();
        match self.name.as_str() {
            "Read" => ("Read", field("file_path").to_owned()),
            "Edit" | "MultiEdit" => ("Edit", field("file_path").to_owned()),
            "Write" => ("Write", field("file_path").to_owned()),
            "NotebookEdit" => ("Edit", field("notebook_path").to_owned()),
            "Bash" | "PowerShell" => ("Run", first_line(field("command"))),
            "Grep" => ("Search", field("pattern").to_owned()),
            "Glob" => ("Find", field("pattern").to_owned()),
            "WebFetch" => ("Fetch", field("url").to_owned()),
            "WebSearch" => ("Search the web", field("query").to_owned()),
            "TodoWrite" => ("Plan", String::new()),
            "Agent" | "Task" => ("Agent", field("description").to_owned()),
            "AskUserQuestion" => ("Ask", String::new()),
            "ExitPlanMode" => ("Propose the plan", String::new()),
            "Skill" => ("Use the skill", field("skill").to_owned()),
            name => (name, String::new()),
        }
    }
}

/// A string field of `value`.
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// A number field of `value` (0 when missing).
fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// The blocks of a message's content (none when it is a string).
fn blocks(content: &Value) -> impl Iterator<Item = &Value> {
    content.as_array().into_iter().flatten()
}

/// A tool result's text: a string, or its text blocks joined.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| text(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether a user message in a past session is Claude Code's own (a command's output, a
/// reminder), not something the user typed.
fn is_internal(prompt: &str) -> bool {
    let prompt = prompt.trim_start();
    [
        "<command-",
        "<local-command-",
        "<system-reminder>",
        "Caveat:",
    ]
    .iter()
    .any(|start| prompt.starts_with(start))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn kinds(transcript: &Transcript) -> Vec<ItemKind> {
        transcript
            .items()
            .iter()
            .map(|item| item.kind.clone())
            .collect()
    }

    #[test]
    fn a_turn_streams_then_is_made_whole() {
        let mut transcript = Transcript::default();
        transcript.message(&json!({
            "type": "system", "subtype": "init", "session_id": "s1",
            "model": "claude-test", "permissionMode": "acceptEdits"
        }));
        assert_eq!(
            (transcript.session(), transcript.mode()),
            (Some("s1"), Some(Mode::AcceptEdits))
        );
        transcript.prompt("List the files");
        assert!(transcript.busy());
        let stream = |event: Value| json!({"type": "stream_event", "event": event, "parent_tool_use_id": null});
        transcript.message(&stream(
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ));
        transcript.message(&stream(
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Look"}}),
        ));
        transcript.message(&stream(
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "ing"}}),
        ));
        assert_eq!(
            kinds(&transcript).last(),
            Some(&ItemKind::Text("Looking".to_owned()))
        );
        let before = transcript.version();
        // The whole message replaces what streamed; then a tool call and its result.
        transcript.message(&json!({"type": "assistant", "parent_tool_use_id": null,
            "message": {"content": [{"type": "text", "text": "Looking now."}]}}));
        assert!(transcript.version() > before);
        transcript.message(&json!({"type": "assistant", "parent_tool_use_id": null,
            "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "ls\n-la"}}]}}));
        transcript.message(&json!({"type": "user", "parent_tool_use_id": null,
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t1",
                "content": [{"type": "text", "text": "a\nb"}], "is_error": false}]}}));
        transcript.message(
            &json!({"type": "result", "subtype": "success", "is_error": false,
            "duration_ms": 1200, "num_turns": 2, "total_cost_usd": 0.25, "result": "done",
            "modelUsage": {"claude-test": {"inputTokens": 10, "cacheReadInputTokens": 90,
                "cacheCreationInputTokens": 0, "outputTokens": 5}}}),
        );
        assert!(!transcript.busy());

        let items = kinds(&transcript);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], ItemKind::Prompt("List the files".to_owned()));
        assert_eq!(items[1], ItemKind::Text("Looking now.".to_owned()));
        let ItemKind::Tool(call) = &items[2] else {
            panic!("{items:?}");
        };
        assert_eq!(call.summary(), ("Run", "ls".to_owned()));
        assert_eq!(
            call.result,
            Some(ToolResult {
                text: "a\nb".to_owned(),
                error: false
            })
        );
        assert_eq!(
            items[3],
            ItemKind::Turn(TurnEnd {
                cost_usd: 0.25,
                duration_ms: 1200,
                turns: 2,
                input_tokens: 100,
                output_tokens: 5,
                error: None
            })
        );
        assert!((transcript.cost_usd() - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn failures_and_limits_are_noticed() {
        let mut transcript = Transcript::default();
        transcript.prompt("Go");
        transcript.message(&json!({"type": "rate_limit_event",
            "rate_limit_info": {"status": "allowed"}}));
        transcript.message(&json!({"type": "rate_limit_event",
            "rate_limit_info": {"status": "rejected"}}));
        transcript.message(
            &json!({"type": "result", "subtype": "error_during_execution",
            "is_error": true, "errors": ["interrupted"], "total_cost_usd": 0.0}),
        );
        transcript.message(&json!({"type": "something_new", "anything": [1, 2]}));
        let items = kinds(&transcript);
        assert_eq!(items.len(), 3);
        assert!(matches!(
            &items[1],
            ItemKind::Notice(Notice {
                level: Level::Error,
                ..
            })
        ));
        let ItemKind::Turn(end) = &items[2] else {
            panic!("{items:?}");
        };
        assert_eq!(end.error.as_deref(), Some("interrupted"));
    }

    #[test]
    fn a_past_session_reads_as_it_went() {
        let mut transcript = Transcript::default();
        transcript.history(&[
            json!({"type": "user", "parent_tool_use_id": null,
                "message": {"role": "user", "content": "Fix the build"}}),
            json!({"type": "user", "parent_tool_use_id": null,
                "message": {"role": "user", "content": "<command-name>/clear</command-name>"}}),
            json!({"type": "assistant", "parent_tool_use_id": null,
                "message": {"content": [{"type": "thinking", "thinking": "Hmm."},
                    {"type": "tool_use", "id": "t1", "name": "Read",
                        "input": {"file_path": "/w/src/main.rs"}}]}}),
            json!({"type": "user", "parent_tool_use_id": null,
                "message": {"role": "user", "content": [{"type": "tool_result",
                    "tool_use_id": "t1", "content": "fn main() {}", "is_error": false}]}}),
            json!({"type": "assistant", "parent_tool_use_id": "t9",
                "message": {"content": [{"type": "text", "text": "A subagent's word."}]}}),
        ]);
        let items = transcript.items();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].kind, ItemKind::Prompt("Fix the build".to_owned()));
        assert_eq!(items[1].kind, ItemKind::Thinking("Hmm.".to_owned()));
        let ItemKind::Tool(call) = &items[2].kind else {
            panic!("{items:?}");
        };
        assert_eq!(call.summary(), ("Read", "/w/src/main.rs".to_owned()));
        assert!(call.result.is_some());
        assert_eq!(items[3].parent.as_deref(), Some("t9"));
    }
}
