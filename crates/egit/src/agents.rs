//! Agents: Claude Code working in a repository's work tree beside egit, through `egit-agents`'
//! bridge. [`AgentsSetup`] is the window's (Node found, the SDK installed, who is signed in);
//! [`RepoAgents`] a repository tab's: its conversations, running or resumed, and the past ones
//! Claude Code keeps for it.
//!
//! A conversation starts its bridge (in a job) with its first prompt; until then nothing runs.
//! What the bridge says is taken in each frame ([`RepoAgents::poll`]): the SDK's messages into
//! the conversation's transcript, permission requests into the answers awaited.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egit_agents::accounts::{AuthStatus, Signer, auth_status};
use egit_agents::install::{Install, Node, find_node};
use egit_agents::protocol::{Answer, Event, Info, Mode, PermissionRequest, Request};
use egit_agents::sessions::{SessionInfo, list_sessions, session_history};
use egit_agents::{Bridge, Launch, Level, Transcript};
use egit_git::CancelToken;
use egui_async::Bind;
use serde_json::Value;

use crate::jobs;
use crate::problem::Problem;

/// How many past conversations are listed.
const PAST: usize = 50;

/// Claude's setup, for the whole window.
pub(crate) struct AgentsSetup {
    /// Where the SDK is installed (none without a cache folder).
    pub(crate) install: Option<Install>,
    /// Node and npm, and whether the SDK is installed, once looked for.
    pub(crate) found: Bind<Found, Problem>,
    /// Installing the SDK, and why the last try failed.
    pub(crate) installing: Bind<(), Problem>,
    pub(crate) install_problem: Option<Problem>,
    cancel: CancelToken,
    /// Who is signed in to Claude Code's own folder.
    pub(crate) signed_in: Bind<AuthStatus, Problem>,
    /// The bridge run in place of the installed one (the tests' stand-in SDK).
    #[cfg(test)]
    stand_in: Option<Launch>,
}

/// What looking for Node found.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub(crate) node: Node,
    /// Whether the SDK is installed already.
    pub(crate) ready: bool,
}

impl AgentsSetup {
    /// Claude's setup, installed under `cache` (egit's cache folder).
    pub(crate) fn new(cache: Option<&Path>) -> Self {
        Self {
            install: cache.map(Install::at),
            found: Bind::new(true),
            installing: Bind::new(true),
            install_problem: None,
            cancel: CancelToken::new(),
            signed_in: Bind::new(true),
            #[cfg(test)]
            stand_in: None,
        }
    }

    /// Takes in what its jobs found; looks for Node the first time it is asked, and who is
    /// signed in once the SDK is there.
    pub(crate) fn poll(&mut self) {
        self.found.poll();
        self.installing.poll();
        self.signed_in.poll();
        // Taken as it lands (not `just_completed`, whose frame every window in a process
        // shares): installed, Node and the SDK are looked for again; failed, said.
        match self.installing.take() {
            Some(Ok(())) => self.look_again(),
            Some(Err(problem)) => self.install_problem = Some(problem),
            None => {}
        }
        if self.is_ready() && self.signed_in.is_idle() {
            self.ask_who();
        }
    }

    /// Looks for Node (and the SDK) when nothing has yet.
    pub(crate) fn look(&mut self) {
        if self.found.is_idle() {
            self.look_again();
        }
    }

    /// Looks for Node and the SDK again (after installing either).
    pub(crate) fn look_again(&mut self) {
        let install = self.install.clone();
        self.found.request(jobs::blocking(move || {
            let node = find_node()
                .map_err(|error| Problem::new("Node.js wasn't found", error.to_string()))?;
            let ready = install.as_ref().is_some_and(Install::is_ready);
            Ok(Found { node, ready })
        }));
        self.signed_in = Bind::new(true);
    }

    /// Installs the SDK with the Node found.
    pub(crate) fn install(&mut self) {
        let (Some(install), Some(Ok(found))) = (self.install.clone(), self.found.read_as_ref())
        else {
            return;
        };
        let node = found.node.clone();
        self.install_problem = None;
        self.cancel = CancelToken::new();
        let cancel = self.cancel.clone();
        self.installing.request(jobs::blocking(move || {
            install
                .install(&node, &cancel)
                .map_err(|error| Problem::new("Claude couldn't be set up", error.to_string()))
        }));
    }

    /// Stops installing.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether conversations can start: Node found, the SDK installed.
    pub(crate) fn is_ready(&mut self) -> bool {
        matches!(self.found.read_as_ref(), Some(Ok(found)) if found.ready)
    }

