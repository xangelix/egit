//! People's pictures, found by their email, GitHub's first and Gravatar's as a fallback:
//!
//! - a GitHub no-reply address (`12345+login@users.noreply.github.com`) is its account's
//!   picture, by the account's number;
//! - any other address is looked up as GitHub shows commits' authors: its avatars service
//!   finds the account that verified the address, whether its profile shows the address or not
//!   (`u/e?email=`). An address of no account gets a placeholder (one picture for all of them,
//!   larger than asked for), which counts as none;
//! - then Gravatar's (the address's SHA-256, asked with `d=404`, so someone without one says so
//!   rather than being drawn a default).
//!
//! No credential goes with these requests, so redirects are followed, to HTTPS only; answers are
//! a picture's size at most. They are made here, or by egit in a browser's relay.

use std::fmt::Write as _;

use egit_git::Relay;
use egit_git::relay::protocol::{Fetch, FetchMode, Method};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::error::ForgeError;

/// The largest picture read: a 128-pixel one is a few kilobytes.
const PICTURE_LIMIT: u64 = 1024 * 1024;

/// The SHA-256 of GitHub's placeholder (its `ETag` too): what its lookup by email answers for an
/// address of no account.
const GITHUB_PLACEHOLDER: &str = "1506265dd79492985d428bf3ac115e873c30eedde97749ace57dff5a0020a0a2";

/// Where pictures are asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureSources {
    /// GitHub's pictures by account number, ending in `/` (`…/u/`).
    pub github_ids: String,
    /// GitHub's profiles by login, ending in `/` (their `<login>.png` redirects to the picture).
    pub github_logins: String,
    /// GitHub's pictures by an address its accounts verified, the address to follow
    /// (`…/u/e?email=`).
    pub github_emails: String,
    /// Gravatar's pictures by hash, ending in `/` (`…/avatar/`).
    pub gravatar: String,
}

impl PictureSources {
    /// The public services: GitHub's and Gravatar's.
    #[must_use]
    pub fn public() -> Self {
        Self {
            github_ids: "https://avatars.githubusercontent.com/u/".to_owned(),
            github_logins: "https://github.com/".to_owned(),
            github_emails: "https://avatars.githubusercontent.com/u/e?email=".to_owned(),
            gravatar: "https://www.gravatar.com/avatar/".to_owned(),
        }
    }

    /// Where `email`'s picture may be, `size` pixels square, in the order to ask: a GitHub
    /// no-reply address's account alone; any other address on GitHub, then on Gravatar. The
    /// address is compared as both compare it, trimmed and in lower case.
    #[must_use]
    pub fn urls(&self, email: &str, size: u32) -> Vec<Source> {
        let email = email.trim().to_lowercase();
        if let Some(local) = email.strip_suffix("@users.noreply.github.com") {
            // `12345+login`, or an older account's `login` alone.
            match local.split_once('+') {
                Some((id, _)) if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) => {
                    return vec![Source::Exact(format!(
                        "{}{id}?s={size}&v=4",
                        self.github_ids
                    ))];
                }
                None if !local.is_empty() => {
                    let url = format!("{}{local}.png?size={size}", self.github_logins);
                    return vec![Source::Exact(url)];
                }
                _ => {}
            }
        }
        vec![
            Source::GitHubByEmail(format!(
                "{}{}&s={size}",
                self.github_emails,
                query_escaped(&email)
            )),
            Source::Exact(format!(
                "{}{}?s={size}&d=404",
                self.gravatar,
                sha256_hex(email.as_bytes())
            )),
        ]
    }
}

/// Where a picture may be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// An address whose answer is the person's picture, or none (404).
    Exact(String),
    /// GitHub's lookup by email, which answers a placeholder for an address of no account.
    GitHubByEmail(String),
}

impl Source {
    /// Its address.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::Exact(url) | Self::GitHubByEmail(url) => url,
        }
    }
}

/// Whether `bytes`, answered by GitHub's lookup by email for a picture `size` pixels square, is
/// its placeholder: the one it is known to be, or a PNG wider than asked for (a person's picture
/// comes at the size asked, or smaller; the placeholder is 420 pixels whatever is asked).
fn is_github_placeholder(bytes: &[u8], size: u32) -> bool {
    if sha256_hex(bytes) == GITHUB_PLACEHOLDER {
        return true;
    }
    // A PNG's width: big-endian, after its signature and the IHDR chunk's length and name.
    let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    let width = bytes
        .get(16..20)
        .and_then(|width| <[u8; 4]>::try_from(width).ok())
        .map(u32::from_be_bytes);
    png && width.is_some_and(|width| width > size)
}

