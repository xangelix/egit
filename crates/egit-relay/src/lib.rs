//! egit's relay: egit in a browser runs every program it needs on the computer the relay runs on.
//!
//! A browser opens the relay's address: the relay serves egit's web build (a folder of its
//! files, [`Options::web`]) and upgrades `/relay` to a WebSocket, over which egit runs git (and
//! `sh`, for the files it reads and writes) here, as `egit_git::relay` describes. Each
//! connection is a session: its programs, each in a process group of its own, stopped with it;
//! and git's and ssh's questions, asked through an askpass server of its own and relayed to its
//! browser.
//!
//! Only a browser holding the relay's token is let in: it is in the address the relay prints,
//! after `#`, which a browser never sends in a request, and egit sends it as its first message.
//! Requests naming a host other than the relay's (a page of another site, through DNS
//! rebinding) and `WebSocket`s opened from another site's page are refused before that.
//!
//! A proxy may serve the relay at an address of its own, under a path of its own (code-server's
//! `/proxy/<port>/`, a reverse proxy's folder): given ([`Options::urls`]), its host is let in
//! and its address is printed too. The page asks for everything relative to its own address.

mod cli;
mod delivery;
mod http;
mod running;
mod session;

use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use zeroize::Zeroizing;

pub use cli::main;
pub use egit_git::askpass::{HELPER_NAME, helper_status, run_helper};

/// How a relay serves.
#[derive(Debug)]
pub struct Options {
    /// Where it listens: the loopback address unless told otherwise (reached through `ssh -L`,
    /// or a proxy of the user's).
    pub listen: SocketAddr,
    /// The folder of egit's web build (`index.html`, its script, its WebAssembly): none, and
    /// a page says how to make one.
    pub web: Option<PathBuf>,
    /// What a browser must say first.
    pub token: Token,
    /// Host names a browser may name besides the relay's own addresses (a proxy's), each
    /// with its port or without.
    pub hosts: Vec<String>,
    /// Addresses a proxy gives browsers for the relay (code-server's
    /// `https://…/proxy/{{port}}/`), `{{port}}` standing for the port listened on: each one's
    /// host let in, as [`hosts`](Self::hosts) are, and each printed (see [`is_proxy_address`]).
    pub urls: Vec<String>,
}

/// Whether `template` is an address a proxy may give browsers for the relay.
///
/// That is `http://` or `https://`, a host (and port), and perhaps a path, with no user, query,
/// or fragment; `{{port}}` anywhere in it stands for the port listened on.
#[must_use]
pub fn is_proxy_address(template: &str) -> bool {
    proxied(template, 1).is_some()
}

/// `template` with `{{port}}` as `port`: the address, ending with `/` (the page is that folder's
/// `index.html`), and its host as a `Host` header names it.
fn proxied(template: &str, port: u16) -> Option<(String, String)> {
    let mut address = template.replace("{{port}}", &port.to_string());
    let rest = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))?;
    let (host, _path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_owned();
    if host.is_empty()
        || host.contains('@')
        || rest.contains(['?', '#'])
        || rest
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    if !address.ends_with('/') {
        address.push('/');
    }
    Some((address, host))
}

/// The secret a browser proves it was given the relay's address with.
pub struct Token(Zeroizing<String>);

impl Token {
    /// A new one: 32 random bytes, in hex.
    ///
    /// # Errors
    ///
    /// When the operating system gives no randomness.
    pub fn random() -> io::Result<Self> {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        getrandom::fill(bytes.as_mut()).map_err(io::Error::other)?;
        let mut hex = Zeroizing::new(String::with_capacity(64));
        for byte in bytes.iter() {
            for nibble in [byte >> 4, byte & 0x0f] {
                hex.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
            }
        }
        Ok(Self(hex))
    }

    /// One given (`EGIT_RELAY_TOKEN`, so the address stays the same between runs).
    #[must_use]
    pub fn given(token: &str) -> Self {
        Self(Zeroizing::new(token.to_owned()))
    }

    /// As the address carries it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `given` is it, in a time that doesn't tell how much of it matched.
    fn matches(&self, given: &[u8]) -> bool {
        let token = self.0.as_bytes();
        let differs = token
            .iter()
            .zip(given)
            .fold(0_u8, |differs, (a, b)| differs | (a ^ b));
        differs == 0 && token.len() == given.len()
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<secret>)")
    }
}

/// What every connection shares.
struct Shared {
    token: Token,
    web: Option<PathBuf>,
    hosts: Vec<String>,
    /// The address listened on.
    address: SocketAddr,
    /// The relay's own executable, which answers as the askpass helper.
    executable: PathBuf,
    /// The home folder here.
    home: PathBuf,
    /// This computer's name.
    host: String,
    /// What every program gets besides a session's own (ssh's connections shared).
    environment: Vec<(OsString, OsString)>,
    /// ssh's connections shared while the relay runs, so a key is asked for once.
    _ssh: Option<egit_git::SshSharing>,
    /// Downloads waiting for their browser.
    deliveries: Arc<delivery::Deliveries>,
}

