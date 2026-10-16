//! The settings that make up a person's git profile: who they commit as, how they sign, how git
//! asks for their credentials, and how pull and push behave.
//!
//! Each setting comes with the [`ConfigEntry`] it was read from, so its scope and file can be
//! shown next to it.

use bstr::BStr;

use crate::config::{ConfigEntry, ConfigSnapshot, parse_bool};

/// A profile read from a [`ConfigSnapshot`].
#[derive(Clone, Copy, Debug)]
pub struct Profile<'a> {
    snapshot: &'a ConfigSnapshot,
}

impl ConfigSnapshot {
    /// The profile these settings make up.
    #[must_use]
    pub const fn profile(&self) -> Profile<'_> {
        Profile { snapshot: self }
    }
}

/// A setting and the entry it comes from (absent when git's default applies).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting<'a, T> {
    /// The value in effect.
    pub value: T,
    /// Where it is set; `None` means git's default.
    pub entry: Option<ConfigEntry<'a>>,
}

impl<'a, T> Setting<'a, T> {
    const fn default(value: T) -> Self {
        Self { value, entry: None }
    }

    const fn from(value: T, entry: ConfigEntry<'a>) -> Self {
        Self {
            value,
            entry: Some(entry),
        }
    }
}

/// How commits and tags are signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningFormat {
    /// OpenPGP, through `gpg` (git's default).
    OpenPgp,
    /// X.509, through `gpgsm`.
    X509,
    /// An SSH key, through `ssh-keygen`.
    Ssh,
}

impl SigningFormat {
    /// The value `gpg.format` takes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OpenPgp => "openpgp",
            Self::X509 => "x509",
            Self::Ssh => "ssh",
        }
    }

    const fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"openpgp" => Some(Self::OpenPgp),
            b"x509" => Some(Self::X509),
            b"ssh" => Some(Self::Ssh),
            _ => None,
        }
    }

    /// The key naming the program that signs in this format.
    const fn program_key(self) -> &'static str {
        match self {
            Self::OpenPgp => "gpg.openpgp.program",
            Self::X509 => "gpg.x509.program",
            Self::Ssh => "gpg.ssh.program",
        }
    }
}

/// What `pull.rebase` asks a pull to do with local commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRebase {
    /// Merge the upstream (`false`).
    Merge,
    /// Rebase onto the upstream (`true`).
    Rebase,
    /// Rebase, keeping local merge commits (`merges`).
    RebaseMerges,
    /// Rebase interactively (`interactive`).
    Interactive,
}

impl PullRebase {
    fn parse(value: Option<&[u8]>) -> Option<Self> {
        match value {
            Some(b"merges" | b"m") => Some(Self::RebaseMerges),
            Some(b"interactive" | b"i") => Some(Self::Interactive),
            Some(value) => parse_bool(value).map(Self::from_bool),
            None => Some(Self::Rebase),
        }
    }

    const fn from_bool(rebase: bool) -> Self {
        if rebase { Self::Rebase } else { Self::Merge }
    }
}

/// What `pull.ff` allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullFastForward {
    /// Fast-forward when possible, merge otherwise (`true`).
    Allowed,
    /// Always create a merge commit (`false`).
    Never,
    /// Only fast-forward; refuse otherwise (`only`).
    Only,
}

impl PullFastForward {
    fn parse(value: Option<&[u8]>) -> Option<Self> {
        match value {
            Some(b"only") => Some(Self::Only),
            Some(value) => parse_bool(value).map(|ff| if ff { Self::Allowed } else { Self::Never }),
            None => Some(Self::Allowed),
        }
    }
}

/// How the profile signs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signing<'a> {
    /// The format (`gpg.format`).
    pub format: Setting<'a, SigningFormat>,
    /// The key (`user.signingKey`): an OpenPGP key id or email, an SSH key's path or literal,
    /// or an X.509 certificate's id. Unset, gpg picks by the committer's email.
    pub key: Option<ConfigEntry<'a>>,
    /// Whether commits are signed by default (`commit.gpgSign`).
    pub commits: Setting<'a, bool>,
    /// Whether annotated tags are signed by default (`tag.gpgSign`).
    pub tags: Setting<'a, bool>,
    /// The program that signs, when not the default for the format.
    pub program: Option<ConfigEntry<'a>>,
    /// The file of trusted SSH signers that verifying SSH signatures needs
    /// (`gpg.ssh.allowedSignersFile`).
    pub allowed_signers: Option<ConfigEntry<'a>>,
}

