//! One browser's connection: the token it proves itself with, the programs it runs, and the
//! questions their helpers ask.
//!
//! Messages go out through one writer task, in the order they were sent; what comes in is
//! handled as it comes, never waiting (programs run on threads of their own). The session ends
//! with its connection: every program it started is stopped, and every question refused.

use std::collections::HashMap;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use egit_git::askpass::{AskpassServer, Prompt};
use egit_git::relay::protocol::{FromRelay, ToRelay, VERSION};
use futures_util::{SinkExt as _, StreamExt as _};
use parking_lot::Mutex;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::Shared;
use crate::running::Running;

/// How long a browser has to say who it is.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How often questions shown are looked at, for an asker gone.
const ASKERS_LOOKED_AT: Duration = Duration::from_millis(250);

/// Where messages to the browser go.
pub(crate) type Out = UnboundedSender<Vec<u8>>;

/// Runs a session over `socket` until it closes.
pub(crate) async fn run(socket: WebSocketStream<TcpStream>, peer: SocketAddr, shared: Arc<Shared>) {
    let (mut sink, mut incoming) = socket.split();
    let (out, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if sink.send(Message::Binary(message.into())).await.is_err() {
                break;
            }
        }
        let _closed = sink.close().await;
    });

    let hello = tokio::time::timeout(HELLO_TIMEOUT, next_message(&mut incoming)).await;
    let refusal = match hello {
        Ok(Some(ToRelay::Hello { version, token })) => {
            if !shared.token.matches(&token) {
                tracing::warn!(%peer, "a browser without the relay's token");
                Some("That isn't this relay's token: open the address it printed.".to_owned())
            } else if version != VERSION {
                Some(format!(
                    "This relay speaks version {VERSION} of egit's protocol, and this egit {version}: \
                     they come from different builds."
                ))
            } else {
                None
            }
        }
        _ => Some("A browser says who it is first.".to_owned()),
    };
    if let Some(why) = refusal {
        let _sent = out.send(FromRelay::Refused { why }.encode());
        drop(out);
        let _written = writer.await;
        return;
    }
    tracing::info!(%peer, "a browser connected");
    let _sent = out.send(
        FromRelay::Welcome {
            version: VERSION,
            home: <Vec<u8> as bstr::ByteVec>::from_os_str_lossy(shared.home.as_os_str())
                .into_owned(),
            host: shared.host.clone(),
        }
        .encode(),
    );

    let session = Session::start(out.clone(), &shared);
    while let Some(message) = next_message(&mut incoming).await {
        session.take(message);
    }
    session.end();
    tracing::info!(%peer, "a browser went away");
    drop(out);
    let _written = writer.await;
}

/// The next message the browser sent, read as egit's; `None` once the connection ended (or
/// sent something that isn't one).
async fn next_message(
    incoming: &mut futures_util::stream::SplitStream<WebSocketStream<TcpStream>>,
) -> Option<ToRelay> {
    loop {
        match incoming.next().await? {
            Ok(Message::Binary(bytes)) => {
                return ToRelay::decode(&bytes)
                    .inspect_err(|garbled| tracing::warn!(%garbled, "a browser's message"))
                    .ok();
            }
            // Answered by the WebSocket itself.
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Ok(Message::Text(_) | Message::Close(_)) | Err(_) => return None,
        }
    }
}

/// What a session runs and asks.
struct Session {
    out: Out,
    shared: Arc<Shared>,
    /// The environment its programs get besides the relay's: its askpass server's.
    environment: Arc<[(OsString, OsString)]>,
    running: Arc<Mutex<HashMap<u32, Running>>>,
    asked: Arc<Mutex<HashMap<u32, Prompt>>>,
    /// Its askpass server, which ends with it.
    askpass: Option<AskpassServer>,
    ended: Arc<AtomicBool>,
}

