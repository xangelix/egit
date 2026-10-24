//! Requests to a forge's API, made here or by a relay.
//!
//! Here (`here`: HTTPS through rustls on aws-lc-rs, the system's trust store, the environment's
//! proxy), or, where git runs through a relay (egit in a browser), by the relay, which makes
//! them the same way. No redirects followed (a credential never travels to another place), and
//! answers of bounded size.

#[cfg(not(target_arch = "wasm32"))]
use std::io::Read as _;

use egit_git::relay::Fetching;
use egit_git::relay::protocol::{Fetch, FetchMode, Method};
use egit_git::{Git, Relay};
use zeroize::Zeroizing;

use crate::error::ForgeError;
use crate::host::{ForgeHost, ForgeKind};

/// The version of GitHub's REST API egit speaks.
const GITHUB_API_VERSION: &str = "2022-11-28";

/// The largest answer read: a page of a hundred pull requests is a few hundred kilobytes.
const ANSWER_LIMIT: u64 = 32 * 1024 * 1024;

/// How much of a refusal's body is kept: its reason.
const REASON_LIMIT: usize = 64 * 1024;

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

/// A host's API client: here, or through a relay.
#[derive(Clone, Debug)]
pub(crate) struct Http {
    host: ForgeHost,
    relay: Option<Relay>,
}

