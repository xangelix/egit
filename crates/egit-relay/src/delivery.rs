//! Downloads delivered to the browser: a request's successful answer that egit asked to be
//! delivered (an artifact, perhaps gigabytes) waits at an address of its own (`/delivery/<key>`:
//! random, taken once, for [`BROWSER_COMES`]) for the browser egit sends there, then its body
//! goes to it as the forge sends it, never through egit's connection, and the page holds none
//! of it.
//!
//! Its digest is checked as it goes, its last part held back until it is: one that doesn't
//! match is cut short, and the browser says the download failed. How much went is told to egit
//! as it goes ([`PROGRESS_EVERY`]).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, Read, Write as _};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use egit_git::relay::protocol::{Delivery, FromRelay, Missing};
use parking_lot::Mutex;
use sha2::{Digest as _, Sha256};

use crate::session::Out;

/// How long a delivery waits for the browser.
const BROWSER_COMES: Duration = Duration::from_secs(60);

/// How often a delivery looks whether it was stopped, while it waits.
const LOOKED_AT: Duration = Duration::from_millis(250);

/// How often egit is told how much went, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How long a browser may take a part before it counts as gone (a download it paused).
const BROWSER_TAKES: Duration = Duration::from_secs(300);

/// How much is read from the forge at once.
const CHUNK: usize = 256 * 1024;

/// What the address's path starts with.
pub(crate) const PREFIX: &str = "/delivery/";

/// The deliveries waiting for their browser, by key: where its connection goes.
#[derive(Default)]
pub(crate) struct Deliveries {
    waiting: Mutex<HashMap<String, Sender<TcpStream>>>,
}

impl Deliveries {
    /// A delivery's address, waiting from now: removed once taken, or as it is dropped.
    fn wait(self: &Arc<Self>) -> io::Result<Waiting> {
        let key = crate::Token::random()?.as_str().to_owned();
        let (sender, receiver) = mpsc::channel();
        self.waiting.lock().insert(key.clone(), sender);
        Ok(Waiting {
            key,
            receiver,
            deliveries: Arc::clone(self),
        })
    }

    /// The delivery waiting at `key`, where the browser's connection goes; taken once.
    pub(crate) fn take(&self, key: &str) -> Option<Sender<TcpStream>> {
        self.waiting.lock().remove(key)
    }
}

/// A delivery's address, waiting for its browser.
struct Waiting {
    key: String,
    receiver: Receiver<TcpStream>,
    deliveries: Arc<Deliveries>,
}

impl Waiting {
    /// The path the browser opens.
    fn path(&self) -> String {
        format!("{PREFIX}{}", self.key)
    }

    /// The browser's connection, once it came; why not, else.
    fn browser(&self, stopped: &dyn Fn() -> bool) -> Result<TcpStream, &'static str> {
        let since = Instant::now();
        loop {
            if stopped() {
                return Err("the download was stopped");
            }
            match self.receiver.recv_timeout(LOOKED_AT) {
                Ok(stream) => return Ok(stream),
                Err(RecvTimeoutError::Timeout) if since.elapsed() < BROWSER_COMES => {}
                Err(_) => {
                    return Err(
                        "the browser didn't take the download (a blocker of downloads \
                                or pop-ups may have stopped it)",
                    );
                }
            }
        }
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.deliveries.waiting.lock().remove(&self.key);
    }
}

/// Request `id`'s successful answer: its body (`length` bytes, where said) delivered to the
/// browser as `delivery` says. What to tell egit at its end.
pub(crate) fn deliver(
    id: u32,
    (delivery, deliveries): (&Delivery, &Arc<Deliveries>),
    (body, length): (&mut dyn Read, Option<u64>),
    stopped: &dyn Fn() -> bool,
    out: &Out,
) -> FromRelay {
    let failed = |message: &str| FromRelay::Failed {
        id,
        missing: Missing::Nothing,
        message: message.to_owned(),
    };
    let waiting = match deliveries.wait() {
        Ok(waiting) => waiting,
        Err(error) => return failed(&format!("the download couldn't be offered: {error}")),
    };
    let delivering = FromRelay::Delivering {
        id,
        path: waiting.path(),
    };
    if out.send(delivering.encode()).is_err() {
        return failed("the browser went away");
    }
    let mut browser = match waiting.browser(stopped) {
        Ok(browser) => browser,
        Err(why) => return failed(why),
    };
    drop(waiting);
    match stream(id, delivery, (body, length), &mut browser, stopped, out) {
        Ok(()) => {
            let _closed = browser.shutdown(Shutdown::Write);
            FromRelay::Exited {
                id,
                exit: egit_git::Exit::new(Some(0), None),
            }
        }
        Err(why) => {
            // Cut short: the browser says the download failed.
            let _closed = browser.shutdown(Shutdown::Both);
            failed(&why)
        }
    }
}