impl Session {
    fn start(out: Out, shared: &Arc<Shared>) -> Self {
        let asked: Arc<Mutex<HashMap<u32, Prompt>>> = Arc::default();
        let ended = Arc::new(AtomicBool::new(false));
        let mut environment: Vec<(OsString, OsString)> = shared.environment.clone();
        let askpass = match AskpassServer::start(&shared.executable, || {}) {
            Ok((server, prompts)) => {
                environment.extend(server.environment());
                let (out, asked, ended) = (out.clone(), Arc::clone(&asked), Arc::clone(&ended));
                let relaying = std::thread::Builder::new()
                    .name("relaying questions".to_owned())
                    .spawn(move || relay_prompts(&prompts, &out, &asked, &ended));
                if let Err(error) = relaying {
                    tracing::warn!(%error, "git's questions can't be relayed");
                }
                Some(server)
            }
            Err(error) => {
                tracing::warn!(%error, "git's questions can't be relayed");
                None
            }
        };
        Self {
            out,
            shared: Arc::clone(shared),
            environment: environment.into(),
            running: Arc::default(),
            asked,
            askpass,
            ended,
        }
    }

    /// Takes in a message from the browser.
    fn take(&self, message: ToRelay) {
        match message {
            // Said already.
            ToRelay::Hello { .. } => {}
            ToRelay::Start { id, program } => {
                let finished = {
                    let running = Arc::clone(&self.running);
                    move |id| {
                        running.lock().remove(&id);
                    }
                };
                // Held while it starts: one that ends at once is forgotten after it was noted.
                let mut running = self.running.lock();
                let started = Running::start(
                    id,
                    program,
                    (&self.environment, &self.shared.home),
                    self.out.clone(),
                    finished,
                );
                running.insert(id, started);
            }
            ToRelay::Stdin { id, data } => {
                if let Some(running) = self.running.lock().get(&id) {
                    running.input(data);
                }
            }
            ToRelay::CloseStdin { id } => {
                if let Some(running) = self.running.lock().get_mut(&id) {
                    running.close_input();
                }
            }
            ToRelay::Terminate { id } => {
                if let Some(running) = self.running.lock().get(&id) {
                    running.terminate();
                }
            }
            ToRelay::Credit { id, bytes } => {
                if let Some(running) = self.running.lock().get(&id) {
                    running.credit(bytes);
                }
            }
            ToRelay::Fetch { id, fetch } => {
                let finished = {
                    let running = Arc::clone(&self.running);
                    move |id| {
                        running.lock().remove(&id);
                    }
                };
                // Held while it starts, as a program's is.
                let mut running = self.running.lock();
                let started = Running::fetch(
                    id,
                    fetch,
                    (self.out.clone(), Arc::clone(&self.shared.deliveries)),
                    finished,
                );
                running.insert(id, started);
            }
            ToRelay::Answer { id, answer } => {
                let prompt = self.asked.lock().remove(&id);
                if let Some(prompt) = prompt {
                    let answered = match answer {
                        Some(answer) => prompt.answer(&answer),
                        None => prompt.refuse(),
                    };
                    if let Err(error) = answered {
                        tracing::debug!(%error, "an answer couldn't be given");
                    }
                }
            }
        }
    }

    /// Ends the session: its programs stopped, its questions refused.
    fn end(mut self) {
        self.ended.store(true, Ordering::Release);
        let running = std::mem::take(&mut *self.running.lock());
        for (_, running) in running {
            running.abandon();
        }
        // Dropped, each is refused.
        self.asked.lock().clear();
        self.askpass = None;
    }
}

/// Relays the questions `prompts` brings to the browser, each by a number of its own, until
/// the session ends; tells it of those whose asker went away.
fn relay_prompts(
    prompts: &std::sync::mpsc::Receiver<Prompt>,
    out: &Out,
    asked: &Mutex<HashMap<u32, Prompt>>,
    ended: &AtomicBool,
) {
    let mut next = 0_u32;
    while !ended.load(Ordering::Acquire) {
        match prompts.recv_timeout(ASKERS_LOOKED_AT) {
            Ok(prompt) => {
                next = next.wrapping_add(1);
                let message = FromRelay::Prompt {
                    id: next,
                    kind: prompt.kind().code(),
                    context: prompt.context().map(str::to_owned),
                    text: prompt.text().to_owned(),
                };
                asked.lock().insert(next, prompt);
                if out.send(message.encode()).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let gone: Vec<u32> = asked
            .lock()
            .iter()
            .filter(|(_, prompt)| prompt.is_abandoned())
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            asked.lock().remove(&id);
            let _sent = out.send(FromRelay::Withdrawn { id }.encode());
        }
    }
}
