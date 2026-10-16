//! Prompts from git's and ssh's helpers, relayed to egit's window.
//!
//! Git asks for credentials it has no helper for through `GIT_ASKPASS`, and ssh asks for
//! passphrases, PINs, and decisions about unknown hosts through `SSH_ASKPASS`. egit points both
//! at itself: [`AskpassServer`] makes a directory private to the user holding a Unix socket and
//! a link to egit's executable named [`HELPER_NAME`], and [`AskpassServer::environment`] puts
//! them in every git process's environment. When git or ssh runs the link, egit starts as the
//! helper ([`run_helper`]): it sends the prompt over the socket, waits for the window's
//! [`Prompt`] to be answered, and prints the answer.
//!
//! Only the user's own processes reach the socket (the directory is `0700`, and the peer's
//! uid is checked), each connection must carry the random token from the environment, every
//! field is bounded, and answers are wiped from memory once written.

mod client;
mod protocol;
mod server;

pub use client::run_helper;
pub use protocol::PromptKind;
pub use server::{AskpassServer, Prompt};

/// The name of the helper. egit runs as the helper when started under this name.
pub const HELPER_NAME: &str = "egit-askpass";

/// The socket the helper connects to.
pub(crate) const SOCKET_VAR: &str = "EGIT_ASKPASS_SOCKET";
/// The token the helper presents.
pub(crate) const TOKEN_VAR: &str = "EGIT_ASKPASS_TOKEN";
/// What the command asking was for (`GitCommand::prompt_context`).
pub(crate) const CONTEXT_VAR: &str = "EGIT_ASKPASS_CONTEXT";
/// `0` when the command may not ask anyone (`GitCommand::non_interactive`).
pub(crate) const INTERACTIVE_VAR: &str = "EGIT_ASKPASS_INTERACTIVE";

/// Asks through a server as a helper started with `environment` would (the `testing`
/// feature): for tests of whatever shows the prompts.
///
/// # Errors
///
/// When the environment names no server, or it can't be reached.
#[cfg(feature = "testing")]
pub fn ask_as_helper(
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    kind: PromptKind,
    prompt: &str,
    context: &str,
) -> std::io::Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
    let find = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    let missing = || std::io::Error::other("the environment names no askpass server");
    let socket = find(SOCKET_VAR).ok_or_else(missing)?;
    let token = find(TOKEN_VAR)
        .and_then(|token| protocol::Token::from_hex(token.as_encoded_bytes()))
        .ok_or_else(missing)?;
    client::ask(
        std::path::Path::new(socket),
        &token,
        kind,
        prompt.as_bytes(),
        context.as_bytes(),
    )
}

#[cfg(test)]
mod tests;