/// Sends the answer's head to `browser`, then its body as it comes, the last part only once
/// its digest matched (and as long as said).
fn stream(
    id: u32,
    delivery: &Delivery,
    (body, length): (&mut dyn Read, Option<u64>),
    browser: &mut TcpStream,
    stopped: &dyn Fn() -> bool,
    out: &Out,
) -> Result<(), String> {
    let gone = |error: &io::Error| format!("the browser stopped taking the download: {error}");
    browser
        .set_write_timeout(Some(BROWSER_TAKES))
        .map_err(|error| gone(&error))?;
    let framing = length.map_or_else(
        || "Transfer-Encoding: chunked\r\n".to_owned(),
        |length| format!("Content-Length: {length}\r\n"),
    );
    let answer = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Disposition: {}\r\n{framing}Cache-Control: no-store\r\nConnection: close\r\n{}\r\n",
        disposition(&delivery.name),
        crate::http::SECURITY,
    );
    browser
        .write_all(answer.as_bytes())
        .map_err(|error| gone(&error))?;
    let chunked = length.is_none();
    let mut hasher = delivery.sha256.map(|_| Sha256::new());
    let (mut buffer, mut held) = (vec![0; CHUNK], Vec::with_capacity(CHUNK));
    let (mut went, mut told, mut told_at) = (0_u64, 0_u64, Instant::now());
    loop {
        if stopped() {
            return Err("the download was stopped".to_owned());
        }
        let read = match body.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(egit_forge::here::unread(&error).0),
        };
        if read == 0 {
            break;
        }
        let part = buffer.get(..read).unwrap_or_default();
        if let Some(hasher) = &mut hasher {
            hasher.update(part);
        }
        // The part before goes; this one waits for the next, or for the digest.
        send(browser, &held, chunked).map_err(|error| gone(&error))?;
        went = went.saturating_add(u64::try_from(held.len()).unwrap_or(u64::MAX));
        held.clear();
        held.extend_from_slice(part);
        if went > told && told_at.elapsed() >= PROGRESS_EVERY {
            let _sent = out.send(FromRelay::Delivered { id, bytes: went }.encode());
            (told, told_at) = (went, Instant::now());
        }
    }
    let whole = went.saturating_add(u64::try_from(held.len()).unwrap_or(u64::MAX));
    if length.is_some_and(|length| length != whole) {
        return Err(format!(
            "the forge sent {whole} bytes of the {} it said",
            length.unwrap_or_default()
        ));
    }
    if let (Some(hasher), Some(expected)) = (hasher, delivery.sha256)
        && hasher.finalize().as_slice() != expected.as_slice()
    {
        return Err(
            "it didn't match its SHA-256 digest: the browser was told the download \
                    failed"
                .to_owned(),
        );
    }
    send(browser, &held, chunked).map_err(|error| gone(&error))?;
    if chunked {
        browser
            .write_all(b"0\r\n\r\n")
            .map_err(|error| gone(&error))?;
    }
    browser.flush().map_err(|error| gone(&error))?;
    let _sent = out.send(FromRelay::Delivered { id, bytes: whole }.encode());
    Ok(())
}

/// Writes `part` to the browser, framed as a chunk where the length wasn't said.
fn send(browser: &mut TcpStream, part: &[u8], chunked: bool) -> io::Result<()> {
    if part.is_empty() {
        return Ok(());
    }
    if chunked {
        browser.write_all(format!("{:x}\r\n", part.len()).as_bytes())?;
        browser.write_all(part)?;
        browser.write_all(b"\r\n")
    } else {
        browser.write_all(part)
    }
}

/// The `Content-Disposition` saving the download as `name`: plainly for browsers that read
/// only `filename` (what isn't plain ASCII made `_`), and whole in UTF-8 (RFC 6266).
fn disposition(name: &str) -> String {
    // What RFC 5987 lets stand in a value as it is, besides letters and digits.
    const SAFE: &[u8] = b"!#$&+-.^_`|~";
    // Only a file's name: never a folder's part, nor what a header can't hold.
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = if name.is_empty() || name == "." || name == ".." {
        "download"
    } else {
        name
    };
    let plain: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && !matches!(character, '"' | '\\' | '%') {
                character
            } else if character == ' ' {
                ' '
            } else {
                '_'
            }
        })
        .collect();
    let encoded = name.bytes().fold(String::new(), |mut encoded, byte| {
        if byte.is_ascii_alphanumeric() || SAFE.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
        encoded
    });
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_named_plainly_and_whole() {
        assert_eq!(
            disposition("artifacts.zip"),
            "attachment; filename=\"artifacts.zip\"; filename*=UTF-8''artifacts.zip"
        );
        assert_eq!(
            disposition("rapport d'été \"final\".zip"),
            "attachment; filename=\"rapport d'_t_ _final_.zip\"; \
             filename*=UTF-8''rapport%20d%27%C3%A9t%C3%A9%20%22final%22.zip"
        );
        // A path's folders, and what names nothing, are never taken.
        assert_eq!(
            disposition("../../.bashrc"),
            "attachment; filename=\".bashrc\"; filename*=UTF-8''.bashrc"
        );
        assert_eq!(
            disposition(".."),
            "attachment; filename=\"download\"; filename*=UTF-8''download"
        );
        // A line break never ends the header.
        assert!(!disposition("a\r\nSet-Cookie: x").contains('\n'));
    }
}
