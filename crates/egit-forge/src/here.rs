//! Requests made on this computer, by egit for itself or by a relay for egit in a browser.
//!
//! HTTPS through rustls on aws-lc-rs, certificates checked against the system's trust store
//! (`rustls-platform-verifier`), the environment's proxy (`HTTPS_PROXY`, `NO_PROXY`; never for
//! the loopback address), and the redirects and time limits a request's [`FetchMode`] says: the
//! same wherever egit runs.

use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use egit_git::relay::protocol::{Fetch, FetchMode, Method};
use ureq::Agent;
use ureq::http::HeaderValue;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// The longest a download's body may take.
const DOWNLOAD_LASTS: Duration = Duration::from_hours(12);

/// What a host answered: its status and headers, its body still to read.
pub struct Answered {
    /// Its status.
    pub status: u16,
    /// Its headers, names in lower case, values that aren't text left out.
    pub headers: Vec<(String, String)>,
    body: ureq::Body,
}

impl Answered {
    /// The first value of header `name` (lower case).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// Its body, read as it comes (at most `limit` bytes: past it, reading fails).
    #[must_use]
    pub fn into_body(self, limit: u64) -> impl Read + Send + 'static {
        self.body.into_with_config().limit(limit).reader()
    }
}

/// Why a request couldn't be made, in words: what to show after the host's name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unreached(pub String);

/// Makes `fetch` here, and gives its answer's head.
///
/// # Errors
///
/// When the host can't be reached, or a header isn't one.
pub fn fetch_here(fetch: &Fetch) -> Result<Answered, Unreached> {
    let agent = agent(fetch.mode, is_loopback(&fetch.url));
    let mut request = match fetch.method {
        Method::Get => ureq::http::Request::get(&fetch.url),
        Method::Post => ureq::http::Request::post(&fetch.url),
    };
    for (name, value) in &fetch.headers {
        let mut value = HeaderValue::from_bytes(value)
            .map_err(|_| Unreached("a header isn't one".to_owned()))?;
        // An authorization never shows in a log of the request.
        value.set_sensitive(name.eq_ignore_ascii_case("authorization"));
        request = request.header(name.as_str(), value);
    }
    let sent = match &fetch.body {
        Some(body) => request
            .body(body.clone())
            .map_err(|error| Unreached(error.to_string()))
            .and_then(|request| agent.run(request).map_err(|error| unreached(&error))),
        None => request
            .body(())
            .map_err(|error| Unreached(error.to_string()))
            .and_then(|request| agent.run(request).map_err(|error| unreached(&error))),
    }?;
    let status = sent.status().as_u16();
    let headers = sent
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_str().to_owned(),
                value.to_str().ok()?.trim().to_owned(),
            ))
        })
        .collect();
    Ok(Answered {
        status,
        headers,
        body: sent.into_body(),
    })
}

/// A failure to reach a host, in words.
fn unreached(error: &ureq::Error) -> Unreached {
    Unreached(match error {
        ureq::Error::Timeout(_) => "it took too long to answer".to_owned(),
        ureq::Error::HostNotFound => "its name couldn't be resolved".to_owned(),
        ureq::Error::ConnectionFailed => "the connection failed".to_owned(),
        ureq::Error::BodyExceedsLimit(_) => "the answer is too large".to_owned(),
        other => other.to_string(),
    })
}

/// A failure reading an answer as it comes, in words.
#[must_use]
pub fn unread(error: &std::io::Error) -> Unreached {
    if error.kind() == std::io::ErrorKind::Other
        && let Some(inner) = error.get_ref()
        && let Some(error) = inner.downcast_ref::<ureq::Error>()
    {
        return unreached(error);
    }
    Unreached(error.to_string())
}

/// Whether `url` is on the loopback address (a test's fake forge): never through a proxy.
fn is_loopback(url: &str) -> bool {
    ["http://127.0.0.1:", "http://[::1]:", "http://localhost:"]
        .iter()
        .any(|loopback| url.starts_with(loopback))
}

/// Every way a request is made.
const MODES: [FetchMode; 3] = [FetchMode::Api, FetchMode::Download, FetchMode::Picture];

/// The agent for `mode` (one each, shared, so connections are reused), through the proxy or,
/// to the loopback address, not.
fn agent(mode: FetchMode, loopback: bool) -> Agent {
    static AGENTS: OnceLock<Vec<Agent>> = OnceLock::new();
    let agents = AGENTS.get_or_init(|| {
        MODES
            .iter()
            .flat_map(|&mode| [make(mode, false), make(mode, true)])
            .collect()
    });
    let index = MODES
        .iter()
        .position(|&made| made == mode)
        .unwrap_or_default()
        .saturating_mul(2)
        .saturating_add(usize::from(loopback));
    agents
        .get(index)
        .cloned()
        .unwrap_or_else(|| make(mode, loopback))
}

/// An agent made for `mode`.
fn make(mode: FetchMode, loopback: bool) -> Agent {
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .build();
    let mut config = Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .user_agent(concat!("egit/", env!("CARGO_PKG_VERSION")))
        .tls_config(tls);
    if loopback {
        config = config.proxy(None);
    }
    config = match mode {
        // A credential never travels to another place.
        FetchMode::Api => config
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(60))),
        FetchMode::Download => config
            .max_redirects(0)
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(DOWNLOAD_LASTS)),
        // No credential: a picture's redirects are followed, over HTTPS alone.
        FetchMode::Picture => config
            .https_only(!loopback)
            .max_redirects(3)
            .timeout_global(Some(Duration::from_secs(30))),
    };
    config.build().into()
}
