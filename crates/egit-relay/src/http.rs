//! A browser's request: egit's web build served, or `/relay` upgraded to a WebSocket.
//!
//! One request a connection (`Connection: close`), read whole before it is answered: a request
//! line and headers of at most [`HEAD_LIMIT`], within [`HEAD_TIMEOUT`]. Before anything else,
//! the host it names must be the relay's: loopback names on any port (a tunnel's `ssh -L`
//! listens on its own), the address listened on, and the names given (a proxy's); a WebSocket
//! must also be opened from the relay's own page (`Origin`).

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use crate::Shared;

/// The longest request head read.
const HEAD_LIMIT: usize = 16 * 1024;

/// How long a browser has to send its request.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// What every answer says, besides its own: no other site frames it or reads it, it loads
/// nothing but its own files (its WebAssembly compiled), and it talks to the relay alone; and it
/// is isolated from other sites' pages (opener and embedder policies), which lets it share its
/// memory with its workers (`SharedArrayBuffer`): egit's page runs threaded.
pub(crate) const SECURITY: &str = "Content-Security-Policy: default-src 'self'; \
     script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; \
     base-uri 'none'; form-action 'none'\r\n\
     X-Content-Type-Options: nosniff\r\n\
     Referrer-Policy: no-referrer\r\n\
     Cross-Origin-Opener-Policy: same-origin\r\n\
     Cross-Origin-Embedder-Policy: require-corp\r\n\
     Cross-Origin-Resource-Policy: same-origin\r\n";

/// A request as read: what the relay needs of it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Request {
    method: String,
    path: String,
    host: Option<String>,
    origin: Option<String>,
    upgrade: Option<String>,
    connection: Option<String>,
    version: Option<String>,
    key: Option<String>,
    accept_encoding: Option<String>,
}

/// Answers the request on `stream`.
pub(crate) async fn handle(mut stream: TcpStream, peer: SocketAddr, shared: Arc<Shared>) {
    let head = match tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream)).await {
        Ok(Ok(head)) => head,
        Ok(Err(error)) => {
            tracing::debug!(%error, %peer, "a request couldn't be read");
            return;
        }
        Err(_) => return,
    };
    let Some(request) = parse(&head) else {
        let _sent = answer(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"Not a request.\n",
        )
        .await;
        return;
    };
    if !host_allowed(request.host.as_deref(), &shared) {
        tracing::warn!(%peer, host = ?request.host, "a request for another host");
        let _sent = answer(
            &mut stream,
            "421 Misdirected Request",
            "text/plain",
            b"This relay answers for its own address alone.\n",
        )
        .await;
        return;
    }
    let path = request.path.split(['?', '#']).next().unwrap_or_default();
    if path == "/relay" {
        upgrade(stream, peer, &request, shared).await;
        return;
    }
    if let Some(key) = path.strip_prefix(crate::delivery::PREFIX)
        && request.method == "GET"
    {
        deliver(stream, peer, key, &shared).await;
        return;
    }
    let sent = match request.method.as_str() {
        "GET" | "HEAD" => serve_file(&mut stream, &request, path, &shared).await,
        _ => {
            answer(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain",
                b"Only GET.\n",
            )
            .await
        }
    };
    if let Err(error) = sent {
        tracing::debug!(%error, %peer, "an answer couldn't be sent");
    }
}

/// Hands the connection to the download waiting at `key` (its request's thread answers it);
/// none there, says so.
async fn deliver(mut stream: TcpStream, peer: SocketAddr, key: &str, shared: &Shared) {
    let Some(delivery) = shared.deliveries.take(key) else {
        let _sent = answer(
            &mut stream,
            "404 Not Found",
            "text/plain",
            b"This download isn't here: it was taken, or its minute is over.\n",
        )
        .await;
        return;
    };
    // The delivery writes on a thread of its own, as its request reads.
    let taken = stream
        .into_std()
        .and_then(|stream| stream.set_nonblocking(false).map(|()| stream));
    match taken {
        // Gone, it was stopped meanwhile: the connection closes.
        Ok(stream) => drop(delivery.send(stream)),
        Err(error) => tracing::debug!(%error, %peer, "a download's connection couldn't be taken"),
    }
}

/// Reads up to the end of the request's head (a blank line); what follows is left.
async fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        // Never past the head: what a client sends after it isn't read here.
        let read = stream.peek(&mut buffer).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the request ended early",
            ));
        }
        let seen = buffer.get(..read).unwrap_or_default();
        let start = head.len();
        head.extend_from_slice(seen);
        if let Some(end) = find(&head, b"\r\n\r\n") {
            let through = end.saturating_add(4);
            let take = through.saturating_sub(start);
            head.truncate(through);
            discard(stream, take).await?;
            return Ok(head);
        }
        discard(stream, read).await?;
        if head.len() > HEAD_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the request's head is too long",
            ));
        }
    }
}

