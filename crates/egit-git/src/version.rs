//! The version of the git binary egit drives, and the oldest one it supports.

use std::fmt;

/// A git release, as `git version` reports it.
///
/// Only the three numeric components are kept: platform suffixes (`.windows.1`, `(Apple Git-…)`)
/// and release-candidate markers carry nothing egit decides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    /// The major release (the `2` of 2.43.0).
    pub major: u16,
    /// The minor release (the `43` of 2.43.0).
    pub minor: u16,
    /// The maintenance release (the `0` of 2.43.0).
    pub patch: u16,
}

impl GitVersion {
    /// The oldest git egit supports: Ubuntu 24.04 LTS's, the oldest platform egit targets.
    pub const MINIMUM: Self = Self::new(2, 43, 0);

    /// A version from its components.
    #[must_use]
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses what `git version` prints, such as `git version 2.43.0` or
    /// `git version 2.47.1.windows.2`.
    ///
    /// # Errors
    ///
    /// Returns [`VersionParseError`] when the text doesn't start with `git version` followed by
    /// at least a major and a minor number.
    pub fn parse(output: &[u8]) -> Result<Self, VersionParseError> {
        let text = output.trim_ascii();
        let rest = text
            .strip_prefix(b"git version ")
            .ok_or(VersionParseError)?;
        let token = rest
            .split(u8::is_ascii_whitespace)
            .next()
            .ok_or(VersionParseError)?;
        let mut parts = token.split(|&byte| byte == b'.' || byte == b'-');
        let major = parts
            .next()
            .and_then(leading_number)
            .ok_or(VersionParseError)?;
        let minor = parts
            .next()
            .and_then(leading_number)
            .ok_or(VersionParseError)?;
        // A missing or non-numeric third component (`2.44.rc0`) reads as `.0`.
        let patch = parts.next().and_then(leading_number).unwrap_or(0);
        Ok(Self::new(major, minor, patch))
    }

    /// Whether egit supports this version.
    #[must_use]
    pub fn is_supported(self) -> bool {
        self >= Self::MINIMUM
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The text `git version` printed isn't a version egit can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not the output of `git version`")]
pub struct VersionParseError;

/// The number at the start of `part` (`rc0` has none; `0rc1` is `0`).
fn leading_number(part: &[u8]) -> Option<u16> {
    let digits = part.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let digits = part.get(..digits).filter(|digits| !digits.is_empty())?;
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_versions() {
        assert_eq!(
            GitVersion::parse(b"git version 2.43.0\n"),
            Ok(GitVersion::new(2, 43, 0))
        );
        assert_eq!(
            GitVersion::parse(b"git version 2.56.0"),
            Ok(GitVersion::new(2, 56, 0))
        );
    }

    #[test]
    fn ignores_platform_suffixes() {
        assert_eq!(
            GitVersion::parse(b"git version 2.47.1.windows.2"),
            Ok(GitVersion::new(2, 47, 1))
        );
        assert_eq!(
            GitVersion::parse(b"git version 2.39.5 (Apple Git-154)"),
            Ok(GitVersion::new(2, 39, 5))
        );
    }

    #[test]
    fn release_candidates_read_as_their_release() {
        assert_eq!(
            GitVersion::parse(b"git version 2.44.0-rc1"),
            Ok(GitVersion::new(2, 44, 0))
        );
        assert_eq!(
            GitVersion::parse(b"git version 2.44.rc0"),
            Ok(GitVersion::new(2, 44, 0))
        );
    }

    #[test]
    fn rejects_other_text() {
        assert_eq!(GitVersion::parse(b""), Err(VersionParseError));
        assert_eq!(
            GitVersion::parse(b"hub version 2.14.2"),
            Err(VersionParseError)
        );
        assert_eq!(GitVersion::parse(b"git version"), Err(VersionParseError));
        assert_eq!(GitVersion::parse(b"git version 2"), Err(VersionParseError));
        assert_eq!(
            GitVersion::parse(b"git version x.y.z"),
            Err(VersionParseError)
        );
        assert_eq!(
            GitVersion::parse(b"git version 99999.0.0"),
            Err(VersionParseError)
        );
    }

    #[test]
    fn the_floor_is_ubuntu_24_04s_git() {
        assert!(GitVersion::new(2, 43, 0).is_supported());
        assert!(GitVersion::new(2, 56, 0).is_supported());
        assert!(GitVersion::new(3, 0, 0).is_supported());
        assert!(!GitVersion::new(2, 42, 9).is_supported());
        assert!(!GitVersion::new(1, 99, 99).is_supported());
    }

    #[test]
    fn displays_as_dotted_numbers() {
        assert_eq!(GitVersion::new(2, 43, 0).to_string(), "2.43.0");
    }
}
