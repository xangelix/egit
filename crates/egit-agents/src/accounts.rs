//! Who a conversation runs as.
//!
//! egit has no Claude login of its own (the Agent SDK's
//! documentation: "Unless previously approved, Anthropic does not allow third party developers
//! to offer claude.ai login or rate limits for their products"): a [`Signer`] is a Claude Code
//! configuration folder, signed into with Claude Code itself, or an Anthropic API key.
//!
//! Each folder keeps its own login, settings, and sessions (`CLAUDE_CONFIG_DIR`), so several
//! accounts run at once, each conversation as its own. [`auth_status`] asks Claude Code who is
//! signed in to one.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use egit_git::{Git, Running};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::AgentError;

/// Variables through which Claude Code finds credentials other than a folder's login.
const OTHER_CREDENTIALS: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Who a conversation runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signer {
    /// Claude Code as the terminal runs it: its own folder (`~/.claude`, or the one egit's
    /// environment names) and whatever credentials that environment holds.
    Default,
    /// A Claude Code configuration folder of its own, as signed into there (other credentials
    /// in egit's environment left out).
    Folder(PathBuf),
    /// An Anthropic API key (billed by the Console).
    ApiKey(Zeroizing<String>),
}

impl Signer {
    /// The variables the bridge (and Claude Code under it) needs to run as it, and those it
    /// mustn't have.
    #[must_use]
    pub fn environment(&self) -> (Vec<(OsString, OsString)>, Vec<OsString>) {
        let others = || OTHER_CREDENTIALS.iter().map(OsString::from);
        match self {
            Self::Default => (Vec::new(), Vec::new()),
            Self::Folder(folder) => (
                vec![("CLAUDE_CONFIG_DIR".into(), folder.clone().into_os_string())],
                others().collect(),
            ),
            Self::ApiKey(key) => (
                vec![("ANTHROPIC_API_KEY".into(), OsString::from(key.as_str()))],
                others().skip(1).collect(),
            ),
        }
    }
}

/// Who is signed in, as Claude Code says (`claude auth status --json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthStatus {
    /// Whether anyone is.
    pub logged_in: bool,
    /// How (`claude.ai`, `api_key`, `oauth_token`, `none`, …).
    pub auth_method: String,
    /// Their email, when Claude Code says.
    pub email: Option<String>,
    /// Their organization.
    #[serde(alias = "orgName")]
    pub organization: Option<String>,
    /// Their plan (`max`, `pro`, `team`).
    pub subscription_type: Option<String>,
    /// The configuration folder asked about.
    pub config_directory: Option<String>,
}

/// Asks Claude Code (`claude`, the binary the SDK installed) who is signed in for `signer`
/// (`there`: on the other computer that git runs on).
///
/// # Errors
///
/// When it can't be run, or what it says can't be read; [`AgentError::SecretOnCommandLine`]
/// for an API key over ssh (a relay hands a program its variables apart).
pub async fn auth_status(
    claude: &Path,
    signer: &Signer,
    there: Option<&Git>,
) -> Result<AuthStatus, AgentError> {
    let (set, unset) = signer.environment();
    let args = ["auth", "status", "--json"].map(OsStr::new);
    if there.is_some_and(|git| git.host().ssh().is_some()) && matches!(signer, Signer::ApiKey(_)) {
        // A command line there would show it.
        return Err(AgentError::SecretOnCommandLine);
    }
    let mut running = if let Some(git) = there {
        git.run_there(claude.as_os_str(), &args, None, (&set, &unset), false)?
    } else {
        let mut command = Command::new(claude);
        command.args(args);
        for key in &unset {
            command.env_remove(key);
        }
        command.envs(set.iter().map(|(key, value)| (key, value)));
        Running::here(command, false)?
    };
    // Not signed in, it says so and exits 1: what it prints is the answer either way.
    let said = running.read_to_end(1 << 20).await?;
    running.wait().await?;
    Ok(serde_json::from_slice(&said)?)
}

/// The command signing into `folder` with Claude Code itself, to run in a terminal (`there`:
/// on the other computer ssh names so, through `ssh -t`).
#[must_use]
pub fn login_command(claude: &Path, folder: Option<&Path>, there: Option<&str>) -> String {
    if let Some(host) = there {
        let inner = login_command_unix(claude, folder);
        return format!("ssh -t {host} \"{inner}\"");
    }
    if cfg!(windows) {
        return folder.map_or_else(
            || format!("& \"{}\" auth login", claude.display()),
            |folder| {
                format!(
                    "$env:CLAUDE_CONFIG_DIR = \"{}\"; & \"{}\" auth login",
                    folder.display(),
                    claude.display()
                )
            },
        );
    }
    login_command_unix(claude, folder)
}

/// The command signing into `folder`, for a POSIX shell.
fn login_command_unix(claude: &Path, folder: Option<&Path>) -> String {
    let quoted = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let binary = quoted(&claude.to_string_lossy());
    folder.map_or_else(
        || format!("{binary} auth login"),
        |folder| {
            format!(
                "CLAUDE_CONFIG_DIR={} {binary} auth login",
                quoted(&folder.to_string_lossy())
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_signer_has_its_own_credentials() {
        assert_eq!(Signer::Default.environment(), (Vec::new(), Vec::new()));
        let (set, unset) = Signer::Folder("/c/work".into()).environment();
        assert_eq!(set, [("CLAUDE_CONFIG_DIR".into(), "/c/work".into())]);
        assert_eq!(unset.len(), 3);
        let (set, unset) = Signer::ApiKey(Zeroizing::new("sk-test".to_owned())).environment();
        assert_eq!(set, [("ANTHROPIC_API_KEY".into(), "sk-test".into())]);
        assert!(!unset.contains(&"ANTHROPIC_API_KEY".into()));

        let status: AuthStatus = serde_json::from_str(
            r#"{"loggedIn":true,"authMethod":"claude.ai","email":"a@b.c","orgName":"Team","subscriptionType":"max","new":1}"#,
        )
        .unwrap();
        assert!(status.logged_in);
        assert_eq!(status.organization.as_deref(), Some("Team"));
        if cfg!(unix) {
            assert_eq!(
                login_command(
                    Path::new("/opt/it's/claude"),
                    Some(Path::new("/c/work")),
                    None
                ),
                r"CLAUDE_CONFIG_DIR='/c/work' '/opt/it'\''s/claude' auth login"
            );
        }
        assert_eq!(
            login_command(Path::new("/c/claude"), None, Some("devbox")),
            r#"ssh -t devbox "'/c/claude' auth login""#
        );
        if cfg!(unix) {}
    }
}
