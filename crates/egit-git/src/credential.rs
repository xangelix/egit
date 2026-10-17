//! Credentials as git's helpers keep them. `git credential fill` asks the configured helpers
//! (and the user, when allowed to) for a place's username and password, `approve` has the
//! helpers keep what worked, and `reject` has them forget what didn't. egit signs in to a
//! forge's web API with them, as the user's git signs in to its remotes, and keeps no secret of
//! its own.
//!
//! The protocol is `key=value` lines on standard input and output, ending at an empty line.
//! What `fill` prints is kept as it is, so `approve` and `reject` hand the helpers back every
//! attribute they gave (an expiry, a refresh token), and wiped from memory when dropped.

use std::fmt;
use std::ops::Range;

use bstr::ByteSlice as _;
use zeroize::Zeroizing;

use crate::cancel::CancelToken;
use crate::command::GitCommand;
use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// Where a credential is for: a protocol and a host (with its port, if any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialPlace<'a> {
    /// `https`, say.
    pub protocol: &'a str,
    /// `github.com`, or `example.com:8443`.
    pub host: &'a str,
}

impl CredentialPlace<'_> {
    /// Whether git would take these as attribute values: no line breaks or NULs, which would
    /// let one attribute pass for others.
    fn is_valid(self) -> bool {
        let fits = |value: &str| {
            !value.is_empty() && !value.bytes().any(|byte| matches!(byte, b'\n' | b'\r' | 0))
        };
        fits(self.protocol) && fits(self.host)
    }
}

/// Whether filling a credential may ask the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asking<'a> {
    /// Never: only what the helpers already keep (work nobody is watching).
    Never,
    /// Through egit's prompts (and any helper's own windows), labelled with what is being done.
    As(&'a str),
}

/// A username and password from git's credential helpers (or the user), with every attribute
/// the helpers gave; wiped from memory when dropped, and never shown in debug output.
pub struct Credential {
    attributes: Zeroizing<Vec<u8>>,
    username: Range<usize>,
    password: Range<usize>,
}

impl Credential {
    /// Reads what `git credential fill` printed; `None` without a password.
    fn parse(attributes: Zeroizing<Vec<u8>>) -> Option<Self> {
        let mut username = 0..0;
        let mut password = None;
        let mut start = 0_usize;
        for line in attributes.split(|&byte| byte == b'\n') {
            let end = start.saturating_add(line.len());
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix(b"username=") {
                username = end.saturating_sub(value.len())..end;
            } else if let Some(value) = line.strip_prefix(b"password=") {
                password = Some(end.saturating_sub(value.len())..end);
            }
            start = end.saturating_add(1);
        }
        let password = password.filter(|range| !range.is_empty())?;
        Some(Self {
            attributes,
            username,
            password,
        })
    }

    /// The username (empty if the helpers gave none).
    #[must_use]
    pub fn username(&self) -> &str {
        self.attributes
            .get(self.username.clone())
            .and_then(|bytes| bytes.to_str().ok())
            .unwrap_or_default()
    }

    /// The password (or token): the secret. Not UTF-8, it is empty.
    #[must_use]
    pub fn password(&self) -> &str {
        self.attributes
            .get(self.password.clone())
            .and_then(|bytes| bytes.to_str().ok())
            .unwrap_or_default()
    }

    /// The attributes as `fill` printed them, ended by an empty line: what `approve` and
    /// `reject` read.
    fn description(&self) -> Zeroizing<Vec<u8>> {
        let mut description =
            Zeroizing::new(Vec::with_capacity(self.attributes.len().saturating_add(2)));
        let end = self
            .attributes
            .find(b"\n\n")
            .map_or(self.attributes.len(), |at| at.saturating_add(1));
        description.extend_from_slice(self.attributes.get(..end).unwrap_or_default());
        if !description.ends_with(b"\n") {
            description.push(b'\n');
        }
        description.push(b'\n');
        description
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username())
            .field("password", &"<secret>")
            .finish()
    }
}