/// A relay listening, not yet serving.
pub struct Bound {
    listener: TcpListener,
    shared: Arc<Shared>,
    /// The addresses proxies give browsers, each ending with `/`.
    proxied: Vec<String>,
}

/// Listens as `options` say.
///
/// # Errors
///
/// When the address can't be listened on, the relay can't find itself, or a proxy's address
/// isn't one ([`is_proxy_address`]).
pub async fn bind(options: Options) -> io::Result<Bound> {
    let listener = TcpListener::bind(options.listen).await?;
    let address = listener.local_addr()?;
    let mut hosts = options.hosts;
    let mut urls = Vec::with_capacity(options.urls.len());
    for template in &options.urls {
        let (url, host) = proxied(template, address.port()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{template} isn't an address a browser opens (http:// or https://)"),
            )
        })?;
        hosts.push(host);
        urls.push(url);
    }
    let executable = std::env::current_exe()?;
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let ssh = egit_git::SshSharing::start();
    let environment = ssh
        .as_ref()
        .map_or_default(egit_git::SshSharing::environment);
    Ok(Bound {
        listener,
        shared: Arc::new(Shared {
            token: options.token,
            web: options.web,
            hosts,
            address,
            executable,
            home,
            host: host_name(),
            environment,
            _ssh: ssh,
            deliveries: Arc::default(),
        }),
        proxied: urls,
    })
}

impl Bound {
    /// Where it listens.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.shared.address
    }

    /// The address a browser opens, the token after `#`.
    #[must_use]
    pub fn url(&self) -> String {
        let address = self.shared.address;
        let host = if address.ip().is_unspecified() {
            self.shared.host.clone()
        } else {
            address.ip().to_string()
        };
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        format!(
            "http://{host}:{}/#{}",
            address.port(),
            self.shared.token.as_str()
        )
    }

    /// The addresses proxies give browsers for it (as [`Options::urls`] named them), each with
    /// the token after `#`.
    #[must_use]
    pub fn proxied_urls(&self) -> Vec<String> {
        self.proxied
            .iter()
            .map(|url| format!("{url}#{}", self.shared.token.as_str()))
            .collect()
    }

    /// Serves browsers until the process ends.
    pub async fn serve(self) {
        loop {
            let (stream, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                // Out of file descriptors, say: another may go through a little later.
                Err(error) => {
                    tracing::warn!(%error, "a connection couldn't be accepted");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let shared = Arc::clone(&self.shared);
            tokio::spawn(async move {
                http::handle(stream, peer, shared).await;
            });
        }
    }
}

/// This computer's name.
fn host_name() -> String {
    #[cfg(unix)]
    {
        rustix::system::uname()
            .nodename()
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_matches_itself_alone() {
        let token = Token::random().unwrap();
        assert_eq!(token.as_str().len(), 64);
        assert!(token.matches(token.as_str().as_bytes()));
        assert!(!token.matches(b""));
        assert!(!token.matches(&token.as_str().as_bytes()[..63]));
        let mut other = token.as_str().as_bytes().to_vec();
        other[10] ^= 1;
        assert!(!token.matches(&other));
        assert_ne!(Token::random().unwrap().as_str(), token.as_str());
        assert!(!format!("{token:?}").contains(token.as_str()));
    }

    #[test]
    fn a_proxy_s_address_is_the_page_s_folder_and_names_its_host() {
        let code_server = "https://code.example/@me/ws.main/apps/code-server/proxy/{{port}}/";
        assert_eq!(
            proxied(code_server, 7171),
            Some((
                "https://code.example/@me/ws.main/apps/code-server/proxy/7171/".to_owned(),
                "code.example".to_owned()
            ))
        );
        // A port in the host's name (a subdomain's proxy), and a folder without its last slash.
        assert_eq!(
            proxied("http://{{port}}--ws.example:8080/at", 9000),
            Some((
                "http://9000--ws.example:8080/at/".to_owned(),
                "9000--ws.example:8080".to_owned()
            ))
        );
        assert_eq!(
            proxied("https://code.example", 1),
            Some((
                "https://code.example/".to_owned(),
                "code.example".to_owned()
            ))
        );
        for refused in [
            "code.example/proxy/1/",
            "ftp://code.example/",
            "https:///proxy/",
            "https://user@code.example/",
            "https://code.example/?a=b",
            "https://code.example/#token",
            "https://code example/",
            "https://code.example/a\nb",
        ] {
            assert!(!is_proxy_address(refused), "{refused}");
        }
    }
}
