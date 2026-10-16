//! The helper's side: the process git or ssh starts to ask the user something.

use std::ffi::OsStr;
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;

use bstr::ByteSlice as _;
use zeroize::Zeroizing;

use super::protocol::{self, PromptKind, Token};
use super::{CONTEXT_VAR, INTERACTIVE_VAR, SOCKET_VAR, TOKEN_VAR};

/// Runs as git's or ssh's askpass helper: sends `prompt` (the argument git or ssh passed) to
/// egit's window and prints the answer for the asker to read.
///
/// Returns the code the process should exit with: failure when the user refused, when the
/// command may not ask (`non_interactive`), or when the window can't be reached.
#[must_use]
pub fn run_helper(prompt: &OsStr) -> ExitCode {
    let var = std::env::var_os;
    if var(INTERACTIVE_VAR).is_some_and(|value| value == "0") {
        return ExitCode::FAILURE;
    }
    let token = var(TOKEN_VAR).and_then(|token| Token::from_hex(token.as_bytes()));
    let (Some(socket), Some(token)) = (var(SOCKET_VAR), token) else {
        complain("not started by egit");
        return ExitCode::FAILURE;
    };
    let kind = classify(prompt.as_bytes(), var("SSH_ASKPASS_PROMPT").as_deref());
    let context = var(CONTEXT_VAR).unwrap_or_default();

    match ask(
        Path::new(&socket),
        &token,
        kind,
        prompt.as_bytes(),
        context.as_bytes(),
    ) {
        Ok(Some(answer)) => match kind {
            // Answered by the exit code alone.
            PromptKind::Confirm | PromptKind::Notice => ExitCode::SUCCESS,
            PromptKind::Visible | PromptKind::Secret | PromptKind::YesNo => {
                let mut stdout = io::stdout().lock();
                let printed = stdout
                    .write_all(&answer)
                    .and_then(|()| stdout.write_all(b"\n"))
                    .and_then(|()| stdout.flush());
                match printed {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        complain(&error.to_string());
                        ExitCode::FAILURE
                    }
                }
            }
        },
        Ok(None) => ExitCode::FAILURE,
        Err(error) => {
            complain(&error.to_string());
            ExitCode::FAILURE
        }
    }
}

/// Sends a prompt to the window at `socket` and waits for the answer: `Some` answered, `None`
/// refused.
pub(crate) fn ask(
    socket: &Path,
    token: &Token,
    kind: PromptKind,
    prompt: &[u8],
    context: &[u8],
) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    let mut stream = UnixStream::connect(socket)?;
    protocol::write_request(&mut stream, token, kind, prompt, context)?;
    protocol::read_response(&mut stream)
}

/// What a prompt asks for, from its text and the `SSH_ASKPASS_PROMPT` ssh sets for prompts
/// that aren't typed answers.
pub(crate) fn classify(prompt: &[u8], ssh_askpass_prompt: Option<&OsStr>) -> PromptKind {
    match ssh_askpass_prompt.map(OsStrExt::as_bytes) {
        Some(b"confirm") => PromptKind::Confirm,
        Some(b"none") => PromptKind::Notice,
        _ if prompt.find(b"(yes/no").is_some() => PromptKind::YesNo,
        _ if prompt.starts_with(b"Username") => PromptKind::Visible,
        // Anything else may be a secret: hide it.
        _ => PromptKind::Secret,
    }
}

/// Tells whoever reads the asker's stderr (git passes it on) why the helper failed.
fn complain(reason: &str) {
    // Nothing more can be done if stderr is gone too.
    let _ = writeln!(io::stderr(), "egit-askpass: {reason}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_git_and_ssh_prompts() {
        let classify_text = |text: &str| classify(text.as_bytes(), None);
        assert_eq!(
            classify_text("Username for 'https://example.com': "),
            PromptKind::Visible
        );
        assert_eq!(
            classify_text("Password for 'https://me@example.com': "),
            PromptKind::Secret
        );
        assert_eq!(
            classify_text("Enter passphrase for key '/home/me/.ssh/id_ed25519': "),
            PromptKind::Secret
        );
        assert_eq!(
            classify_text(
                "The authenticity of host 'example.com' can't be established.\n\
                 Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            PromptKind::YesNo
        );
        assert_eq!(
            classify_text("Enter PIN for authenticator: "),
            PromptKind::Secret
        );
    }

    #[test]
    fn ssh_says_when_nothing_is_typed() {
        assert_eq!(
            classify(b"Allow use of key?", Some(OsStr::new("confirm"))),
            PromptKind::Confirm
        );
        assert_eq!(
            classify(b"Confirm user presence for key", Some(OsStr::new("none"))),
            PromptKind::Notice
        );
        assert_eq!(
            classify(b"Enter passphrase: ", Some(OsStr::new("something-new"))),
            PromptKind::Secret
        );
    }
}