/// Reads `count` bytes already peeked at, and drops them.
async fn discard(stream: &mut TcpStream, count: usize) -> io::Result<()> {
    let mut left = count;
    let mut buffer = [0_u8; 1024];
    while left > 0 {
        let chunk = buffer.get_mut(..left.min(1024)).unwrap_or_default();
        let read = stream.read(chunk).await?;
        if read == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        left = left.saturating_sub(read);
    }
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The request in `head`, if it is one.
fn parse(head: &[u8]) -> Option<Request> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Request::new(&mut headers);
    if !parsed.parse(head).ok()?.is_complete() {
        return None;
    }
    let mut request = Request {
        method: parsed.method?.to_owned(),
        path: parsed.path?.to_owned(),
        ..Request::default()
    };
    for header in parsed.headers.iter() {
        let value = || Some(String::from_utf8_lossy(header.value).trim().to_owned());
        let name = header.name.to_ascii_lowercase();
        let field = match name.as_str() {
            "host" => &mut request.host,
            "origin" => &mut request.origin,
            "upgrade" => &mut request.upgrade,
            "connection" => &mut request.connection,
            "sec-websocket-version" => &mut request.version,
            "sec-websocket-key" => &mut request.key,
            "accept-encoding" => &mut request.accept_encoding,
            _ => continue,
        };
        *field = value();
    }
    Some(request)
}

/// Whether `host` (a `Host` header: a name or address, and perhaps a port) is the relay's.
fn host_allowed(host: Option<&str>, shared: &Shared) -> bool {
    let Some(host) = host else {
        return false;
    };
    let name = host_name_of(host);
    let loopback = name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let listened = shared.address.ip();
    let ours = name
        .parse::<IpAddr>()
        .is_ok_and(|address| address == listened || listened.is_unspecified());
    let given = shared
        .hosts
        .iter()
        .any(|given| given.eq_ignore_ascii_case(host) || given.eq_ignore_ascii_case(name));
    loopback || ours || given
}

/// A `Host` header's name, without its port (an IPv6 address without its brackets).
fn host_name_of(host: &str) -> &str {
    if let Some(bracketed) = host.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or_default();
    }
    host.rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(host, |(name, _)| name)
}

/// Upgrades the connection to a WebSocket and runs a session over it, or says why not.
async fn upgrade(mut stream: TcpStream, peer: SocketAddr, request: &Request, shared: Arc<Shared>) {
    let has = |value: Option<&String>, word: &str| {
        value.is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(word))
        })
    };
    let refusal = if request.method != "GET"
        || !has(request.upgrade.as_ref(), "websocket")
        || !has(request.connection.as_ref(), "upgrade")
        || request.version.as_deref() != Some("13")
    {
        Some((
            "400 Bad Request",
            "A WebSocket, version 13, is opened here.\n",
        ))
    } else if !same_origin(request) {
        tracing::warn!(%peer, origin = ?request.origin, "a WebSocket from another site");
        Some((
            "403 Forbidden",
            "Only the relay's own page opens its WebSocket.\n",
        ))
    } else {
        None
    };
    let key = request.key.as_deref().unwrap_or_default();
    if let Some((status, why)) = refusal.or_else(|| {
        key.is_empty()
            .then_some(("400 Bad Request", "A WebSocket needs its key.\n"))
    }) {
        let _sent = answer(&mut stream, status, "text/plain", why.as_bytes()).await;
        return;
    }
    let accepted = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        derive_accept_key(key.as_bytes())
    );
    if stream.write_all(accepted.as_bytes()).await.is_err() {
        return;
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(egit_git::relay::protocol::MOST))
        .max_frame_size(Some(egit_git::relay::protocol::MOST));
    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, Some(config)).await;
    crate::session::run(socket, peer, shared).await;
}

/// Whether the WebSocket was opened from a page the relay served: its origin names the host
/// the request does.
fn same_origin(request: &Request) -> bool {
    let (Some(origin), Some(host)) = (&request.origin, &request.host) else {
        return false;
    };
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(|named| named.eq_ignore_ascii_case(host))
}

