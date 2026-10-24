//! A program running through a relay: its standard output read as it comes, its standard error
//! handed on as it comes, and how it ended.

use std::collections::VecDeque;
use std::future::poll_fn;
use std::io;
use std::task::{Poll, Waker};

use parking_lot::Mutex;

use super::{Missing, Relay, ToRelay, WINDOW};
use crate::output::Exit;

/// Where a program's standard error goes as it comes (git's progress, a transcript, its tail).
pub(crate) type Stderr = Box<dyn FnMut(&[u8]) + Send>;

/// How much standard input goes in one message.
const INPUT_CHUNK: usize = 64 * 1024;

/// How much of its output is taken in before the relay is told: a quarter of what it may send
/// meanwhile, so it never waits while there is room.
const CREDIT_STEP: u32 = WINDOW >> 2;

/// Why a program didn't run to its end through the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotRun {
    /// It couldn't be started: what was missing, and the relay's words.
    Missing(Missing, String),
    /// The connection ended first, and why.
    Lost(String),
}

/// What the relay said of a process, shared between the connection and its reader.
pub(crate) struct Slot(Mutex<Inner>);

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.0.lock();
        f.debug_struct("Slot")
            .field("waiting", &inner.chunks.len())
            .field("end", &inner.end)
            .finish_non_exhaustive()
    }
}

struct Inner {
    /// A request's answer's head, once it came.
    head: Option<Head>,
    /// Output not yet read, in order; `offset` of the first already read.
    chunks: VecDeque<Vec<u8>>,
    offset: usize,
    end: Option<Result<Exit, NotRun>>,
    waker: Option<Waker>,
    stderr: Stderr,
    /// Nobody reads its output any more: what comes is dropped, the relay told to send more.
    abandoned: bool,
}

impl Slot {
    pub(crate) fn new(stderr: Stderr) -> Self {
        Self(Mutex::new(Inner {
            head: None,
            chunks: VecDeque::new(),
            offset: 0,
            end: None,
            waker: None,
            stderr,
            abandoned: false,
        }))
    }

    /// More output; how much the relay may send at once, where nobody reads it any more.
    pub(crate) fn stdout(&self, data: Vec<u8>) -> Option<u32> {
        let mut inner = self.0.lock();
        if inner.abandoned {
            return Some(u32::try_from(data.len()).unwrap_or(u32::MAX));
        }
        if !data.is_empty() {
            inner.chunks.push_back(data);
        }
        inner.wake();
        None
    }

    /// A request's answer's head.
    pub(crate) fn head(&self, head: Head) {
        let mut inner = self.0.lock();
        inner.head = Some(head);
        inner.wake();
    }

    /// More of its standard error, handed on at once.
    pub(crate) fn stderr(&self, data: &[u8]) {
        (self.0.lock().stderr)(data);
    }

    pub(crate) fn exited(&self, exit: Exit) {
        self.end(Ok(exit));
    }

    pub(crate) fn failed(&self, missing: Missing, message: String) {
        self.end(Err(NotRun::Missing(missing, message)));
    }

    pub(crate) fn lost(&self, why: &str) {
        self.end(Err(NotRun::Lost(why.to_owned())));
    }

    fn end(&self, end: Result<Exit, NotRun>) {
        let mut inner = self.0.lock();
        if inner.end.is_none() {
            inner.end = Some(end);
        }
        inner.wake();
    }
}

impl Inner {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

/// A program running through a relay. Dropped before it ended, it is stopped.
#[derive(Debug)]
pub(crate) struct Process {
    relay: Relay,
    id: u32,
    slot: std::sync::Arc<Slot>,
    /// Whether its standard input is open.
    input: bool,
    /// Output read since the relay was last told.
    taken: u32,
    ended: bool,
}

impl Process {
    pub(crate) const fn new(
        relay: Relay,
        id: u32,
        slot: std::sync::Arc<Slot>,
        input: bool,
    ) -> Self {
        Self {
            relay,
            id,
            slot,
            input,
            taken: 0,
            ended: false,
        }
    }

    /// Its id, as the relay knows it.
    pub(crate) const fn id(&self) -> u32 {
        self.id
    }

    /// The relay it runs through.
    pub(crate) const fn relay(&self) -> &Relay {
        &self.relay
    }