    /// Asks Claude Code who is signed in to its own folder.
    pub(crate) fn ask_who(&mut self) {
        let Some(claude) = self.install.as_ref().and_then(Install::claude) else {
            // Nothing to ask (a stand-in in the tests): said once, not looked for each frame.
            self.signed_in.fill(Err(Problem::new(
                "Claude Code isn't installed",
                String::new(),
            )));
            return;
        };
        self.signed_in.request(jobs::blocking(move || {
            auth_status(&claude, &Signer::Default).map_err(|error| {
                Problem::new(
                    "Claude Code couldn't say who is signed in",
                    error.to_string(),
                )
            })
        }));
    }

    /// Runs `launch` in place of the installed bridge, Node found and Claude ready (the tests:
    /// a stand-in SDK).
    #[cfg(test)]
    pub(crate) fn stand_in(&mut self, node: Node, launch: Launch) {
        self.stand_in = Some(launch);
        self.found.fill(Ok(Found { node, ready: true }));
    }

    /// How to run the bridge as `signer`, once it is installed.
    pub(crate) fn launch(&mut self, signer: &Signer) -> Option<Launch> {
        #[cfg(test)]
        if let Some(launch) = &self.stand_in {
            return Some(launch.clone());
        }
        let install = self.install.clone()?;
        let Some(Ok(found)) = self.found.read_as_ref() else {
            return None;
        };
        let (env, unset) = signer.environment();
        Some(Launch {
            node: found.node.node.clone(),
            bridge: install.bridge(),
            env,
            unset,
        })
    }
}

/// A repository tab's agents.
pub(crate) struct RepoAgents {
    /// Whether the panel shows.
    pub(crate) shown: bool,
    /// Its conversations, the newest last.
    pub(crate) conversations: Vec<Conversation>,
    /// The one shown.
    pub(crate) current: usize,
    /// The past conversations Claude Code keeps for the work tree, newest first.
    pub(crate) past: Bind<Arc<[SessionInfo]>, Problem>,
    /// Wakes the window when a bridge says something.
    waker: egui::Context,
}

impl RepoAgents {
    pub(crate) fn new(waker: &egui::Context) -> Self {
        Self {
            shown: false,
            conversations: vec![Conversation::new(None)],
            current: 0,
            past: Bind::new(true),
            waker: waker.clone(),
        }
    }

    /// The conversation shown.
    pub(crate) fn current(&mut self) -> Option<&mut Conversation> {
        self.conversations.get_mut(self.current)
    }

    /// Takes in what each conversation's bridge and jobs said.
    pub(crate) fn poll(&mut self) {
        self.past.poll();
        for conversation in &mut self.conversations {
            conversation.poll();
        }
    }

    /// Lists the past conversations again.
    pub(crate) fn list_past(&mut self, launch: Option<Launch>, work_tree: &Path) {
        let Some(launch) = launch else {
            return;
        };
        let dir = work_tree.to_owned();
        self.past.request(jobs::blocking(move || {
            list_sessions(&launch, &dir, PAST)
                .map(Arc::from)
                .map_err(|error| {
                    Problem::new(
                        "Earlier conversations couldn't be listed",
                        error.to_string(),
                    )
                })
        }));
    }

    /// Starts a new conversation (or shows the empty one there is).
    pub(crate) fn begin(&mut self) {
        if let Some(index) = self.conversations.iter().position(Conversation::is_blank) {
            self.current = index;
            return;
        }
        self.conversations.push(Conversation::new(None));
        self.current = self.conversations.len().saturating_sub(1);
    }

    /// Opens a past conversation (or shows it, if open), its messages read in a job.
    pub(crate) fn resume(
        &mut self,
        launch: Option<Launch>,
        work_tree: &Path,
        session: &SessionInfo,
    ) {
        if let Some(index) = self.conversations.iter().position(|conversation| {
            conversation.resume.as_deref() == Some(session.session_id.as_str())
                || conversation.transcript.session() == Some(session.session_id.as_str())
        }) {
            self.current = index;
            return;
        }
        let mut conversation = Conversation::new(Some(session.session_id.clone()));
        conversation.title.clone_from(&session.summary);
        if let Some(launch) = launch {
            let dir = work_tree.to_owned();
            let id = session.session_id.clone();
            conversation.history.request(jobs::blocking(move || {
                session_history(&launch, &dir, &id)
                    .map(Arc::new)
                    .map_err(|error| {
                        Problem::new("The conversation couldn't be read", error.to_string())
                    })
            }));
        }
        // An empty one gives way to it.
        if let Some(blank) = self.conversations.iter().position(Conversation::is_blank) {
            self.conversations.remove(blank);
        }
        self.conversations.push(conversation);
        self.current = self.conversations.len().saturating_sub(1);
    }