/// Serves a file of the web build: `/` is `index.html`; anything outside the folder isn't.
async fn serve_file(
    stream: &mut TcpStream,
    request: &Request,
    path: &str,
    shared: &Shared,
) -> io::Result<()> {
    let Some(web) = &shared.web else {
        return answer(
            stream,
            "404 Not Found",
            "text/html; charset=utf-8",
            NOT_BUILT,
        )
        .await;
    };
    let wanted = if path == "/" { "/index.html" } else { path };
    let Some(file) = within(web, wanted) else {
        return answer(stream, "404 Not Found", "text/plain", b"Not here.\n").await;
    };
    let accepts = |encoding: &str| {
        request.accept_encoding.as_deref().is_some_and(|accepted| {
            accepted
                .split(',')
                .any(|part| part.split(';').next().unwrap_or_default().trim() == encoding)
        })
    };
    // Compressed beside it, where the build made it so and the browser takes it.
    let mut chosen = None;
    for (encoding, extension) in [("br", "br"), ("gzip", "gz")] {
        let mut compressed = file.clone().into_os_string();
        compressed.push(".");
        compressed.push(extension);
        if accepts(encoding)
            && tokio::fs::metadata(&compressed)
                .await
                .is_ok_and(|meta| meta.is_file())
        {
            chosen = Some((PathBuf::from(compressed), Some(encoding)));
            break;
        }
    }
    let (read_from, encoding) = chosen.unwrap_or_else(|| (file.clone(), None));
    let Ok(contents) = tokio::fs::read(&read_from).await else {
        return answer(stream, "404 Not Found", "text/plain", b"Not here.\n").await;
    };
    let mut head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\n\
         Vary: Accept-Encoding\r\nConnection: close\r\n{SECURITY}",
        content_type(&file),
        contents.len()
    );
    if let Some(encoding) = encoding {
        head.push_str("Content-Encoding: ");
        head.push_str(encoding);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if request.method != "HEAD" {
        stream.write_all(&contents).await?;
    }
    stream.shutdown().await
}

/// `wanted` (a request's path) inside `web`, when it is a plain path to a file there: no `..`,
/// no hidden names, nothing a platform reads otherwise.
fn within(web: &Path, wanted: &str) -> Option<PathBuf> {
    let relative = Path::new(wanted.strip_prefix('/')?);
    let mut path = web.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        if name.starts_with('.') || name.contains(['\\', ':']) || name.contains('%') {
            return None;
        }
        path.push(name);
    }
    (path != web).then_some(path)
}

/// A file's type, by its extension.
fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "webmanifest") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}

/// What a browser sees where no web build is there.
const NOT_BUILT: &[u8] = b"<!doctype html><meta charset=utf-8><title>egit</title>\
<p>egit's web build isn't here. Build it with <code>scripts/build-web.sh</code>, then start the \
relay with <code>egit-relay --web target/web</code>.</p>\n";

/// A whole answer.
async fn answer(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n{SECURITY}\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_read_for_what_the_relay_needs() {
        let head =
            b"GET /relay HTTP/1.1\r\nHost: localhost:9000\r\nOrigin: http://localhost:9000\r\n\
            Upgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let request = parse(head).unwrap();
        assert_eq!(request.path, "/relay");
        assert_eq!(request.host.as_deref(), Some("localhost:9000"));
        assert!(same_origin(&request));
        assert!(parse(b"GET / HTTP/1.1\r\nHost: x").is_none());
        // Another site's page.
        let other = Request {
            origin: Some("https://evil.example".to_owned()),
            ..request
        };
        assert!(!same_origin(&other));
    }

    #[test]
    fn a_host_s_name_comes_without_its_port() {
        assert_eq!(host_name_of("localhost:7171"), "localhost");
        assert_eq!(host_name_of("[::1]:7171"), "::1");
        assert_eq!(host_name_of("devbox"), "devbox");
        assert_eq!(host_name_of("127.0.0.1"), "127.0.0.1");
    }

    #[test]
    fn only_plain_paths_inside_the_build_are_served() {
        let web = Path::new("/srv/web");
        assert_eq!(
            within(web, "/index.html"),
            Some(PathBuf::from("/srv/web/index.html"))
        );
        assert_eq!(
            within(web, "/assets/egit_bg.wasm"),
            Some(PathBuf::from("/srv/web/assets/egit_bg.wasm"))
        );
        for refused in [
            "/../etc/passwd",
            "/a/../../b",
            "/.git/config",
            "/a%2e%2e",
            "/",
            "x",
            "/C:x",
        ] {
            assert_eq!(within(web, refused), None, "{refused}");
        }
        assert_eq!(content_type(Path::new("egit_bg.wasm")), "application/wasm");
    }
}