/// Makes `fetch` (to `url`, on `host`): by `relay` where given, else here; gives its answer's
/// head, its body still to read.
pub(crate) async fn exchange(
    relay: Option<&Relay>,
    fetch: Fetch,
    (host, url): (&str, &str),
) -> Result<Exchange, ForgeError> {
    let network = |message: String| {
        tracing::debug!(host, url, message, "the forge couldn't be reached");
        ForgeError::Network {
            host: host.to_owned(),
            message,
        }
    };
    match relay {
        Some(relay) => {
            let mut fetching = relay.fetch(fetch);
            let head = fetching.head().await.map_err(network)?;
            Ok(Exchange {
                host: host.to_owned(),
                status: head.status,
                headers: head.headers,
                body: Body::Relayed(fetching),
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        None => {
            let answered =
                crate::here::fetch_here(&fetch).map_err(|unreached| network(unreached.0))?;
            Ok(Exchange {
                host: host.to_owned(),
                status: answered.status,
                headers: answered.headers.clone(),
                body: Body::Here(Box::new(answered.into_body(u64::MAX))),
            })
        }
        #[cfg(target_arch = "wasm32")]
        None => Err(network(
            "a browser reaches it through its relay alone".to_owned(),
        )),
    }
}

/// An answer coming: its status and headers, its body read as it comes.
pub(crate) struct Exchange {
    host: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
}

enum Body {
    #[cfg(not(target_arch = "wasm32"))]
    Here(Box<dyn std::io::Read + Send>),
    Relayed(Fetching),
}

impl Exchange {
    /// Its status.
    pub(crate) const fn status(&self) -> u16 {
        self.status
    }

    /// The first of `names` (lower case) it has.
    pub(crate) fn header(&self, names: [&str; 2]) -> Option<String> {
        names.iter().find_map(|name| {
            self.headers
                .iter()
                .find(|(header, _)| header == name)
                .map(|(_, value)| value.clone())
        })
    }

    /// More of its body: how much, 0 at its end.
    pub(crate) async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, ForgeError> {
        let network = |message: String| ForgeError::Network {
            host: self.host.clone(),
            message,
        };
        match &mut self.body {
            #[cfg(not(target_arch = "wasm32"))]
            Body::Here(reader) => reader
                .read(buffer)
                .map_err(|error| network(crate::here::unread(&error).0)),
            Body::Relayed(fetching) => {
                let read = fetching
                    .read(buffer)
                    .await
                    .map_err(|error| network(error.to_string()))?;
                if read == 0 {
                    fetching.end().await.map_err(network)?;
                }
                Ok(read)
            }
        }
    }
}

impl Http {
    /// The client for `host`, its requests made where `git` reaches the network: here, or by
    /// its relay.
    pub(crate) fn new(host: ForgeHost, git: &Git) -> Self {
        Self {
            host,
            relay: git.host().relay().cloned(),
        }
    }

    /// The host it talks to.
    pub(crate) const fn host(&self) -> &ForgeHost {
        &self.host
    }

    /// Sends `body` (JSON; a GET without one) to `url`, one of the host's, with `token` as the
    /// bearer of its authority, and reads the answer.
    pub(crate) async fn send(
        &self,
        url: &str,
        body: Option<&[u8]>,
        token: &str,
    ) -> Result<Answer, ForgeError> {
        let started = web_time::Instant::now();
        let method = if body.is_some() { "POST" } else { "GET" };
        let fetch = self.request(url, body, token, (None, false))?;
        let mut exchange = self.exchange(fetch, url).await?;
        // GitHub's names, then GitLab's.
        let remaining = exchange
            .header(["x-ratelimit-remaining", "ratelimit-remaining"])
            .and_then(|value| value.parse().ok());
        let reset = exchange
            .header(["x-ratelimit-reset", "ratelimit-reset"])
            .and_then(|value| value.parse().ok());
        let retry_after = exchange
            .header(["retry-after", "retry-after"])
            .and_then(|value| value.parse().ok());
        let location = exchange.header(["location", "location"]);
        let status = exchange.status;
        let mut body = Vec::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = exchange.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(buffer.get(..read).unwrap_or_default());
            if u64::try_from(body.len()).unwrap_or(u64::MAX) > ANSWER_LIMIT {
                return Err(ForgeError::Malformed {
                    host: self.host.name().to_owned(),
                    detail: "the answer is too large".to_owned(),
                });
            }
        }
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
    pub(crate) async fn stream(
        &self,
        url: &str,
        token: &str,
        (from, limit): (Option<u64>, u64),
        sink: &mut Sink<'_>,
    ) -> Result<Streamed, ForgeError> {
        let started = web_time::Instant::now();
        let fetch = self.request(url, None, token, (from, false))?;
        let mut exchange = self.exchange(fetch, url).await?;
        let status = exchange.status;
        let content_range = exchange.header(["content-range", "content-range"]);
        let (mut given, mut said) = (0_u64, Vec::new());
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = exchange.read(&mut buffer).await?;
            let Some(part) = buffer.get(..read).filter(|part| !part.is_empty()) else {
                break;
            };
            if (200..300).contains(&status) {
                given = given.saturating_add(u64::try_from(part.len()).unwrap_or(u64::MAX));
                if given > limit {
                    return Err(ForgeError::Malformed {
                        host: self.host.name().to_owned(),
                        detail: "the answer is too large".to_owned(),
                    });
                }
                sink(status, content_range.as_deref(), part);
            } else if said.len() < REASON_LIMIT {
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
    /// signed: only over HTTPS, but to the loopback address). Without an overall time limit; a
    /// redirect is said (`location`), never followed.
    pub(crate) async fn download(
        &self,
        url: &str,
        token: Option<&str>,
        sink: &mut (dyn FnMut(&[u8]) -> bool + Send),
    ) -> Result<Streamed, ForgeError> {
        let started = web_time::Instant::now();
        let fetch = if let Some(token) = token {
            self.request(url, None, token, (None, true))?
        } else {
            let loopback = url.starts_with("http://127.0.0.1:") || url.starts_with("http://[::1]:");
            if !url.starts_with("https://") && !loopback {
                return Err(ForgeError::Insecure {
                    host: self.host.name().to_owned(),
                });
            }
            Fetch {
                method: Method::Get,
                url: url.to_owned(),
                headers: Vec::new(),
                body: None,
                mode: FetchMode::Download,
            }
        };
        let mut exchange = self.exchange(fetch, url).await?;
        let status = exchange.status;
        let location = exchange.header(["location", "location"]);
        let (mut given, mut said, mut stopped) = (0_u64, Vec::new(), false);
        let mut buffer = vec![0_u8; 256 * 1024];
        loop {
            let read = exchange.read(&mut buffer).await?;
            let Some(part) = buffer.get(..read).filter(|part| !part.is_empty()) else {
                break;
            };
            if (200..300).contains(&status) {
                given = given.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                if !sink(part) {
                    stopped = true;
                    break;
                }
            } else if said.len() < REASON_LIMIT {
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

    /// Makes `fetch` (to `url`), here or by the relay, and gives its answer's head.
    async fn exchange(&self, fetch: Fetch, url: &str) -> Result<Exchange, ForgeError> {
        exchange(self.relay.as_ref(), fetch, (self.host.name(), url)).await
    }

    /// A request to `url` with `token` (and `body`, JSON, as a POST; `from`, a `Range` of the
    /// bytes as stored; `download`, any kind of answer accepted, without an overall time
    /// limit).
    fn request(
        &self,
        url: &str,
        body: Option<&[u8]>,
        token: &str,
        (from, download): (Option<u64>, bool),
    ) -> Result<Fetch, ForgeError> {
        if !self.host.is_safe_for_credentials() {
            return Err(ForgeError::Insecure {
                host: self.host.name().to_owned(),
            });
        }
        // A line break would end the header there and start another.
        if token.contains(['\r', '\n']) {
            return Err(ForgeError::Refused {
                host: self.host.name().to_owned(),
            });
        }
        let mut authorization = Zeroizing::new(Vec::with_capacity(token.len().saturating_add(7)));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(token.as_bytes());
        // GitHub's media type and API version; GitLab needs neither.
        let github = self.host.kind() == ForgeKind::GitHub;
        let accept = if download {
            "*/*"
        } else if github {
            "application/vnd.github+json"
        } else {
            "application/json"
        };
        let mut headers = vec![
            ("Authorization".to_owned(), authorization),
            (
                "Accept".to_owned(),
                Zeroizing::new(accept.as_bytes().to_vec()),
            ),
        ];
        let plain = |value: &str| Zeroizing::new(value.as_bytes().to_vec());
        if github {
            headers.push(("X-GitHub-Api-Version".to_owned(), plain(GITHUB_API_VERSION)));
        }
        if body.is_some() {
            headers.push(("Content-Type".to_owned(), plain("application/json")));
        }
        if let Some(from) = from {
            // A range of the bytes as stored: compressed, they'd be another's.
            headers.push(("Range".to_owned(), plain(&format!("bytes={from}-"))));
            headers.push(("Accept-Encoding".to_owned(), plain("identity")));
        }
        Ok(Fetch {
            method: if body.is_some() {
                Method::Post
            } else {
                Method::Get
            },
            url: url.to_owned(),
            headers,
            body: body.map(<[u8]>::to_vec),
            mode: if download {
                FetchMode::Download
            } else {
                FetchMode::Api
            },
        })
    }
}
