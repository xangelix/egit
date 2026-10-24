//! Git through a relay: egit's side of a connection to `egit-relay`.
//!
//! The relay runs every program egit asks for (git, and `sh` for the files it reads and writes)
//! on its computer, and relays what they print and what their helpers ask.
//!
//! The connection itself is the platform's ([`Transport`]: a browser's WebSocket, or a test's):
//! it sends what [`Relay`] hands it, in order, and hands [`Relay::receive`] each message that
//! comes, and [`Relay::closed`] its end. Everything else is here: the processes under way, each
//! read as it is printed (at most [`WINDOW`] of a process's output waiting before the relay is
//! told it was taken in), the prompts their helpers ask, and the ids that tell them apart.

mod process;
pub mod protocol;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::future::poll_fn;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::task::{Poll, Waker};

use parking_lot::Mutex;
use zeroize::Zeroizing;

pub use process::Fetching;
pub(crate) use process::{NotRun, Process, Stderr as StderrSink};
pub use protocol::{
    Fetch, FetchMode, FromRelay, Garbled, Method, Missing, Program, ToRelay, VERSION, WINDOW,
};

use crate::askpass::{Prompt, PromptKind};
use crate::git::Git;

/// Where a relay's messages go and come from: a connection the platform made.
pub trait Transport: Send + Sync + 'static {
    /// Sends `message` (one binary WebSocket message), after those sent before.
    fn send(&self, message: Vec<u8>);

    /// Closes the connection.
    fn close(&self);
}

/// A connection to a relay. Cheap to clone: clones share it.
#[derive(Clone)]
pub struct Relay(Arc<Shared>);

struct Shared {
    /// Tells relays apart, for [`Hash`] and [`PartialEq`].
    number: u64,
    transport: Box<dyn Transport>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_id: u32,
    processes: HashMap<u32, Arc<process::Slot>>,
    /// The relay's welcome, once said.
    welcome: Option<Welcome>,
    /// Why the connection ended, once it did.
    ended: Option<String>,
    /// Woken once welcomed or ended.
    waiting: Vec<Waker>,
    /// Where prompts go: the window's queue.
    prompts: Option<Prompts>,
    /// Prompts shown, by id: told when their asker goes away.
    asked: HashMap<u32, Arc<std::sync::atomic::AtomicBool>>,
}

/// Where a relay's prompts go, and what wakes the window as each comes.
type Prompts = (Sender<Prompt>, Arc<dyn Fn() + Send + Sync>);

/// What a relay said of itself as it welcomed egit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Welcome {
    /// Its home folder.
    pub home: Vec<u8>,
    /// The name of its computer.
    pub host: String,
}

static RELAYS: AtomicU64 = AtomicU64::new(0);

impl Relay {
    /// Speaks to a relay through `transport`, saying who it is with `token` (the relay's,
    /// as it printed it).
    pub fn new(transport: impl Transport, token: &[u8]) -> Self {
        let relay = Self(Arc::new(Shared {
            number: RELAYS.fetch_add(1, Ordering::Relaxed),
            transport: Box::new(transport),
            state: Mutex::new(State::default()),
        }));
        relay.send(&ToRelay::Hello {
            version: VERSION,
            token: Zeroizing::new(token.to_vec()),
        });
        relay
    }

    /// Hands the relay's prompts (git's and ssh's helpers' questions) to `prompts`, calling
    /// `notify` as each comes (to wake the window); until then they are refused.
    pub fn send_prompts_to(
        &self,
        prompts: Sender<Prompt>,
        notify: impl Fn() + Send + Sync + 'static,
    ) {
        self.0.state.lock().prompts = Some((prompts, Arc::new(notify)));
    }

