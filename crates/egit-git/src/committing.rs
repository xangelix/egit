//! Making a commit of what the index holds: `git commit`, so hooks, signing (OpenPGP, SSH,
//! X.509, or any program the user configured), and trailers work as they do on the command
//! line.

use bstr::ByteSlice as _;

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// How a commit is made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewCommit {
    /// Its message. Only whitespace is cleaned up (`--cleanup=whitespace`): a line starting
    /// with `#` is the user's, not a comment.
    pub message: String,
    /// Replace `HEAD` (its message, with the index's changes) instead of adding a commit.
    pub amend: bool,
    /// Add a `Signed-off-by` trailer naming the committer.
    pub sign_off: bool,
    /// Sign it (`-S`), don't (`--no-gpg-sign`), or as `commit.gpgSign` says (`None`).
    pub sign: Option<bool>,
    /// Trailers to add (`Co-authored-by: Name <email>`), each by `--trailer`: git places them
    /// with any the message ends with, after a blank line.
    pub trailers: Vec<String>,
}

/// A commit made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    /// What git and the hooks it ran printed (hooks print to standard error).
    pub output: String,
}

/// Commits what the index holds, as `commit` says. A key's passphrase is asked for through
/// git's askpass, labelled as signing the commit.
///
/// # Errors
///
/// When git refuses (nothing to commit, no identity, a hook said no, signing failed: the
/// failure's standard error says which, a hook's output included) or is cancelled.
pub fn create_commit(
    git: &Git,
    repository: &Repository,
    commit: &NewCommit,
    cancel: &CancelToken,
) -> Result<Committed, GitError> {
    let mut command = git
        .write("commit")
        .args(["--file=-", "--cleanup=whitespace"])
        .prompt_context("Signing the commit")
        .current_dir(repository.work_tree())
        .stdin(commit.message.clone().into_bytes())
        .cancel_token(cancel);
    if commit.amend {
        command = command.arg("--amend");
    }
    if commit.sign_off {
        command = command.arg("--signoff");
    }
    for trailer in &commit.trailers {
        command = command.arg("--trailer").arg(trailer);
    }
    match commit.sign {
        Some(true) => command = command.arg("--gpg-sign"),
        Some(false) => command = command.arg("--no-gpg-sign"),
        None => {}
    }
    let done = command.output()?;
    let mut output = done.stderr.as_bytes().to_str_lossy().into_owned();
    output.push_str(&done.stdout.to_str_lossy());
    Ok(Committed {
        output: output.trim().to_owned(),
    })
}

/// Gives `HEAD` a new message, and nothing else.
///
/// `git commit --amend --only` (no paths: what is staged stays staged, out of it), the message
/// written as `create_commit` writes it (whitespace cleaned up, a line starting with `#` kept),
/// `--allow-empty` (a commit changing nothing keeps changing nothing), signed as the
/// configuration says, its hooks run.
///
/// # Errors
///
/// When git refuses (a hook, signing), or can't be run.
pub fn reword_head(
    git: &Git,
    repository: &Repository,
    message: &str,
    cancel: &CancelToken,
) -> Result<Committed, GitError> {
    let done = git
        .write("commit")
        .args([
            "--amend",
            "--only",
            "--allow-empty",
            "--file=-",
            "--cleanup=whitespace",
        ])
        .prompt_context("Signing the commit")
        .current_dir(repository.work_tree())
        .stdin(message.as_bytes().to_vec())
        .cancel_token(cancel)
        .output()?;
    let mut output = done.stderr.as_bytes().to_str_lossy().into_owned();
    output.push_str(&done.stdout.to_str_lossy());
    Ok(Committed {
        output: output.trim().to_owned(),
    })
}
