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
    /// `location`, where a redirect points (GitHub's to a log's file).
    pub(crate) location: Option<String>,
}

/// What takes a streamed answer's body a part at a time, with its status and `Content-Range`.
pub(crate) type Sink<'a> = dyn FnMut(u16, Option<&str>, &[u8]) + Send + 'a;

/// How a streamed answer went: its status, and what a refusal said (its body, 64 KiB at most).
#[derive(Debug)]
pub(crate) struct Streamed {
    pub(crate) status: u16,
    pub(crate) said: Vec<u8>,
    /// Where a redirect points.
    pub(crate) location: Option<String>,
    /// Whether the sink said to stop before the end.
    pub(crate) stopped: bool,
}

/// One of an answer's headers: the first of `names` it has.
fn header_of(response: &ureq::http::Response<ureq::Body>, names: [&str; 2]) -> Option<String> {
    names.iter().find_map(|name| {
        response
            .headers()
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .map(str::to_owned)
    })
}

/// A host's API client.
#[derive(Clone, Debug)]
pub(crate) struct Http {
    host: ForgeHost,
    agent: Agent,
    /// Downloads' (an artifact may be gigabytes): the same, but without an overall time limit;
    /// 60 s for an answer to begin, and its body as long as [`DOWNLOAD_LASTS`].
    downloads: Agent,
}