    /// Closes conversation `index` (its bridge ends), keeping one to show.
    pub(crate) fn close(&mut self, index: usize) {
        if index < self.conversations.len() {
            self.conversations.remove(index);
        }
        if self.conversations.is_empty() {
            self.conversations.push(Conversation::new(None));
        }
        self.current = self.current.min(self.conversations.len().saturating_sub(1));
    }

    /// Sends `text` to the conversation shown, starting its bridge first if it isn't running.
    pub(crate) fn send(&mut self, launch: Option<Launch>, work_tree: &Path, text: &str) {
        let waker = self.waker.clone();
        if let Some(conversation) = self.current() {
            conversation.send(launch, work_tree, text, waker);
        }
    }
}

/// A conversation with Claude.
pub(crate) struct Conversation {
    /// What it is called: its title, or its first prompt.
    pub(crate) title: String,
    /// What was said and done.
    pub(crate) transcript: Transcript,
    /// Tools waiting for permission, the oldest first.
    pub(crate) waiting: Vec<PermissionRequest>,
    /// Who it runs as, and the models and commands there are, once asked.
    pub(crate) info: Option<Info>,
    /// The mode it is in: as asked, or as Claude Code last said it changed to.
    pub(crate) mode: Mode,
    /// The mode Claude Code last said it was in.
    reported: Option<Mode>,
    /// The model asked for (`None`: Claude Code's default).
    pub(crate) model: Option<String>,
    /// The past session it goes on with.
    pub(crate) resume: Option<String>,
    /// Its past messages, being read.
    pub(crate) history: Bind<Arc<Vec<Value>>, Problem>,
    /// Its bridge starting.
    starting: Bind<Started, Problem>,
    bridge: Option<Bridge>,
    events: Option<Receiver<Event>>,
    /// Whether its bridge said it is ready (requests before then wait in `queued`).
    ready: bool,
    /// Requests made before its bridge was ready, sent once it is, in order.
    queued: Vec<Request>,
    /// Whether Claude Code's information was asked for.
    asked_info: bool,
    /// What its view keeps (the draft, what is folded open, heights).
    pub(crate) view: crate::views::agents::ConversationView,
}

/// A bridge started in a job.
struct Started {
    bridge: Bridge,
    events: Receiver<Event>,
}

impl Conversation {
    fn new(resume: Option<String>) -> Self {
        Self {
            title: String::new(),
            transcript: Transcript::default(),
            waiting: Vec::new(),
            info: None,
            mode: Mode::Default,
            reported: None,
            model: None,
            resume,
            history: Bind::new(true),
            starting: Bind::new(true),
            bridge: None,
            events: None,
            ready: false,
            queued: Vec::new(),
            asked_info: false,
            view: crate::views::agents::ConversationView::default(),
        }
    }

    /// Whether nothing has happened in it yet.
    pub(crate) fn is_blank(&self) -> bool {
        self.resume.is_none() && self.transcript.items().is_empty() && self.bridge.is_none()
    }

    /// Whether its bridge is starting or running.
    pub(crate) fn is_running(&mut self) -> bool {
        self.bridge.is_some() || self.starting.is_pending()
    }

    /// Whether a turn is under way.
    pub(crate) fn is_busy(&mut self) -> bool {
        self.transcript.busy() || self.starting.is_pending()
    }

    /// The mode it is in.
    pub(crate) const fn mode(&self) -> Mode {
        self.mode
    }

    /// Sends a prompt, starting the bridge first if it isn't running.
    fn send(&mut self, launch: Option<Launch>, work_tree: &Path, text: &str, waker: egui::Context) {
        if self.title.is_empty() {
            text.lines()
                .next()
                .unwrap_or_default()
                .clone_into(&mut self.title);
        }
        self.transcript.prompt(text);
        self.view.follow();
        let request = Request::Send {
            text: text.to_owned(),
        };
        if self.is_running() {
            self.deliver(request);
            return;
        }
        self.queued.push(request);
        let Some(launch) = launch else {
            self.transcript
                .notice(Level::Error, "Claude isn't set up on this computer.");
            self.transcript.ended();
            return;
        };
        let start = Request::Start {
            cwd: work_tree.to_string_lossy().into_owned(),
            resume: self
                .transcript
                .session()
                .map(str::to_owned)
                .or_else(|| self.resume.clone()),
            model: self.model.clone(),
            mode: self.mode,
        };
        self.queued.insert(0, start);
        self.starting.request(jobs::blocking(move || {
            let (bridge, events) = Bridge::start(&launch, move || waker.request_repaint())
                .map_err(|error| Problem::new("Claude couldn't be started", error.to_string()))?;
            Ok(Started { bridge, events })
        }));
    }

