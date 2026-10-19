//! The conversations Claude Code keeps for a folder, and what was said in one.
//!
//! The terminal's and editors' are among them (they share its sessions). They are read through
//! the SDK (`listSessions`, `getSessionMessages`) by the bridge run once.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::AgentError;
use crate::bridge::{Launch, run_once};

/// A conversation kept for a folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionInfo {
    /// Its id.
    pub session_id: String,
    /// Its title: one given it, a summary, or its first prompt.
    pub summary: String,
    /// When it last changed, in milliseconds since 1970.
    pub last_modified: i64,
    /// The branch checked out when it ended.
    pub git_branch: Option<String>,
    /// The folder it ran in.
    pub cwd: Option<String>,
}

/// The newest `limit` conversations kept for `dir` (and its worktrees), newest first. Blocks:
/// run it in a job.
///
/// # Errors
///
/// When the bridge fails.
pub fn list_sessions(
    launch: &Launch,
    dir: &Path,
    limit: usize,
) -> Result<Vec<SessionInfo>, AgentError> {
    let answer = run_once(
        launch,
        &["sessions", &dir.to_string_lossy(), &limit.to_string()],
    )?;
    let mut sessions: Vec<SessionInfo> = serde_json::from_value(answer)?;
    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    Ok(sessions)
}

/// What was said in conversation `session` of `dir`, for [`crate::Transcript::history`]. Blocks:
/// run it in a job.
///
/// # Errors
///
/// When the bridge fails.
pub fn session_history(
    launch: &Launch,
    dir: &Path,
    session: &str,
) -> Result<Vec<Value>, AgentError> {
    let answer = run_once(launch, &["history", &dir.to_string_lossy(), session])?;
    Ok(serde_json::from_value(answer)?)
}