/// Asks git's credential helpers for `place`'s credential, and the user as `asking` allows.
///
/// The configuration is `repository`'s when given (a repository may name helpers of its own),
/// the user's otherwise. `None` when there is none: no helper has one, and asking wasn't
/// allowed or the user declined.
///
/// # Errors
///
/// When git fails otherwise (a helper that breaks, say) or is cancelled.
pub fn fill_credential(
    git: &Git,
    repository: Option<&Repository>,
    place: CredentialPlace<'_>,
    asking: Asking<'_>,
    cancel: &CancelToken,
) -> Result<Option<Credential>, GitError> {
    if !place.is_valid() {
        tracing::warn!(?place, "not asking for a credential for an unusable place");
        return Ok(None);
    }
    let mut request = Zeroizing::new(Vec::new());
    request.extend_from_slice(b"protocol=");
    request.extend_from_slice(place.protocol.as_bytes());
    request.extend_from_slice(b"\nhost=");
    request.extend_from_slice(place.host.as_bytes());
    request.extend_from_slice(b"\n\n");
    let command = credential_command(git, repository, "fill")
        .secret_stdin(request)
        .cancel_token(cancel);
    let command = match asking {
        Asking::Never => command.non_interactive(),
        Asking::As(context) => command.prompt_context(context),
    };
    let filled = command.secret_output()?;
    if filled.status.success() {
        return Ok(Credential::parse(filled.stdout));
    }
    let message = filled.stderr.message();
    // Nothing kept and nobody asked (or the user declined): git gives up reading one.
    if message.contains("could not read") || message.contains("told us to quit") {
        return Ok(None);
    }
    Err(GitError::failed("credential", filled.status, filled.stderr))
}

/// Has git's credential helpers keep `credential`: it worked.
///
/// # Errors
///
/// When git fails.
pub fn approve_credential(
    git: &Git,
    repository: Option<&Repository>,
    credential: &Credential,
) -> Result<(), GitError> {
    credential_command(git, repository, "approve")
        .secret_stdin(credential.description())
        .output()
        .map(drop)
}

/// Has git's credential helpers forget `credential`: it was refused.
///
/// # Errors
///
/// When git fails.
pub fn reject_credential(
    git: &Git,
    repository: Option<&Repository>,
    credential: &Credential,
) -> Result<(), GitError> {
    credential_command(git, repository, "reject")
        .secret_stdin(credential.description())
        .output()
        .map(drop)
}

/// `git credential <action>` in the repository's work tree, or in `/` for the user's
/// configuration alone.
fn credential_command(git: &Git, repository: Option<&Repository>, action: &str) -> GitCommand {
    git.read("credential")
        .arg(action)
        .current_dir(repository.map_or_else(
            || crate::git::outside_repositories().to_owned(),
            |repo| repo.work_tree().to_owned(),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Option<Credential> {
        Credential::parse(Zeroizing::new(text.as_bytes().to_vec()))
    }

    #[test]
    fn a_filled_credential_keeps_every_attribute() {
        let credential = parsed(
            "protocol=https\nhost=github.com\nusername=ada\npassword=s3cret\n\
             password_expiry_utc=1700000000\n",
        )
        .unwrap();
        assert_eq!(credential.username(), "ada");
        assert_eq!(credential.password(), "s3cret");
        assert_eq!(
            credential.description().as_slice(),
            b"protocol=https\nhost=github.com\nusername=ada\npassword=s3cret\n\
              password_expiry_utc=1700000000\n\n"
        );
        assert!(!format!("{credential:?}").contains("s3cret"));
    }

    #[test]
    fn no_password_is_no_credential() {
        assert!(parsed("protocol=https\nhost=github.com\nusername=ada\n").is_none());
        assert!(parsed("protocol=https\nhost=github.com\npassword=\n").is_none());
    }

    #[test]
    fn what_follows_an_empty_line_is_ignored() {
        let credential = parsed("username=a\npassword=b\n\npassword=c\n").unwrap();
        assert_eq!(credential.password(), "b");
        assert_eq!(
            credential.description().as_slice(),
            b"username=a\npassword=b\n\n"
        );
    }

    #[test]
    fn places_that_would_inject_attributes_are_refused() {
        let place = |host| CredentialPlace {
            protocol: "https",
            host,
        };
        assert!(place("github.com").is_valid());
        assert!(place("example.com:8443").is_valid());
        assert!(!place("evil.com\npassword=x").is_valid());
        assert!(!place("").is_valid());
    }
}