    /// Answers a permission request (and stops waiting for it).
    pub(crate) fn answer(&mut self, answer: Answer) {
        self.waiting
            .retain(|waiting| waiting.request != answer.request);
        self.deliver(Request::Answer(answer));
    }

    /// Stops the turn under way.
    pub(crate) fn interrupt(&mut self) {
        self.deliver(Request::Interrupt);
    }

    /// Changes the permission mode (now, or for when it starts).
    pub(crate) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.deliver(Request::Mode { mode });
    }

    /// Changes the model (now, or for when it starts).
    pub(crate) fn set_model(&mut self, model: Option<String>) {
        self.model.clone_from(&model);
        self.deliver(Request::Model { model });
    }

    /// Hands `request` to the bridge: at once when it is ready, after the requests before it
    /// while it starts; nothing while it isn't running (what it would change is kept for its
    /// start).
    fn deliver(&mut self, request: Request) {
        if self.ready {
            self.write(&request);
        } else if self.is_running() {
            self.queued.push(request);
        }
    }

    /// Writes `request` to the bridge; a failure is said in the transcript.
    fn write(&mut self, request: &Request) {
        let Some(bridge) = &self.bridge else {
            return;
        };
        if let Err(error) = bridge.send(request) {
            self.transcript
                .notice(Level::Error, format!("Claude couldn't be told: {error}"));
        }
    }

    /// Takes in what its jobs and bridge said.
    fn poll(&mut self) {
        // Each job's result taken as it lands (not `just_completed`, whose frame every window
        // in a process shares, so another's frame can pass between landing and asking).
        match self.history.take() {
            Some(Ok(messages)) => {
                self.transcript.history(&messages);
                self.view.follow();
            }
            Some(Err(problem)) => {
                let said = format!("{}: {}", problem.title, problem.detail);
                self.transcript.notice(Level::Error, said);
            }
            None => {}
        }
        match self.starting.take() {
            Some(Ok(started)) => {
                self.bridge = Some(started.bridge);
                self.events = Some(started.events);
            }
            Some(Err(problem)) => {
                self.queued.clear();
                self.transcript.notice(
                    Level::Error,
                    format!("{}: {}", problem.title, problem.detail),
                );
                self.transcript.ended();
            }
            None => {}
        }
        let events: Vec<Event> = self
            .events
            .as_ref()
            .map(|events| events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            self.event(event);
        }
        // Claude Code changed mode itself (a plan approved, say): the menu follows.
        let reported = self.transcript.mode();
        if reported.is_some() && reported != self.reported {
            self.reported = reported;
            if let Some(mode) = reported {
                self.mode = mode;
            }
        }
    }

    /// One thing the bridge said.
    fn event(&mut self, event: Event) {
        match event {
            Event::Ready => {
                self.ready = true;
                for request in std::mem::take(&mut self.queued) {
                    self.write(&request);
                }
            }
            Event::Message { message } => {
                let first = self.transcript.session().is_none();
                self.transcript.message(&message);
                if first && self.transcript.session().is_some() && !self.asked_info {
                    self.asked_info = true;
                    self.write(&Request::Info);
                }
            }
            Event::Permission(asked) => self.waiting.push(asked),
            Event::Withdrawn { request } => {
                self.waiting.retain(|waiting| waiting.request != request);
            }
            Event::Info(info) => self.info = Some(info),
            Event::Failed { op, message } => {
                let what = op.map_or_else(String::new, |op| format!(" ({op})"));
                self.transcript.notice(
                    Level::Error,
                    format!("Claude couldn't do that{what}: {message}"),
                );
            }
            Event::Ended { error } => {
                if let Some(error) = error {
                    self.transcript.notice(Level::Error, error);
                }
                self.transcript.ended();
                self.waiting.clear();
                self.bridge = None;
                self.events = None;
                self.ready = false;
                self.queued.clear();
            }
        }
    }
}

/// egit's folder for its agents' own files (the cache's sibling is the SDK's).
pub(crate) fn cache_folder(home: Option<&Path>) -> Option<PathBuf> {
    crate::avatars::cache_folder(home)
        .and_then(|pictures| pictures.ancestors().nth(2).map(Path::to_owned))
}