    /// Waits for the relay's welcome.
    ///
    /// # Errors
    ///
    /// Why the connection ended first (the relay refused, or it was lost).
    pub async fn welcome(&self) -> Result<Welcome, String> {
        poll_fn(|context| {
            let mut state = self.0.state.lock();
            if let Some(welcome) = &state.welcome {
                return Poll::Ready(Ok(welcome.clone()));
            }
            if let Some(ended) = &state.ended {
                return Poll::Ready(Err(ended.clone()));
            }
            state.waiting.push(context.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Closes the connection: what runs through it ends here at once, and is stopped there.
    pub fn close(&self) {
        self.end("The connection to the relay was closed.");
        self.0.transport.close();
    }

    /// The relay's welcome, once it was said.
    #[must_use]
    pub fn welcomed(&self) -> Option<Welcome> {
        self.0.state.lock().welcome.clone()
    }

    /// Whether the connection ended.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.0.state.lock().ended.is_some()
    }

    /// Takes in a message from the relay.
    pub fn receive(&self, message: &[u8]) {
        let message = match FromRelay::decode(message) {
            Ok(message) => message,
            Err(garbled) => {
                tracing::warn!(%garbled, "a relay's message");
                self.end(&garbled.to_string());
                self.0.transport.close();
                return;
            }
        };
        match message {
            FromRelay::Welcome {
                version,
                home,
                host,
            } => {
                if version != VERSION {
                    self.end(&format!(
                        "The relay speaks version {version} of egit's protocol, not {VERSION}."
                    ));
                    return;
                }
                let mut state = self.0.state.lock();
                state.welcome = Some(Welcome { home, host });
                for waker in state.waiting.drain(..) {
                    waker.wake();
                }
            }
            FromRelay::Refused { why } => self.end(&why),
            FromRelay::Failed {
                id,
                missing,
                message,
            } => {
                let slot = self.0.state.lock().processes.remove(&id);
                if let Some(slot) = slot {
                    slot.failed(missing, message);
                }
            }
            FromRelay::Stdout { id, data } => {
                let slot = self.0.state.lock().processes.get(&id).cloned();
                if let Some(slot) = slot
                    && let Some(credit) = slot.stdout(data)
                {
                    // Nobody reads it any more: the relay may send the rest at once.
                    self.send(&ToRelay::Credit { id, bytes: credit });
                }
            }
            FromRelay::Stderr { id, data } => {
                let slot = self.0.state.lock().processes.get(&id).cloned();
                if let Some(slot) = slot {
                    slot.stderr(&data);
                }
            }
            FromRelay::Exited { id, exit } => {
                let slot = self.0.state.lock().processes.remove(&id);
                if let Some(slot) = slot {
                    slot.exited(exit);
                }
            }
            FromRelay::Prompt {
                id,
                kind,
                context,
                text,
            } => self.prompt(id, kind, context, text),
            FromRelay::Head {
                id,
                status,
                headers,
            } => {
                let slot = self.0.state.lock().processes.get(&id).cloned();
                if let Some(slot) = slot {
                    slot.head(process::Head { status, headers });
                }
            }
            FromRelay::Withdrawn { id } => {
                let withdrawn = self.0.state.lock().asked.remove(&id);
                if let Some(withdrawn) = withdrawn {
                    withdrawn.store(true, Ordering::Release);
                }
            }
        }
    }

    /// Takes in the connection's end, and why: every process under way fails with it.
    pub fn closed(&self, why: &str) {
        self.end(why);
    }

    fn end(&self, why: &str) {
        let (processes, waiting, asked) = {
            let mut state = self.0.state.lock();
            if state.ended.is_none() {
                state.ended = Some(why.to_owned());
            }
            (
                std::mem::take(&mut state.processes),
                std::mem::take(&mut state.waiting),
                std::mem::take(&mut state.asked),
            )
        };
        for slot in processes.into_values() {
            slot.lost(why);
        }
        for waker in waiting {
            waker.wake();
        }
        for withdrawn in asked.into_values() {
            withdrawn.store(true, Ordering::Release);
        }
    }

    /// A prompt the relay relays, for the window.
    fn prompt(&self, id: u32, kind: u8, context: Option<String>, text: String) {
        let Some(kind) = PromptKind::from_code(kind) else {
            self.send(&ToRelay::Answer { id, answer: None });
            return;
        };
        let withdrawn = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut state = self.0.state.lock();
        let Some((prompts, notify)) = state.prompts.clone() else {
            drop(state);
            self.send(&ToRelay::Answer { id, answer: None });
            return;
        };
        state.asked.insert(id, Arc::clone(&withdrawn));
        drop(state);
        let prompt = Prompt::relayed(kind, text, context, (self.clone(), id, withdrawn));
        // A window gone refuses it as it drops.
        if prompts.send(prompt).is_ok() {
            notify();
        }
    }

    /// Answers prompt `id` (`None` refuses it).
    pub(crate) fn answer(&self, id: u32, answer: Option<Zeroizing<Vec<u8>>>) {
        self.0.state.lock().asked.remove(&id);
        self.send(&ToRelay::Answer { id, answer });
    }

    /// Starts `program` there; what it prints to standard error goes to `stderr` as it comes.
    pub(crate) fn start(&self, program: Program, stderr: StderrSink) -> Process {
        let reads = program.stdin;
        self.begin(|id| ToRelay::Start { id, program }, stderr, reads)
    }

    /// Begins what `asking` asks (a program, a request) under an id of its own.
    fn begin(
        &self,
        asking: impl FnOnce(u32) -> ToRelay,
        stderr: StderrSink,
        reads: bool,
    ) -> Process {
        let slot = Arc::new(process::Slot::new(stderr));
        let mut state = self.0.state.lock();
        let id = if let Some(ended) = &state.ended {
            slot.lost(ended);
            0
        } else {
            // Ids run on; after four billion they start over, long after the first ended.
            state.next_id = state.next_id.wrapping_add(1).max(1);
            let id = state.next_id;
            state.processes.insert(id, Arc::clone(&slot));
            id
        };
        drop(state);
        if id != 0 {
            self.send(&asking(id));
        }
        Process::new(self.clone(), id, slot, reads)
    }

    /// Has the relay make `fetch` (a request to a forge, a download, a picture): its answer
    /// read as it comes, as a program's output is.
    #[must_use = "dropped, the request is stopped"]
    pub fn fetch(&self, fetch: Fetch) -> Fetching {
        let process = self.begin(|id| ToRelay::Fetch { id, fetch }, Box::new(|_| {}), false);
        Fetching::new(process)
    }

    /// Hands `data` to process `id`'s standard input.
    pub(crate) fn input(&self, id: u32, data: &[u8]) {
        for chunk in data.chunks(64 * 1024) {
            self.send(&ToRelay::Stdin {
                id,
                data: chunk.to_vec(),
            });
        }
    }

    /// Ends process `id`'s standard input.
    pub(crate) fn close_input(&self, id: u32) {
        self.send(&ToRelay::CloseStdin { id });
    }

    /// Asks the relay to stop process `id` and every process it started.
    pub(crate) fn terminate(&self, id: u32) {
        self.send(&ToRelay::Terminate { id });
    }

    fn send(&self, message: &ToRelay) {
        self.0.transport.send(message.encode());
    }
}

impl fmt::Debug for Relay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.0.state.lock();
        f.debug_struct("Relay")
            .field("number", &self.0.number)
            .field("host", &state.welcome.as_ref().map(|welcome| &welcome.host))
            .field("ended", &state.ended)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Relay {
    fn eq(&self, other: &Self) -> bool {
        self.0.number == other.0.number
    }
}

impl Eq for Relay {}

impl Hash for Relay {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.number.hash(state);
    }
}

/// What runs `program` with `args` where `git` runs through its relay: in `dir`, with git's
/// environment there and `env` over it, but none of this computer's helpers (the relay has its
/// own, told what the command is doing); reading nothing on standard input until told
/// otherwise.
pub(crate) fn program_there(
    git: &Git,
    program: &OsStr,
    args: &[&OsStr],
    dir: Option<&Path>,
    env: &[(&OsStr, &OsStr)],
) -> Program {
    let environment = git.remote_environment();
    let mut set: Vec<(Vec<u8>, Vec<u8>)> = environment
        .map(|environment| {
            environment
                .settings()
                .map(|(key, value)| (bytes(key), bytes(value)))
                .collect()
        })
        .unwrap_or_default();
    set.extend(
        env.iter()
            .filter(|(key, _)| !crate::env::is_local_only(key) || crate::env::tells_the_helper(key))
            .map(|(key, value)| (bytes(key), bytes(value))),
    );
    Program {
        program: bytes(program),
        args: args.iter().map(|arg| bytes(arg)).collect(),
        dir: dir.map(|dir| bytes(dir.as_os_str())),
        set,
        unset: environment
            .map(|environment| environment.removals().map(bytes).collect())
            .unwrap_or_default(),
        stdin: false,
    }
}

/// `os`'s bytes, as the relay's computer has them (byte for byte on Unix).
fn bytes(os: &OsStr) -> Vec<u8> {
    <Vec<u8> as bstr::ByteVec>::from_os_str_lossy(os).into_owned()
}

impl From<NotRun> for std::io::Error {
    fn from(not_run: NotRun) -> Self {
        match not_run {
            NotRun::Missing(Missing::Nothing, message) | NotRun::Lost(message) => {
                Self::other(message)
            }
            NotRun::Missing(Missing::Program | Missing::Folder, message) => {
                Self::new(std::io::ErrorKind::NotFound, message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::exec::Wait as _;
    use crate::output::Exit;

    /// A connection that keeps what egit sent.
    #[derive(Clone, Default)]
    struct Kept(Arc<Mutex<Vec<ToRelay>>>);

    impl Transport for Kept {
        fn send(&self, message: Vec<u8>) {
            self.0.lock().push(ToRelay::decode(&message).unwrap());
        }

        fn close(&self) {}
    }

    impl Kept {
        fn take(&self) -> Vec<ToRelay> {
            std::mem::take(&mut *self.0.lock())
        }
    }

    fn welcomed() -> (Relay, Kept) {
        let kept = Kept::default();
        let relay = Relay::new(kept.clone(), b"token");
        assert!(matches!(
            kept.take().as_slice(),
            [ToRelay::Hello { version: VERSION, token }] if token.as_slice() == b"token"
        ));
        relay.receive(
            &FromRelay::Welcome {
                version: VERSION,
                home: b"/home/ada".to_vec(),
                host: "devbox".to_owned(),
            }
            .encode(),
        );
        (relay, kept)
    }

    fn program() -> Program {
        Program {
            program: b"git".to_vec(),
            args: vec![b"log".to_vec()],
            ..Program::default()
        }
    }

    #[test]
    fn a_relay_welcomes_or_refuses() {
        let (relay, _) = welcomed();
        assert_eq!(relay.welcome().wait().unwrap().host, "devbox");
        let refused = Relay::new(Kept::default(), b"wrong");
        refused.receive(
            &FromRelay::Refused {
                why: "That isn't this relay's token.".to_owned(),
            }
            .encode(),
        );
        assert_eq!(
            refused.welcome().wait().unwrap_err(),
            "That isn't this relay's token."
        );
        assert!(refused.has_ended());
    }

    #[test]
    fn a_program_s_output_is_read_as_it_comes_and_taken_in_told() {
        let (relay, kept) = welcomed();
        let said = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let said = Arc::clone(&said);
            Box::new(move |chunk: &[u8]| said.lock().extend_from_slice(chunk))
        };
        let mut process = relay.start(program(), sink);
        let id = process.id();
        assert!(matches!(kept.take().as_slice(), [ToRelay::Start { .. }]));
        // More than a credit's step, in chunks, then its end.
        let chunk = vec![b'x'; 100_000];
        for _ in 0..4 {
            relay.receive(
                &FromRelay::Stdout {
                    id,
                    data: chunk.clone(),
                }
                .encode(),
            );
        }
        relay.receive(
            &FromRelay::Stderr {
                id,
                data: b"warning: careful\n".to_vec(),
            }
            .encode(),
        );
        relay.receive(
            &FromRelay::Exited {
                id,
                exit: Exit::new(Some(0), None),
            }
            .encode(),
        );
        let mut read = Vec::new();
        let mut buffer = vec![0; 30_000];
        loop {
            let got = process.read(&mut buffer).wait().unwrap();
            if got == 0 {
                break;
            }
            read.extend_from_slice(&buffer[..got]);
        }
        assert_eq!(read.len(), 400_000);
        assert_eq!(process.wait().wait(), Ok(Exit::new(Some(0), None)));
        assert_eq!(said.lock().as_slice(), b"warning: careful\n");
        let credited: u32 = kept
            .take()
            .iter()
            .map(|sent| match sent {
                ToRelay::Credit { bytes, .. } => *bytes,
                other => panic!("{other:?}"),
            })
            .sum();
        assert!((WINDOW >> 2..=400_000).contains(&credited), "{credited}");
    }

    #[test]
    fn a_program_dropped_before_its_end_is_stopped() {
        let (relay, kept) = welcomed();
        let process = relay.start(program(), Box::new(|_| {}));
        let id = process.id();
        relay.receive(
            &FromRelay::Stdout {
                id,
                data: vec![0; 1000],
            }
            .encode(),
        );
        kept.take();
        drop(process);
        let sent = kept.take();
        assert!(sent.contains(&ToRelay::Terminate { id }), "{sent:?}");
        assert!(
            sent.contains(&ToRelay::Credit { id, bytes: 1000 }),
            "{sent:?}"
        );
        // What comes after is let through at once.
        relay.receive(
            &FromRelay::Stdout {
                id,
                data: vec![0; 500],
            }
            .encode(),
        );
        assert_eq!(kept.take(), [ToRelay::Credit { id, bytes: 500 }]);
    }

    #[test]
    fn a_lost_connection_ends_what_runs_and_starts_nothing() {
        let (relay, kept) = welcomed();
        let mut process = relay.start(program(), Box::new(|_| {}));
        relay.closed("the network went away");
        let mut buffer = [0; 16];
        assert_eq!(process.read(&mut buffer).wait().unwrap(), 0);
        assert_eq!(
            process.wait().wait(),
            Err(NotRun::Lost("the network went away".to_owned()))
        );
        kept.take();
        let mut later = relay.start(program(), Box::new(|_| {}));
        assert_eq!(kept.take(), []);
        assert!(matches!(later.wait().wait(), Err(NotRun::Lost(_))));
    }

    #[test]
    fn a_prompt_is_relayed_and_answered_or_withdrawn() {
        let (relay, kept) = welcomed();
        // Before anyone shows them, prompts are refused.
        relay.receive(
            &FromRelay::Prompt {
                id: 1,
                kind: PromptKind::Secret.code(),
                context: None,
                text: "Password: ".to_owned(),
            }
            .encode(),
        );
        assert_eq!(
            kept.take(),
            [ToRelay::Answer {
                id: 1,
                answer: None
            }]
        );
        let (sender, prompts) = mpsc::channel();
        relay.send_prompts_to(sender, || {});
        relay.receive(
            &FromRelay::Prompt {
                id: 2,
                kind: PromptKind::Secret.code(),
                context: Some("Fetching origin".to_owned()),
                text: "Password: ".to_owned(),
            }
            .encode(),
        );
        let prompt = prompts.try_recv().unwrap();
        assert_eq!(prompt.kind(), PromptKind::Secret);
        assert_eq!(prompt.context(), Some("Fetching origin"));
        assert!(!prompt.is_abandoned());
        prompt.answer(b"hunter2").unwrap();
        assert!(matches!(
            kept.take().as_slice(),
            [ToRelay::Answer { id: 2, answer: Some(answer) }] if answer.as_slice() == b"hunter2"
        ));
        relay.receive(
            &FromRelay::Prompt {
                id: 3,
                kind: PromptKind::Notice.code(),
                context: None,
                text: "Touch your key".to_owned(),
            }
            .encode(),
        );
        let notice = prompts.try_recv().unwrap();
        relay.receive(&FromRelay::Withdrawn { id: 3 }.encode());
        assert!(notice.is_abandoned());
    }
}
