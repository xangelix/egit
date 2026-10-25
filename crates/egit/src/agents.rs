//! Agents: Claude Code working in a repository's work tree beside egit, through `egit-agents`'
//! bridge. [`AgentsSetup`] is the window's (Node found, the SDK installed, who is signed in);
//! [`RepoAgents`] a repository tab's: its conversations, running or resumed, and the past ones
//! Claude Code keeps for it.
//!
//! A conversation starts its bridge (in a job) with its first prompt; until then nothing runs.
//! What the bridge says is taken in each frame ([`RepoAgents::poll`]): the SDK's messages into
//! the conversation's transcript, permission requests into the answers awaited.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egit_agents::accounts::{AuthStatus, Signer, auth_status};
use egit_agents::install::{Install, Node, find_node, find_node_there};
use egit_agents::protocol::{
    Answer, Event, Image, Info, Mode, PermissionRequest, Request, Secrets,
};
use egit_agents::sessions::{SessionInfo, list_sessions, session_history};
use egit_agents::{Bridge, Launch, Level, Prompt, Transcript};
use egit_git::CancelToken;
use egit_git::{Asking, CredentialPlace, Git, fill_credential, store_credential};
use egui_async::Bind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jobs;
use crate::problem::Problem;
use crate::settings::StoredPath;

/// How many past conversations are listed.
const PAST: usize = 50;

/// The host an account's API key is kept for, by git's credential helpers (the account's name
/// its user).
pub(crate) const API_HOST: &str = "api.anthropic.com";

/// An account agents run as, besides Claude Code's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentAccount {
    /// Its name, as the user gave it (each account's own).
    pub(crate) name: String,
    /// How it signs in.
    pub(crate) kind: AccountKind,
}

/// How an account signs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AccountKind {
    /// A Claude Code configuration folder of its own, signed into with Claude Code itself.
    Folder(StoredPath),
    /// An Anthropic API key, kept by git's credential helpers (never in egit's settings).
    ApiKey,
}

impl AgentAccount {
    /// Where git's helpers keep its key.
    pub(crate) fn place(&self) -> CredentialPlace<'_> {
        CredentialPlace {
            protocol: "https",
            host: API_HOST,
            username: Some(&self.name),
        }
    }

    /// Who it runs as without its key: its folder, or Claude Code's own (where an API key's
    /// sessions are kept). For listing sessions, and asking who is signed in.
    pub(crate) fn folder_signer(&self) -> Signer {
        match &self.kind {
            AccountKind::Folder(folder) => Signer::Folder(folder.to_path()),
            AccountKind::ApiKey => Signer::Default,
        }
    }

    /// Who it runs as: its folder, or its key filled from git's helpers (never asking). Blocks:
    /// in a job.
    async fn signer(&self, git: Option<&Git>) -> Result<Signer, Problem> {
        if let AccountKind::Folder(folder) = &self.kind {
            return Ok(Signer::Folder(folder.to_path()));
        }
        let missing = || {
            Problem::new(
                format!("No API key is kept for {}", self.name),
                "git's credential helpers have none for it: add the account again in \
                 Preferences.",
            )
        };
        let git = git.ok_or_else(missing)?;
        let filled = fill_credential(git, None, self.place(), Asking::Never, &CancelToken::new())
            .await
            .map_err(|error| {
                Problem::git(
                    format!("The API key for {} couldn't be read", self.name),
                    &error,
                )
            })?
            .ok_or_else(missing)?;
        Ok(Signer::ApiKey(zeroize::Zeroizing::new(
            filled.password().to_owned(),
        )))
    }
}

/// What the Preferences' form adding an account holds.
#[derive(Default)]
pub(crate) struct AccountForm {
    /// The account's name.
    pub(crate) name: String,
    /// An API key rather than a folder.
    pub(crate) api_key: bool,
    /// The folder, as typed (`~` the home folder).
    pub(crate) folder: String,
    /// The key, as typed.
    pub(crate) key: zeroize::Zeroizing<String>,
}

/// How a conversation's bridge starts: the bridge as Claude Code's own account would run it,
/// git, to fill another's API key, and where an account's folder is on the computer it runs on.
#[derive(Clone)]
pub(crate) struct Starter {
    pub(crate) launch: Launch,
    pub(crate) git: Option<Git>,
    pub(crate) homes: Homes,
}

