//! Requests to a forge's API: HTTPS through rustls on aws-lc-rs, certificates checked against
//! the system's trust store (`rustls-platform-verifier`), the environment's proxy (`HTTPS_PROXY`,
//! `NO_PROXY`; never for the loopback address), no redirects followed (a credential never
//! travels to another place), and answers of bounded size. One agent per host, so its
//! connections are reused.

use std::sync::Arc;
use std::time::Duration;

use ureq::Agent;
use ureq::http::HeaderValue;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use zeroize::Zeroizing;

use crate::error::ForgeError;
use crate::host::{ForgeHost, ForgeKind};

/// The version of GitHub's REST API egit speaks.
const GITHUB_API_VERSION: &str = "2022-11-28";

/// The largest answer read: a page of a hundred pull requests is a few hundred kilobytes.
const ANSWER_LIMIT: u64 = 32 * 1024 * 1024;

/// What the host said: its status, its body, and its rate limit's headers.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
    /// `x-ratelimit-remaining`.
    pub(crate) remaining: Option<u32>,
    /// `x-ratelimit-reset` (seconds since the Unix epoch).
    pub(crate) reset: Option<i64>,
    /// `retry-after` (seconds), for a secondary rate limit.
    pub(crate) retry_after: Option<i64>,
}

/// A host's API client.
#[derive(Clone, Debug)]
pub(crate) struct Http {
    host: ForgeHost,
    agent: Agent,
}

impl Http {
    /// The client for `host`.
    pub(crate) fn new(host: ForgeHost) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let mut config = Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(concat!("egit/", env!("CARGO_PKG_VERSION")))
            .tls_config(tls);
        if host.protocol() == "http" {
            // The loopback address, never through a proxy.
            config = config.proxy(None);
        }
        Self {
            host,
            agent: config.build().into(),
        }
    }

    /// The host it talks to.
    pub(crate) const fn host(&self) -> &ForgeHost {
        &self.host
    }

    /// Sends `body` (JSON; a GET without one) to `url`, one of the host's, with `token` as the
    /// bearer of its authority, and reads the answer.
    pub(crate) fn send(
        &self,
        url: &str,
        body: Option<&[u8]>,
        token: &str,
    ) -> Result<Answer, ForgeError> {
        if !self.host.is_safe_for_credentials() {
            return Err(ForgeError::Insecure {
                host: self.host.name().to_owned(),
            });
        }
        let authorization = Zeroizing::new(format!("Bearer {token}"));
        let mut authorization =
            HeaderValue::from_str(&authorization).map_err(|_| ForgeError::Refused {
                host: self.host.name().to_owned(),
            })?;
        authorization.set_sensitive(true);
        // GitHub's media type and API version; GitLab needs neither.
        let github = self.host.kind() == ForgeKind::GitHub;
        let accept = if github {
            "application/vnd.github+json"
        } else {
            "application/json"
        };
        let sent = if let Some(body) = body {
            let mut request = self
                .agent
                .post(url)
                .header("Authorization", authorization)
                .header("Accept", accept)
                .header("Content-Type", "application/json");
            if github {
                request = request.header("X-GitHub-Api-Version", GITHUB_API_VERSION);
            }
            request.send(body)
        } else {
            let mut request = self
                .agent
                .get(url)
                .header("Authorization", authorization)
                .header("Accept", accept);
            if github {
                request = request.header("X-GitHub-Api-Version", GITHUB_API_VERSION);
            }
            request.call()
        };
        let mut response = sent.map_err(|error| self.network(&error))?;
        let header = |names: [&str; 2]| {
            names.iter().find_map(|name| {
                response
                    .headers()
                    .get(*name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::trim)
                    .map(str::to_owned)
            })
        };
        // GitHub's names, then GitLab's.
        let remaining = header(["x-ratelimit-remaining", "ratelimit-remaining"])
            .and_then(|value| value.parse().ok());
        let reset =
            header(["x-ratelimit-reset", "ratelimit-reset"]).and_then(|value| value.parse().ok());
        let retry_after =
            header(["retry-after", "retry-after"]).and_then(|value| value.parse().ok());
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(ANSWER_LIMIT)
            .read_to_vec()
            .map_err(|error| self.network(&error))?;
        Ok(Answer {
            status,
            body,
            remaining,
            reset,
            retry_after,
        })
    }

    /// A failure to reach the host, in words.
    fn network(&self, error: &ureq::Error) -> ForgeError {
        let host = self.host.name().to_owned();
        let message = match error {
            ureq::Error::Timeout(_) => "it took too long to answer".to_owned(),
            ureq::Error::HostNotFound => "its name couldn't be resolved".to_owned(),
            ureq::Error::ConnectionFailed => "the connection failed".to_owned(),
            ureq::Error::BodyExceedsLimit(_) => {
                return ForgeError::Malformed {
                    host,
                    detail: "the answer is too large".to_owned(),
                };
            }
            other => other.to_string(),
        };
        ForgeError::Network { host, message }
    }
}
