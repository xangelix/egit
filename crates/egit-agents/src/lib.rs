//! egit's agents: Claude Code working in a repository beside egit, driven through Anthropic's
//! Claude Agent SDK, which runs Claude Code itself (its system prompt, tools, settings,
//! CLAUDE.md files, hooks, MCP servers, sessions). egit reimplements none of it: a small bridge
//! (`bridge/claude/bridge.mjs`) runs the SDK under Node and speaks JSON lines with egit
//! ([`protocol`]), and [`Transcript`] reads what Claude Code says into what egit shows.
//!
//! - [`install`]: Node found, the bridge and the SDK installed into egit's cache by `npm ci`.
//! - [`Bridge`]: a conversation's process; [`run_once`] for one-shot questions.
//! - [`accounts`]: who a conversation runs as (a Claude Code folder's login, or an API key).
//! - [`sessions`]: the conversations kept for a folder, and one's messages.

pub mod accounts;
mod bridge;
pub mod install;
pub mod protocol;
pub mod sessions;
mod transcript;

pub use bridge::{Bridge, Launch, run_once};
pub use transcript::{Item, ItemKind, Level, Notice, ToolCall, ToolResult, Transcript, TurnEnd};

/// What can go wrong with an agent.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Node.js (or npm) isn't on `PATH`.
    #[error("Node.js and npm weren't found")]
    NodeMissing,
    /// Node.js is older than the SDK needs.
    #[error("Node.js {0} is too old: the Claude Agent SDK needs version {min} or newer", min = install::NODE_MINIMUM)]
    NodeTooOld(String),
    /// Installing the SDK failed.
    #[error("{0}")]
    Install(String),
    /// Stopped on request.
    #[error("cancelled")]
    Cancelled,
    /// The bridge has gone.
    #[error("the conversation has ended")]
    Gone,
    /// The bridge failed, saying why.
    #[error("{0}")]
    Bridge(String),
    /// What the bridge (or Claude Code) printed couldn't be read.
    #[error("couldn't read the answer: {0}")]
    Unreadable(#[from] serde_json::Error),
    /// A file or process couldn't be handled.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