/// The home folders of this computer and of the one conversations run on, when that is
/// another: an account's Claude Code folder under this one's (`~/.claude-work`) is the same
/// folder under the home folder there, where Claude Code there signs into it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Homes {
    pub(crate) here: Option<PathBuf>,
    pub(crate) there: Option<PathBuf>,
}

impl Homes {
    /// Where `folder` (as this computer names it) is on the computer conversations run on.
    pub(crate) fn folder(&self, folder: &Path) -> PathBuf {
        let Some(there) = &self.there else {
            return folder.to_owned();
        };
        self.here
            .as_deref()
            .and_then(|here| folder.strip_prefix(here).ok())
            .map_or_else(|| folder.to_owned(), |inside| there.join(inside))
    }

    /// `signer` on the computer conversations run on (its folder placed there).
    pub(crate) fn place(&self, signer: Signer) -> Signer {
        match signer {
            Signer::Folder(folder) => Signer::Folder(self.folder(&folder)),
            other => other,
        }
    }
}

/// Claude's setup on a computer: this one (for the whole window), or another a repository is
/// on (`there`: its git).
pub(crate) struct AgentsSetup {
    /// Where the SDK is installed here (none without a cache folder); there, found with Node.
    install: Option<Install>,
    /// The other computer's git, when the setup is there.
    there: Option<Git>,
    /// This computer's home folder, and the other's (where accounts' folders are there).
    homes: Homes,
    /// Node and npm, and whether the SDK is installed, once looked for.
    pub(crate) found: Bind<Found, Problem>,
    /// Installing the SDK, and why the last try failed.
    pub(crate) installing: Bind<(), Problem>,
    pub(crate) install_problem: Option<Problem>,
    cancel: CancelToken,
    /// Who is signed in, by Claude Code's configuration folder (`None`: its own), as asked.
    statuses: HashMap<Option<PathBuf>, Bind<AuthStatus, Problem>>,
    /// An account being added (its API key stored in a job), and the form adding it.
    pub(crate) adding: Bind<AgentAccount, Problem>,
    pub(crate) form: AccountForm,
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
    /// Where it is (or goes).
    pub(crate) install: Option<Install>,
    /// Claude Code's binary there, once installed.
    pub(crate) claude: Option<PathBuf>,
}

impl AgentsSetup {
    /// Claude's setup, installed under `cache` (egit's cache folder).
    pub(crate) fn new(cache: Option<&Path>) -> Self {
        Self {
            install: cache.map(Install::at),
            there: None,
            homes: Homes::default(),
            found: Bind::new(true),
            installing: Bind::new(true),
            install_problem: None,
            cancel: CancelToken::new(),
            statuses: HashMap::new(),
            adding: Bind::new(true),
            form: AccountForm::default(),
            #[cfg(test)]
            stand_in: None,
        }
    }

    /// Claude's setup on the other computer `git` runs on: Node there, the SDK installed
    /// under its cache folder there; accounts' folders under `home` (this computer's home
    /// folder) are under its home folder there.
    pub(crate) fn there(git: Git, home: Option<&Path>) -> Self {
        Self {
            homes: Homes {
                here: home.map(Path::to_owned),
                there: git.home_there().map(Path::to_owned),
            },
            there: Some(git),
            ..Self::new(None)
        }
    }

    /// Where accounts' folders are on its computer.
    pub(crate) fn homes(&self) -> Homes {
        self.homes.clone()
    }

    /// The other computer it is on (as ssh reaches it), if it is.
    pub(crate) fn elsewhere(&self) -> Option<&str> {
        self.there
            .as_ref()
            .and_then(|git| git.host().ssh())
            .map(|ssh| ssh.destination.as_str())
    }

    /// Claude Code's binary, once installed (as found when Node and the SDK were looked for).
    pub(crate) fn claude(&mut self) -> Option<PathBuf> {
        match self.found.read_as_ref() {
            Some(Ok(found)) => found.claude.clone(),
            _ => None,
        }
    }

    /// Where the SDK is (or goes): here, known at once; there, once Node was looked for.
    pub(crate) fn installation(&mut self) -> Option<Install> {
        if self.there.is_none() {
            return self.install.clone();
        }
        match self.found.read_as_ref() {
            Some(Ok(found)) => found.install.clone(),
            _ => None,
        }
    }