/// The longest a download's body may take.
const DOWNLOAD_LASTS: Duration = Duration::from_hours(12);

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
        let base = || {
            let config = Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_connect(Some(Duration::from_secs(10)))
                .user_agent(concat!("egit/", env!("CARGO_PKG_VERSION")))
                .tls_config(tls.clone());
            if host.protocol() == "http" {
                // The loopback address, never through a proxy.
                config.proxy(None)
            } else {
                config
            }
        };
        let agent = base().timeout_global(Some(Duration::from_secs(60))).build();
        let downloads = base()
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(DOWNLOAD_LASTS))
            .build();
        Self {
            host,
            agent: agent.into(),
            downloads: downloads.into(),
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
        let started = std::time::Instant::now();
        let method = if body.is_some() { "POST" } else { "GET" };
        let mut response = self.request(&self.agent, url, body, token, (None, false))?;
        let header = |names: [&str; 2]| header_of(&response, names);
        // GitHub's names, then GitLab's.
        let remaining = header(["x-ratelimit-remaining", "ratelimit-remaining"])
            .and_then(|value| value.parse().ok());
        let reset =
            header(["x-ratelimit-reset", "ratelimit-reset"]).and_then(|value| value.parse().ok());
        let retry_after =
            header(["retry-after", "retry-after"]).and_then(|value| value.parse().ok());
        let location = header(["location", "location"]);
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(ANSWER_LIMIT)
            .read_to_vec()
            .map_err(|error| self.network(&error))?;
        // What was asked and how it went; never the token.
        tracing::debug!(
            host = self.host.name(),
            method,
            url,
            status,
            bytes = body.len(),
            ms = started.elapsed().as_millis(),
            "the forge answered"
        );
        Ok(Answer {
            status,
            body,
            remaining,
            reset,
            retry_after,
            location,
        })
    }

    /// GETs `url` (a log, say) with `token`, handing a successful answer's body to `sink` a
    /// part at a time as it comes, with its status and `Content-Range` (never more than `limit`
    /// bytes, nor all of it at once); from byte `from` on (`Range`, the bytes as stored: nothing
    /// compressed) where given. A refusal's body (its reason) is kept instead.
    pub(crate) fn stream(
        &self,
        url: &str,
        token: &str,
        (from, limit): (Option<u64>, u64),
        sink: &mut Sink<'_>,
    ) -> Result<Streamed, ForgeError> {
        use std::io::Read as _;
        let started = std::time::Instant::now();
        let mut response = self.request(&self.agent, url, None, token, (from, false))?;
        let status = response.status().as_u16();
        let content_range = header_of(&response, ["content-range", "content-range"]);
        let reader = response.body_mut().with_config().limit(limit);
        let mut reader = reader.reader();
        let (mut given, mut said) = (0_u64, Vec::new());
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| self.network_io(&error))?;
            let Some(part) = buffer.get(..read).filter(|part| !part.is_empty()) else {
                break;
            };
            if (200..300).contains(&status) {
                sink(status, content_range.as_deref(), part);
                given = given.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            } else if said.len() < 64 * 1024 {
                said.extend_from_slice(part);
            }
        }
        tracing::debug!(
            host = self.host.name(),
            url,
            from,
            status,
            content_range,
            bytes = given,
            ms = started.elapsed().as_millis(),
            "the forge streamed"
        );
        Ok(Streamed {
            status,
            said,
            location: None,
            stopped: false,
        })
    }

    /// GETs `url` for a download (an artifact, a log's file), handing a successful answer's
    /// body to `sink` a part at a time until it says to stop (`false`): with `token` (the
    /// host's own address), or without any credential (a place a redirect named, its address
    /// signed: only over HTTPS, but to the loopback address). Through the agent without an
    /// overall time limit; a redirect is said (`location`), never followed.
    pub(crate) fn download(
        &self,
        url: &str,
        token: Option<&str>,
        sink: &mut (dyn FnMut(&[u8]) -> bool + Send),
    ) -> Result<Streamed, ForgeError> {
        use std::io::Read as _;
        let started = std::time::Instant::now();
        let mut response = if let Some(token) = token {
            self.request(&self.downloads, url, None, token, (None, true))?
        } else {
            let loopback = url.starts_with("http://127.0.0.1:") || url.starts_with("http://[::1]:");
            if !url.starts_with("https://") && !loopback {
                return Err(ForgeError::Insecure {
                    host: self.host.name().to_owned(),
                });
            }
            self.downloads
                .get(url)
                .call()
                .map_err(|error| self.network(&error))?
        };
        let status = response.status().as_u16();
        let location = header_of(&response, ["location", "location"]);
        let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
        let (mut given, mut said, mut stopped) = (0_u64, Vec::new(), false);
        let mut buffer = vec![0_u8; 256 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| self.network_io(&error))?;
            let Some(part) = buffer.get(..read).filter(|part| !part.is_empty()) else {
                break;
            };
            if (200..300).contains(&status) {
                given = given.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                if !sink(part) {
                    stopped = true;
                    break;
                }
            } else if said.len() < 64 * 1024 {
                said.extend_from_slice(part);
            }
        }
        // Not the address: a signed one carries its own authority.
        tracing::debug!(
            host = self.host.name(),
            credential = token.is_some(),
            status,
            bytes = given,
            stopped,
            ms = started.elapsed().as_millis(),
            "a download went"
        );
        Ok(Streamed {
            status,
            said,
            location,
            stopped,
        })
    }

    /// Sends a request to `url` through `agent` with `token` (and `body`, JSON, as a POST;
    /// `from`, a `Range` of the bytes as stored; `any`, any kind of answer accepted: a
    /// download's), and gives its answer's head, its body still to read.
    fn request(
        &self,
        agent: &Agent,
        url: &str,
        body: Option<&[u8]>,
        token: &str,
        (from, any): (Option<u64>, bool),
    ) -> Result<ureq::http::Response<ureq::Body>, ForgeError> {
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
        let accept = if any {
            "*/*"
        } else if github {
            "application/vnd.github+json"
        } else {
            "application/json"
        };
        let sent = if let Some(body) = body {
            let mut request = agent
                .post(url)
                .header("Authorization", authorization)
                .header("Accept", accept)
                .header("Content-Type", "application/json");
            if github {
                request = request.header("X-GitHub-Api-Version", GITHUB_API_VERSION);
            }
            request.send(body)
        } else {
            let mut request = agent
                .get(url)
                .header("Authorization", authorization)
                .header("Accept", accept);
            if github {
                request = request.header("X-GitHub-Api-Version", GITHUB_API_VERSION);
            }
            if let Some(from) = from {
                // A range of the bytes as stored: compressed, they'd be another's.
                request = request
                    .header("Range", format!("bytes={from}-"))
                    .header("Accept-Encoding", "identity");
            }
            request.call()
        };
        sent.map_err(|error| {
            tracing::debug!(host = self.host.name(), url, %error, "the forge couldn't be reached");
            self.network(&error)
        })
    }

    /// A failure reading an answer as it comes, in words.
    fn network_io(&self, error: &std::io::Error) -> ForgeError {
        if error.kind() == std::io::ErrorKind::Other
            && let Some(inner) = error.get_ref()
            && let Some(error) = inner.downcast_ref::<ureq::Error>()
        {
            return self.network(error);
        }
        ForgeError::Network {
            host: self.host.name().to_owned(),
            message: error.to_string(),
        }
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