    /// Hands `data` to its standard input.
    ///
    /// # Errors
    ///
    /// When its input is closed.
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "the relay takes what it is sent at once; a slower one would be waited for"
    )]
    pub(crate) async fn write(&self, data: &[u8]) -> io::Result<()> {
        if !self.input {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the program's input is closed",
            ));
        }
        for chunk in data.chunks(INPUT_CHUNK) {
            self.relay.send(&ToRelay::Stdin {
                id: self.id,
                data: chunk.to_vec(),
            });
        }
        Ok(())
    }

    /// Ends its standard input.
    pub(crate) fn close_input(&mut self) {
        if std::mem::take(&mut self.input) {
            self.relay.send(&ToRelay::CloseStdin { id: self.id });
        }
    }

    /// Reads its standard output into `buffer`: how much, 0 once it ended.
    ///
    /// # Errors
    ///
    /// Never: how it ended is [`Self::wait`]'s to say.
    pub(crate) async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = poll_fn(|context| {
            let mut inner = self.slot.0.lock();
            let offset = inner.offset;
            if let Some(chunk) = inner.chunks.front() {
                let rest = chunk.get(offset..).unwrap_or_default();
                let count = rest.len().min(buffer.len());
                if let (Some(into), Some(from)) = (buffer.get_mut(..count), rest.get(..count)) {
                    into.copy_from_slice(from);
                }
                if count == rest.len() {
                    inner.chunks.pop_front();
                    inner.offset = 0;
                } else {
                    inner.offset = offset.saturating_add(count);
                }
                return Poll::Ready(count);
            }
            if inner.end.is_some() || buffer.is_empty() {
                return Poll::Ready(0);
            }
            inner.waker = Some(context.waker().clone());
            Poll::Pending
        })
        .await;
        self.taken = self
            .taken
            .saturating_add(u32::try_from(read).unwrap_or(u32::MAX));
        if self.taken >= CREDIT_STEP {
            self.relay.send(&ToRelay::Credit {
                id: self.id,
                bytes: std::mem::take(&mut self.taken),
            });
        }
        Ok(read)
    }

    /// Waits for it to end (once its output was read, or it was stopped), and says how.
    ///
    /// # Errors
    ///
    /// When it couldn't be started, or the connection ended first.
    pub(crate) async fn wait(&mut self) -> Result<Exit, NotRun> {
        // Output not read is dropped: the relay may send the rest.
        self.abandon_output();
        let ended = poll_fn(|context| {
            let mut inner = self.slot.0.lock();
            if let Some(end) = &inner.end {
                return Poll::Ready(end.clone());
            }
            inner.waker = Some(context.waker().clone());
            Poll::Pending
        })
        .await;
        self.ended = true;
        ended
    }

    /// Stops it and every process it started (SIGTERM to its group there).
    pub(crate) fn terminate(&self) {
        if !self.ended {
            self.relay.send(&ToRelay::Terminate { id: self.id });
        }
    }

    /// Takes in no more of its output: what waits is dropped, and the relay may send the rest.
    fn abandon_output(&mut self) {
        let waiting = {
            let mut inner = self.slot.0.lock();
            inner.abandoned = true;
            let offset = std::mem::take(&mut inner.offset);
            inner
                .chunks
                .drain(..)
                .map(|chunk| chunk.len())
                .sum::<usize>()
                .saturating_sub(offset)
        };
        let credit = self
            .taken
            .saturating_add(u32::try_from(waiting).unwrap_or(u32::MAX));
        self.taken = 0;
        if credit > 0 && !self.ended {
            self.relay.send(&ToRelay::Credit {
                id: self.id,
                bytes: credit,
            });
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        self.terminate();
        self.abandon_output();
    }
}

/// A request's answer's head: its status and headers (names in lower case).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    /// Its status.
    pub status: u16,
    /// Its headers.
    pub headers: Vec<(String, String)>,
}

/// A request a relay makes for egit ([`Relay::fetch`]): its answer's head, then its body as it
/// comes. Dropped before its end, the relay stops it.
#[derive(Debug)]
pub struct Fetching {
    process: Process,
}

impl Fetching {
    pub(crate) const fn new(process: Process) -> Self {
        Self { process }
    }

    /// Waits for its answer's head.
    ///
    /// # Errors
    ///
    /// Why there is none: the host couldn't be reached (in the relay's words), or the
    /// connection to the relay was lost.
    pub async fn head(&mut self) -> Result<Head, String> {
        let slot = std::sync::Arc::clone(&self.process.slot);
        poll_fn(|context| {
            let mut inner = slot.0.lock();
            if let Some(head) = inner.head.take() {
                return Poll::Ready(Ok(head));
            }
            if let Some(end) = &inner.end {
                return Poll::Ready(Err(match end {
                    Ok(_) => "it ended without an answer".to_owned(),
                    Err(NotRun::Missing(_, message) | NotRun::Lost(message)) => message.clone(),
                }));
            }
            inner.waker = Some(context.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// More of its answer's body: how much, 0 at its end.
    ///
    /// # Errors
    ///
    /// Never: how it ended is [`Self::end`]'s to say.
    pub async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.process.read(buffer).await
    }

    /// How its answer ended: whole, or not.
    ///
    /// # Errors
    ///
    /// Why not, in the relay's words, or the connection's.
    pub async fn end(&mut self) -> Result<(), String> {
        match self.process.wait().await {
            Ok(exit) if exit.success() => Ok(()),
            Ok(exit) => Err(format!("the request ended: {exit}")),
            Err(NotRun::Missing(_, message) | NotRun::Lost(message)) => Err(message),
        }
    }

    /// Stops it there.
    pub fn stop(&self) {
        self.process.terminate();
    }
}