/// Escapes `text` for a URL's query: unreserved characters as they are, the rest as `%XX`.
fn query_escaped(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

/// `bytes`' SHA-256, in hexadecimal (how Gravatar names an address's picture).
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// What asking for a picture found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// The picture's bytes, as the service sent them (PNG, JPEG, …).
    Found(Vec<u8>),
    /// The service has none for this address.
    Missing,
}

/// Fetches pictures from [`PictureSources`]: here, or through a relay.
#[derive(Clone, Debug, Default)]
pub struct Pictures {
    relay: Option<Relay>,
}

impl Pictures {
    /// A client asking for pictures here, or (egit in a browser) through `relay`: HTTPS only,
    /// through the environment's proxy, but to the loopback address.
    #[must_use]
    pub const fn new(relay: Option<Relay>) -> Self {
        Self { relay }
    }

    /// `email`'s picture, `size` pixels square, from the first of `sources` that has one (see
    /// [`PictureSources::urls`]): GitHub's placeholder counts as none.
    ///
    /// # Errors
    ///
    /// The first failure, when no source had a picture and one couldn't be asked.
    pub async fn find(
        &self,
        sources: &PictureSources,
        email: &str,
        size: u32,
    ) -> Result<Fetched, ForgeError> {
        let mut failed = None;
        for source in sources.urls(email, size) {
            match self.fetch(source.url()).await {
                Ok(Fetched::Found(bytes)) => {
                    let placeholder = matches!(source, Source::GitHubByEmail(_))
                        && is_github_placeholder(&bytes, size);
                    if !placeholder {
                        return Ok(Fetched::Found(bytes));
                    }
                }
                Ok(Fetched::Missing) => {}
                Err(error) => {
                    failed.get_or_insert(error);
                }
            }
        }
        failed.map_or(Ok(Fetched::Missing), Err)
    }

    /// Asks `url` (one of [`PictureSources::urls`]) for a picture.
    ///
    /// # Errors
    ///
    /// [`ForgeError::Network`] when the service can't be reached or answers otherwise than with a
    /// picture or "none" (404), [`ForgeError::Malformed`] when the picture is too large.
    pub async fn fetch(&self, url: &str) -> Result<Fetched, ForgeError> {
        let host = url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or(url)
            .to_owned();
        let network = |message: String| ForgeError::Network {
            host: host.clone(),
            message,
        };
        let fetch = Fetch {
            method: Method::Get,
            url: url.to_owned(),
            headers: vec![("Accept".to_owned(), Zeroizing::new(b"image/*".to_vec()))],
            body: None,
            mode: FetchMode::Picture,
        };
        let mut exchange = crate::http::exchange(self.relay.as_ref(), fetch, (&host, url)).await?;
        match exchange.status() {
            200 => {}
            404 => return Ok(Fetched::Missing),
            status => return Err(network(format!("it answered {status}"))),
        }
        let mut picture = Vec::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = exchange.read(&mut buffer).await?;
            if read == 0 {
                return Ok(Fetched::Found(picture));
            }
            picture.extend_from_slice(buffer.get(..read).unwrap_or_default());
            if u64::try_from(picture.len()).unwrap_or(u64::MAX) > PICTURE_LIMIT {
                return Err(ForgeError::Malformed {
                    host,
                    detail: "the picture is too large".to_owned(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(email: &str, size: u32) -> Vec<String> {
        PictureSources::public()
            .urls(email, size)
            .iter()
            .map(|source| source.url().to_owned())
            .collect()
    }

    #[test]
    fn pictures_are_github_s_first_then_gravatar_s() {
        assert_eq!(
            urls(" Ada+git@Example.com ", 128),
            [
                "https://avatars.githubusercontent.com/u/e?email=ada%2Bgit%40example.com&s=128",
                "https://www.gravatar.com/avatar/\
                 23da3b025c68b63524adcc61c2857f18340c253969c7bf18835449e466a3aa3f?s=128&d=404",
            ]
        );
        assert_eq!(
            urls("12345+octocat@users.noreply.github.com", 128),
            ["https://avatars.githubusercontent.com/u/12345?s=128&v=4"]
        );
        assert_eq!(
            urls("octocat@users.noreply.github.com", 64),
            ["https://github.com/octocat.png?size=64"]
        );
        // Not an account's number: an ordinary address of that domain.
        assert_eq!(urls("x+y@users.noreply.github.com", 128).len(), 2);
    }

    #[test]
    fn github_s_placeholder_is_told_from_a_picture() {
        // A PNG's signature and IHDR: so wide, so tall.
        let png = |width: u32| {
            let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes
        };
        assert!(is_github_placeholder(&png(420), 128));
        assert!(!is_github_placeholder(&png(128), 128));
        assert!(
            !is_github_placeholder(&png(100), 128),
            "smaller: a person's"
        );
        assert!(!is_github_placeholder(b"\xff\xd8\xff\xe0 a JPEG", 128));
    }
}
