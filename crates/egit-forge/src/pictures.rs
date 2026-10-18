//! People's pictures, found by their email: GitHub's own for its no-reply addresses
//! (`12345+login@users.noreply.github.com`, the numbers its account's), else Gravatar's (the
//! address's SHA-256, asked with `d=404` so someone without a picture says so rather than being
//! drawn a default). No credential goes with these requests, so redirects are followed, to
//! HTTPS only; answers are a picture's size at most.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest as _, Sha256};
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::error::ForgeError;

/// The largest picture read: a 128-pixel one is a few kilobytes.
const PICTURE_LIMIT: u64 = 1024 * 1024;

/// Where pictures are asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureSources {
    /// GitHub's pictures by account number, ending in `/` (`…/u/`).
    pub github_ids: String,
    /// GitHub's profiles by login, ending in `/` (their `<login>.png` redirects to the picture).
    pub github_logins: String,
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
            gravatar: "https://www.gravatar.com/avatar/".to_owned(),
        }
    }

    /// Where `email`'s picture is, `size` pixels square: GitHub's for its no-reply addresses,
    /// else Gravatar's. The address is compared as Gravatar compares it, trimmed and in lower
    /// case.
    #[must_use]
    pub fn url(&self, email: &str, size: u32) -> String {
        let email = email.trim().to_lowercase();
        if let Some(local) = email.strip_suffix("@users.noreply.github.com") {
            // `12345+login`, or an older account's `login` alone.
            match local.split_once('+') {
                Some((id, _)) if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) => {
                    return format!("{}{id}?s={size}&v=4", self.github_ids);
                }
                None if !local.is_empty() => {
                    return format!("{}{local}.png?size={size}", self.github_logins);
                }
                _ => {}
            }
        }
        format!("{}{}?s={size}&d=404", self.gravatar, gravatar_hash(&email))
    }

    /// Whether every source is on the loopback address (tests), where plain HTTP is allowed.
    fn loopback(&self) -> bool {
        [&self.github_ids, &self.github_logins, &self.gravatar]
            .iter()
            .all(|source| is_loopback(source))
    }
}

/// The SHA-256 of an address already trimmed and in lower case, in hexadecimal, as Gravatar
/// names pictures.
fn gravatar_hash(email: &str) -> String {
    let digest = Sha256::digest(email.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Whether `url` is plain HTTP to the loopback address.
fn is_loopback(url: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|start| url.starts_with(start))
}

/// What asking for a picture found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// The picture's bytes, as the service sent them (PNG, JPEG, …).
    Found(Vec<u8>),
    /// The service has none for this address.
    Missing,
}

/// Fetches pictures from [`PictureSources`].
#[derive(Clone, Debug)]
pub struct Pictures {
    agent: Agent,
}

impl Pictures {
    /// A client for `sources`: HTTPS only, through the environment's proxy, unless every
    /// source is on the loopback address.
    #[must_use]
    pub fn new(sources: &PictureSources) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let loopback = sources.loopback();
        let mut config = Agent::config_builder()
            .http_status_as_error(false)
            .https_only(!loopback)
            .max_redirects(3)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("egit/", env!("CARGO_PKG_VERSION")))
            .tls_config(tls);
        if loopback {
            config = config.proxy(None);
        }
        Self {
            agent: config.build().into(),
        }
    }

    /// Asks `url` (from [`PictureSources::url`]) for a picture.
    ///
    /// # Errors
    ///
    /// [`ForgeError::Network`] when the service can't be reached or answers otherwise than with a
    /// picture or "none" (404), [`ForgeError::Malformed`] when the picture is too large.
    pub fn fetch(&self, url: &str) -> Result<Fetched, ForgeError> {
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
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "image/*")
            .call()
            .map_err(|error| network(error.to_string()))?;
        match response.status().as_u16() {
            200 => {}
            404 => return Ok(Fetched::Missing),
            status => return Err(network(format!("it answered {status}"))),
        }
        response
            .body_mut()
            .with_config()
            .limit(PICTURE_LIMIT)
            .read_to_vec()
            .map(Fetched::Found)
            .map_err(|error| match error {
                ureq::Error::BodyExceedsLimit(_) => ForgeError::Malformed {
                    host: host.clone(),
                    detail: "the picture is too large".to_owned(),
                },
                other => network(other.to_string()),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_github_s_for_its_no_reply_addresses_else_gravatar_s() {
        let sources = PictureSources::public();
        assert_eq!(
            sources.url(" Ada@Example.com ", 128),
            "https://www.gravatar.com/avatar/\
             b5fc85e55755f9e0d030a10ab4429b6b2944855f9a0d60077fe832becbc41d72?s=128&d=404"
        );
        assert_eq!(
            sources.url("12345+octocat@users.noreply.github.com", 128),
            "https://avatars.githubusercontent.com/u/12345?s=128&v=4"
        );
        assert_eq!(
            sources.url("octocat@users.noreply.github.com", 64),
            "https://github.com/octocat.png?size=64"
        );
        // Not an account's number: an ordinary address of that domain, for Gravatar.
        assert!(
            sources
                .url("x+y@users.noreply.github.com", 128)
                .starts_with("https://www.gravatar.com/avatar/")
        );
        assert!(!sources.loopback());
        let local = PictureSources {
            github_ids: "http://127.0.0.1:9/u/".to_owned(),
            github_logins: "http://127.0.0.1:9/".to_owned(),
            gravatar: "http://127.0.0.1:9/avatar/".to_owned(),
        };
        assert!(local.loopback());
    }
}