impl<'a> Profile<'a> {
    /// The name commits are made with (`user.name`).
    #[must_use]
    pub fn name(self) -> Option<ConfigEntry<'a>> {
        self.snapshot.get("user.name")
    }

    /// The email commits are made with (`user.email`).
    #[must_use]
    pub fn email(self) -> Option<ConfigEntry<'a>> {
        self.snapshot.get("user.email")
    }

    /// Settings that override the name or email for authorship or committing only
    /// (`author.name`, `author.email`, `committer.name`, `committer.email`).
    pub fn identity_overrides(self) -> impl Iterator<Item = ConfigEntry<'a>> + use<'a> {
        let snapshot = self.snapshot;
        [
            "author.name",
            "author.email",
            "committer.name",
            "committer.email",
        ]
        .into_iter()
        .filter_map(move |key| snapshot.get(key))
    }

    /// Whether git refuses to guess a missing identity from the system (`user.useConfigOnly`).
    #[must_use]
    pub fn use_config_only(self) -> Setting<'a, bool> {
        self.bool("user.useConfigOnly", false)
    }

    /// How commits and tags are signed.
    #[must_use]
    pub fn signing(self) -> Signing<'a> {
        let format = self
            .snapshot
            .get("gpg.format")
            .and_then(|entry| {
                SigningFormat::parse(entry.text()).map(|format| Setting::from(format, entry))
            })
            .unwrap_or_else(|| Setting::default(SigningFormat::OpenPgp));
        let program = match format.value {
            // `gpg.program` is the older name of `gpg.openpgp.program`: whichever git reads
            // last applies.
            SigningFormat::OpenPgp => self.snapshot.entries().rev().find(|entry| {
                entry.key == BStr::new("gpg.program")
                    || entry.key == BStr::new("gpg.openpgp.program")
            }),
            other => self.snapshot.get(other.program_key()),
        };
        Signing {
            format,
            key: self.snapshot.get("user.signingKey"),
            commits: self.bool("commit.gpgSign", false),
            tags: self.bool("tag.gpgSign", false),
            program,
            allowed_signers: self.snapshot.get("gpg.ssh.allowedSignersFile"),
        }
    }

    /// The credential helpers git asks for any URL, in order (`credential.helper`; an empty
    /// value clears the ones before it).
    #[must_use]
    pub fn credential_helpers(self) -> Vec<ConfigEntry<'a>> {
        let mut helpers = Vec::new();
        for entry in self.snapshot.get_all("credential.helper") {
            if entry.text().is_empty() {
                helpers.clear();
            } else {
                helpers.push(entry);
            }
        }
        helpers
    }

    /// Helpers configured for particular URLs (`credential.<url>.helper`).
    pub fn credential_helpers_for_urls(self) -> impl Iterator<Item = ConfigEntry<'a>> + use<'a> {
        self.snapshot.entries().filter(|entry| {
            entry.key.starts_with(b"credential.")
                && entry.key.ends_with(b".helper")
                && entry.key != BStr::new("credential.helper")
        })
    }

    /// What a pull does with local commits (`pull.rebase`). `None` when unset: git then asks
    /// when branches have diverged.
    #[must_use]
    pub fn pull_rebase(self) -> Option<Setting<'a, PullRebase>> {
        let entry = self.snapshot.get("pull.rebase")?;
        PullRebase::parse(entry.value.map(AsRef::as_ref)).map(|value| Setting::from(value, entry))
    }

    /// Whether a pull may fast-forward (`pull.ff`). `None` when unset.
    #[must_use]
    pub fn pull_fast_forward(self) -> Option<Setting<'a, PullFastForward>> {
        let entry = self.snapshot.get("pull.ff")?;
        PullFastForward::parse(entry.value.map(AsRef::as_ref))
            .map(|value| Setting::from(value, entry))
    }

    /// Whether pushing a branch without an upstream sets one up (`push.autoSetupRemote`).
    #[must_use]
    pub fn push_auto_setup_remote(self) -> Setting<'a, bool> {
        self.bool("push.autoSetupRemote", false)
    }

    /// What a push without arguments pushes (`push.default`; git's default is `simple`).
    #[must_use]
    pub fn push_default(self) -> Option<ConfigEntry<'a>> {
        self.snapshot.get("push.default")
    }

    /// The branch name new repositories start on (`init.defaultBranch`).
    #[must_use]
    pub fn default_branch(self) -> Option<ConfigEntry<'a>> {
        self.snapshot.get("init.defaultBranch")
    }

    fn bool(self, key: &str, default: bool) -> Setting<'a, bool> {
        self.snapshot
            .get(key)
            .and_then(|entry| entry.bool().map(|value| Setting::from(value, entry)))
            .unwrap_or_else(|| Setting::default(default))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_rebase_values() {
        assert_eq!(PullRebase::parse(Some(b"true")), Some(PullRebase::Rebase));
        assert_eq!(PullRebase::parse(Some(b"false")), Some(PullRebase::Merge));
        assert_eq!(
            PullRebase::parse(Some(b"merges")),
            Some(PullRebase::RebaseMerges)
        );
        assert_eq!(PullRebase::parse(Some(b"i")), Some(PullRebase::Interactive));
        assert_eq!(PullRebase::parse(None), Some(PullRebase::Rebase));
        assert_eq!(PullRebase::parse(Some(b"sideways")), None);
    }

    #[test]
    fn pull_fast_forward_values() {
        assert_eq!(
            PullFastForward::parse(Some(b"only")),
            Some(PullFastForward::Only)
        );
        assert_eq!(
            PullFastForward::parse(Some(b"no")),
            Some(PullFastForward::Never)
        );
        assert_eq!(
            PullFastForward::parse(Some(b"yes")),
            Some(PullFastForward::Allowed)
        );
        assert_eq!(PullFastForward::parse(None), Some(PullFastForward::Allowed));
        assert_eq!(PullFastForward::parse(Some(b"sometimes")), None);
    }

    #[test]
    fn signing_formats() {
        for format in [
            SigningFormat::OpenPgp,
            SigningFormat::X509,
            SigningFormat::Ssh,
        ] {
            assert_eq!(SigningFormat::parse(format.name().as_bytes()), Some(format));
        }
        assert_eq!(SigningFormat::parse(b"pgp"), None);
    }
}