    /// Takes in what its jobs found.
    pub(crate) fn poll(&mut self) {
        self.found.poll();
        self.installing.poll();
        self.adding.poll();
        for status in self.statuses.values_mut() {
            status.poll();
        }
        // Taken as it lands (not `just_completed`, whose frame every window in a process
        // shares): installed, Node and the SDK are looked for again; failed, said.
        match self.installing.take() {
            Some(Ok(())) => self.look_again(),
            Some(Err(problem)) => self.install_problem = Some(problem),
            None => {}
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
        let (install, there) = (self.install.clone(), self.there.clone());
        self.found.request(jobs::blocking(async move {
            let missing = |error: egit_agents::AgentError| {
                Problem::new("Node.js wasn't found", error.to_string())
            };
            let (node, install) = match there {
                None => (find_node().map_err(missing)?, install),
                // There: its Node, and the cache folder there.
                Some(git) => {
                    let node = find_node_there(&git).await.map_err(missing)?;
                    let install = Install::there(&git).await.map_err(|error| {
                        Problem::new("The other computer couldn't be asked", error.to_string())
                    })?;
                    (node, Some(install))
                }
            };
            let ready = match &install {
                Some(install) => install.is_ready().await,
                None => false,
            };
            let claude = match &install {
                Some(install) if ready => install.claude().await,
                _ => None,
            };
            Ok(Found {
                node,
                ready,
                install,
                claude,
            })
        }));
        self.statuses.clear();
    }

    /// Installs the SDK with the Node found.
    pub(crate) fn install(&mut self) {
        let (Some(install), Some(Ok(found))) = (self.installation(), self.found.read_as_ref())
        else {
            return;
        };
        let node = found.node.clone();
        self.install_problem = None;
        self.cancel = CancelToken::new();
        let cancel = self.cancel.clone();
        self.installing.request(jobs::blocking(async move {
            install
                .install(&node, &cancel)
                .await
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

    /// Who is signed in to Claude Code's configuration `folder` (`None`: its own), asked the
    /// first time (in a job), once it is installed.
    pub(crate) fn who(&mut self, folder: Option<&Path>) -> Option<Result<&AuthStatus, &Problem>> {
        if !self.is_ready() {
            return None;
        }
        let key = folder.map(Path::to_owned);
        if !self.statuses.contains_key(&key) {
            let mut status = Bind::new(true);
            let there = self.there.clone();
            match self.installation() {
                Some(install) => {
                    let signer =
                        folder.map_or(Signer::Default, |folder| Signer::Folder(folder.to_owned()));
                    status.request(jobs::blocking(async move {
                        // Finding its binary is a command there, too: in the job.
                        let claude = install.claude().await.ok_or_else(|| {
                            Problem::new("Claude Code isn't installed", String::new())
                        })?;
                        auth_status(&claude, &signer, there.as_ref())
                            .await
                            .map_err(|error| {
                                Problem::new(
                                    "Claude Code couldn't say who is signed in",
                                    error.to_string(),
                                )
                            })
                    }));
                }
                // Nothing to ask (a stand-in in the tests).
                None => status.fill(Err(Problem::new(
                    "Claude Code isn't installed",
                    String::new(),
                ))),
            }
            self.statuses.insert(key.clone(), status);
        }
        self.statuses.get_mut(&key).and_then(Bind::read_as_ref)
    }

    /// Asks who is signed in to `folder` again (after signing in).
    pub(crate) fn ask_who_again(&mut self, folder: Option<&Path>) {
        self.statuses.remove(&folder.map(Path::to_owned));
    }

    /// Adds `account` once stored: an API key handed to git's helpers (and found there again,
    /// or there is no helper keeping it), in a job.
    pub(crate) fn add_account(
        &mut self,
        git: Option<&Git>,
        account: AgentAccount,
        key: zeroize::Zeroizing<String>,
    ) {
        let git = git.cloned();
        self.adding.request(jobs::blocking(async move {
            if account.kind == AccountKind::ApiKey {
                let git = git.ok_or_else(|| Problem::new("The API key couldn't be kept", "git isn't ready."))?;
                let title = format!("The API key for {} couldn't be kept", account.name);
                store_credential(&git, None, account.place(), &key).await.map_err(|error| Problem::git(&title, &error))?;
                let kept = fill_credential(&git, None, account.place(), Asking::Never, &CancelToken::new())
                    .await.map_err(|error| Problem::git(&title, &error))?;
                if kept.is_none_or(|kept| kept.password() != key.as_str()) {
                    return Err(Problem::new(
                        title,
                        "No credential helper kept it: set one up (Profile, Credentials), then add it again.",
                    ));
                }
            }
            Ok(account)
        }));
    }

    /// Runs `launch` in place of the installed bridge, Node found and Claude ready (the tests:
    /// a stand-in SDK).
    #[cfg(test)]
    pub(crate) fn stand_in(&mut self, node: Node, launch: Launch) {
        self.stand_in = Some(launch);
        self.found.fill(Ok(Found {
            node,
            ready: true,
            install: None,
            claude: None,
        }));
    }

    /// How to run the bridge as `signer`, once it is installed.
    pub(crate) fn launch(&mut self, signer: &Signer) -> Option<Launch> {
        #[cfg(test)]
        if let Some(launch) = &self.stand_in {
            return Some(Launch {
                there: self.there.clone(),
                ..launch.clone()
            });
        }
        let install = self.installation()?;
        let Some(Ok(found)) = self.found.read_as_ref() else {
            return None;
        };
        // There, an API key goes in the start request (no command line there shows it); its
        // folder and the credentials left out are no secret.
        let signer = self.homes.place(signer.clone());
        let (env, unset) = signer.environment();
        let env = if self.there.is_some() && matches!(signer, Signer::ApiKey(_)) {
            Vec::new()
        } else {
            env
        };
        Some(Launch {
            node: found.node.node.clone(),
            bridge: install.bridge(),
            env,
            unset,
            there: self.there.clone(),
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
    /// The account new conversations run as, whose past ones are listed (`None`: Claude
    /// Code's own).
    pub(crate) account: Option<AgentAccount>,
    /// Wakes the window when a bridge says something.
    waker: crate::repaint::Repaint,
}

impl RepoAgents {
    pub(crate) fn new(waker: &crate::repaint::Repaint) -> Self {
        Self {
            shown: false,
            conversations: vec![Conversation::new(None)],
            current: 0,
            past: Bind::new(true),
            account: None,
            waker: waker.clone(),
        }
    }

    /// Runs new conversations as `account` (and the one shown, when it hasn't started), and
    /// lists its past ones.
    pub(crate) fn choose_account(&mut self, account: Option<AgentAccount>) {
        if let Some(conversation) = self.conversations.get_mut(self.current)
            && !conversation.has_begun()
            && conversation.account != account
        {
            // Started ahead as another: started again as this one.
            conversation.stop();
            conversation.account.clone_from(&account);
        }
        self.account = account;
        self.past = Bind::new(true);
    }

    /// Who past conversations are listed as, and read as.
    pub(crate) fn folder_signer(&self) -> Signer {
        self.account
            .as_ref()
            .map_or(Signer::Default, AgentAccount::folder_signer)
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

    /// What its conversations want of the user.
    pub(crate) fn attention(&self) -> Attention {
        self.conversations
            .iter()
            .fold(Attention::default(), |attention, conversation| Attention {
                waiting: attention.waiting.saturating_add(conversation.waiting.len()),
                ended: attention.ended || conversation.ended_unseen.is_some(),
            })
    }

    /// Lists the past conversations again.
    pub(crate) fn list_past(&mut self, launch: Option<Launch>, work_tree: &Path) {
        let Some(launch) = launch else {
            return;
        };
        let dir = work_tree.to_owned();
        self.past.request(jobs::blocking(async move {
            list_sessions(&launch, &dir, PAST)
                .await
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
        } else {
            self.conversations.push(Conversation::new(None));
            self.current = self.conversations.len().saturating_sub(1);
        }
        let account = self.account.clone();
        if let Some(conversation) = self.current() {
            conversation.account = account;
        }
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
        conversation.account.clone_from(&self.account);
        if let Some(launch) = launch {
            let dir = work_tree.to_owned();
            let id = session.session_id.clone();
            conversation.history.request(jobs::blocking(async move {
                session_history(&launch, &dir, &id)
                    .await
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

    /// Sends `prompt` (and `images`) to the conversation shown, starting its bridge first if it
    /// isn't running.
    pub(crate) fn send(
        &mut self,
        starter: Option<Starter>,
        work_tree: &Path,
        (prompt, images): (Prompt, Vec<Image>),
    ) {
        let waker = self.waker.clone();
        if let Some(conversation) = self.current() {
            conversation.send(starter, work_tree, (prompt, images), waker);
        }
    }

    /// Starts the conversation shown ahead of its first prompt, once: what it offers (the
    /// models, the slash commands) and who it runs as are known before anything is sent, and
    /// the first answer comes sooner. Starting asks Claude Code nothing of Anthropic.
    pub(crate) fn warm(&mut self, starter: Option<Starter>, work_tree: &Path) {
        let waker = self.waker.clone();
        if let Some(conversation) = self.current()
            && !conversation.warmed
            && !conversation.is_running()
            && !conversation.history.is_pending()
            && let Some(starter) = starter
        {
            conversation.warmed = true;
            conversation.start(starter, work_tree, waker);
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
    /// The account it runs as (`None`: Claude Code's own); chosen before it starts.
    pub(crate) account: Option<AgentAccount>,
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
    /// Whether a prompt was sent: its account is then settled.
    prompted: bool,
    /// Whether its bridge was started ahead of its first prompt (once).
    warmed: bool,
    /// When the turn under way began.
    turn_began: Option<web_time::Instant>,
    /// What its view keeps (the draft, what is folded open, heights).
    pub(crate) view: crate::views::agents::ConversationView,
    /// When a turn ended, while it hasn't been shown since.
    ended_unseen: Option<web_time::Instant>,
}

/// What a tab's agents want of the user: answers they wait for, and turns that ended unseen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Attention {
    /// Questions and permissions waiting for an answer, in every conversation.
    pub(crate) waiting: usize,
    /// Whether a turn ended in a conversation not shown since.
    pub(crate) ended: bool,
}

impl Attention {
    /// Whether they want anything.
    pub(crate) const fn any(self) -> bool {
        self.waiting > 0 || self.ended
    }

    /// What they want, in words.
    pub(crate) fn words(self) -> String {
        match self.waiting {
            0 if self.ended => "Claude finished: see what it did".to_owned(),
            0 => String::new(),
            1 => "Claude is waiting for your answer".to_owned(),
            waiting => format!("Claude is waiting for {waiting} answers"),
        }
    }
}

/// A bridge started in a job.
struct Started {
    bridge: Bridge,
    events: Receiver<Event>,
    /// Its account's variables, for the start request where it runs on another computer.
    secrets: Secrets,
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
            account: None,
            resume,
            history: Bind::new(true),
            starting: Bind::new(true),
            bridge: None,
            events: None,
            ready: false,
            queued: Vec::new(),
            prompted: false,
            warmed: false,
            turn_began: None,
            view: crate::views::agents::ConversationView::default(),
            ended_unseen: None,
        }
    }

    /// Notes that it was shown: a turn that ended is seen.
    pub(crate) const fn seen(&mut self) {
        self.ended_unseen = None;
    }

    /// Whether nothing has happened in it yet (it may have started, ahead of a prompt).
    pub(crate) fn is_blank(&self) -> bool {
        !self.prompted && self.resume.is_none() && self.transcript.items().is_empty()
    }

    /// Whether it has begun: a prompt sent, or an earlier conversation gone on with. Its
    /// account is settled from then.
    pub(crate) const fn has_begun(&self) -> bool {
        self.prompted || self.resume.is_some()
    }

    /// How long the turn under way has run.
    pub(crate) fn turn_time(&self) -> Option<std::time::Duration> {
        self.turn_began
            .filter(|_| self.transcript.busy())
            .map(|began| began.elapsed())
    }

    /// Whether its bridge is starting (and not ready yet).
    pub(crate) fn is_starting(&mut self) -> bool {
        self.starting.is_pending() || (self.bridge.is_some() && !self.ready)
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
    fn send(
        &mut self,
        starter: Option<Starter>,
        work_tree: &Path,
        (prompt, images): (Prompt, Vec<Image>),
        waker: crate::repaint::Repaint,
    ) {
        if self.title.is_empty() {
            prompt
                .text
                .lines()
                .next()
                .unwrap_or_default()
                .clone_into(&mut self.title);
        }
        let request = Request::Send {
            text: prompt.text.clone(),
            images,
        };
        self.transcript.prompt(prompt);
        self.prompted = true;
        self.turn_began = Some(web_time::Instant::now());
        self.view.follow();
        if self.is_running() {
            self.deliver(request);
            return;
        }
        let Some(starter) = starter else {
            self.transcript
                .notice(Level::Error, "Claude isn't set up on this computer.");
            self.transcript.ended();
            return;
        };
        self.start(starter, work_tree, waker);
        self.queued.push(request);
    }

    /// Starts its bridge (in a job) as its account, going on with its session if it has one;
    /// requests wait until it is ready.
    fn start(&mut self, starter: Starter, work_tree: &Path, waker: crate::repaint::Repaint) {
        let Starter {
            mut launch,
            git,
            homes,
        } = starter;
        self.queued.insert(
            0,
            Request::Start {
                cwd: work_tree.to_string_lossy().into_owned(),
                resume: self
                    .transcript
                    .session()
                    .map(str::to_owned)
                    .or_else(|| self.resume.clone()),
                model: self.model.clone(),
                mode: self.mode,
                // Filled in as it starts, where it runs on another computer.
                env: Secrets::default(),
            },
        );
        let account = self.account.clone();
        self.starting.request(jobs::blocking(async move {
            // As its account: a folder's login (there, the same folder under the home folder
            // there), or its key from git's helpers.
            let signer = match account {
                Some(account) => account.signer(git.as_ref()).await?,
                None => Signer::Default,
            };
            let signer = homes.place(signer);
            let (env, unset) = signer.environment();
            launch.unset.extend(unset);
            let secrets = if launch.there.is_some() && matches!(signer, Signer::ApiKey(_)) {
                // Sent in the start request: no command line there shows it.
                Secrets(
                    env.into_iter()
                        .map(|(key, value)| {
                            (
                                key.to_string_lossy().into_owned(),
                                zeroize::Zeroizing::new(value.to_string_lossy().into_owned()),
                            )
                        })
                        .collect(),
                )
            } else {
                launch.env.extend(env);
                Secrets::default()
            };
            let (bridge, events) = Bridge::start(&launch, move || waker.request_repaint())
                .map_err(|error| Problem::new("Claude couldn't be started", error.to_string()))?;
            Ok(Started {
                bridge,
                events,
                secrets,
            })
        }));
    }

    /// Stops its bridge (started ahead, as another account), forgetting what it said of itself;
    /// it starts again as it is asked.
    fn stop(&mut self) {
        self.starting = Bind::new(true);
        self.bridge = None;
        self.events = None;
        self.ready = false;
        self.queued.clear();
        self.waiting.clear();
        self.info = None;
        self.warmed = false;
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
        self.view.poll();
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
                if let Some(Request::Start { env, .. }) = self.queued.first_mut() {
                    *env = started.secrets;
                }
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
            .map_or_default(|events| events.try_iter().collect());
        let busy = self.transcript.busy();
        for event in events {
            self.event(event);
        }
        if busy && !self.transcript.busy() {
            self.ended_unseen = Some(web_time::Instant::now());
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
                self.transcript.message(&message);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accounts_folder_is_the_same_under_the_home_folder_there() {
        let homes = Homes {
            here: Some(PathBuf::from("/home/me")),
            there: Some(PathBuf::from("/srv/users/me")),
        };
        assert_eq!(
            homes.folder(Path::new("/home/me/.claude-work")),
            PathBuf::from("/srv/users/me/.claude-work")
        );
        // Outside the home folder: the same path there.
        assert_eq!(
            homes.folder(Path::new("/opt/claude/work")),
            PathBuf::from("/opt/claude/work")
        );
        assert_eq!(
            homes.place(Signer::Folder(PathBuf::from("/home/me/.claude-x"))),
            Signer::Folder(PathBuf::from("/srv/users/me/.claude-x"))
        );
        assert_eq!(homes.place(Signer::Default), Signer::Default);
        // Here, every folder is where it is.
        let here = Homes {
            here: Some(PathBuf::from("/home/me")),
            there: None,
        };
        assert_eq!(
            here.folder(Path::new("/home/me/.claude-work")),
            PathBuf::from("/home/me/.claude-work")
        );
    }
}
