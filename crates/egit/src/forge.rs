//! A repository's forge: which of its remotes are GitHub repositories or GitLab projects (on
//! github.com and gitlab.com, or on hosts the configuration lists as `egit.githubHost` and
//! `egit.gitlabHost`), an account per host signed in with git's credentials, and their open pull
//! requests (GitHub's) and merge requests (GitLab's), read in the background (a page at a time,
//! shown as each lands) and read again when the remotes may have moved; the badges they put on
//! their head commits; the CI/CD pipelines (GitLab's, and GitHub Actions' workflow runs), a
//! mark on each commit that ran one (`ci.rs`); and how each forge names them.
//!
//! A read again takes over only once complete, so the list never empties or jumps meanwhile.

use std::path::PathBuf;
use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_forge::{
    Account, CheckState, ForgeError, ForgeHost, ForgeHosts, ForgeKind, ForgeRepository,
    PullRequest, forge_repository, pull_requests, resolve_ssh_alias_on,
};
use egit_git::{CancelToken, ConfigSnapshot, Git, ObjectId, Repository, remote_urls};
use egui_async::Bind;
use parking_lot::{Mutex, MutexGuard};

use crate::ci::{CiMark, Pipelines};
use crate::downloads::{ArtifactWanted, Downloads};
use crate::jobs;
use crate::problem::Problem;

/// The keys listing GitHub Enterprise and self-managed GitLab hosts (one value each).
pub(crate) const GITHUB_HOSTS_KEY: &str = "egit.githubHost";
pub(crate) const GITLAB_HOSTS_KEY: &str = "egit.gitlabHost";

/// The key naming a listed host's ssh host, where its ssh isn't on its own name (several
/// values): `egit.<host>.sshHost`, the host as listed.
pub(crate) fn ssh_host_key(listed: &str) -> String {
    format!("egit.{listed}.sshHost")
}

/// Each listed host's ssh hosts the configuration names (`egit.<host>.sshHost`): the host as
/// written, and the ssh host.
pub(crate) fn ssh_hosts(config: &ConfigSnapshot) -> Vec<(String, String)> {
    ssh_host_entries(config)
        .map(|(listed, ssh, _)| (listed, ssh))
        .collect()
}

/// The same, with each one's entry (where it is set).
pub(crate) fn ssh_host_entries(
    config: &ConfigSnapshot,
) -> impl Iterator<Item = (String, String, egit_git::ConfigEntry<'_>)> {
    config.entries().filter_map(|entry| {
        // git writes the section and the name in lowercase, the host as it was given.
        let listed = entry
            .key
            .strip_prefix(b"egit.")?
            .strip_suffix(b".sshhost")?
            .to_str()
            .ok()?;
        let ssh = entry.text().to_str().ok()?;
        Some((listed.to_owned(), ssh.to_owned(), entry))
    })
}

/// What `kind` calls what it merges: "pull request" or "merge request".
pub(crate) const fn request_noun(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "pull request",
        ForgeKind::GitLab => "merge request",
    }
}

/// How `kind` writes a request's number: GitHub's `#12`, GitLab's `!12`.
pub(crate) fn request_number(kind: ForgeKind, number: u32) -> String {
    match kind {
        ForgeKind::GitHub => format!("#{number}"),
        ForgeKind::GitLab => format!("!{number}"),
    }
}

/// Where a fork's request `number` is fetched from on its base repository, and the branch it
/// is fetched into: GitHub's `refs/pull/12/head` into `pr/12`, GitLab's
/// `refs/merge-requests/12/head` into `mr/12`.
pub(crate) fn fork_head(kind: ForgeKind, number: u32) -> (String, String) {
    match kind {
        ForgeKind::GitHub => (format!("refs/pull/{number}/head"), format!("pr/{number}")),
        ForgeKind::GitLab => (
            format!("refs/merge-requests/{number}/head"),
            format!("mr/{number}"),
        ),
    }
}

/// The most open pull requests read for each repository: the most recently updated.
pub(crate) const LIMIT: u32 = 1_000;

/// A remote that is a forge's repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForgeRemote {
    pub(crate) remote: BString,
    pub(crate) repository: ForgeRepository,
}

/// What a run shares with every repository's forge: the sign-ins made, and the names listed
/// GitLab hosts' ssh is on, as they said.
#[derive(Clone, Debug, Default)]
pub(crate) struct ForgeRun {
    pub(crate) session: crate::sign_ins::SessionSignIns,
    pub(crate) learning: SshLearning,
}

/// The names the listed GitLab hosts' ssh is on, learned this run by asking each once (its
/// projects' ssh addresses give it: its `gitlab_shell_ssh_host`, which a self-managed
/// GitLab's often is), so remotes on those names are taken for its projects unasked.
#[derive(Clone, Debug, Default)]
pub(crate) struct SshLearning(Arc<Mutex<Learning>>);

#[derive(Debug, Default)]
struct Learning {
    /// Hosts to ask, not asked yet.
    wanted: Vec<ForgeHost>,
    /// The names of those asked, or being.
    asked: Vec<String>,
    /// What each said: its name, and the name its ssh is on.
    learned: Vec<(String, String)>,
    /// Bumped with each learned.
    generation: u64,
}

impl SshLearning {
    /// Asks for the self-managed GitLab hosts among `hosts` to be asked, those not asked yet.
    pub(crate) fn want<'a>(&self, hosts: impl IntoIterator<Item = &'a ForgeHost>) {
        let mut learning = self.0.lock();
        for host in hosts {
            let gitlab = host.kind() == ForgeKind::GitLab && !host.is_public();
            if gitlab && !learning.asked.iter().any(|asked| asked == host.name()) {
                learning.asked.push(host.name().to_owned());
                learning.wanted.push(host.clone());
            }
        }
    }

    /// Asks `host` again unless it said already (signed in to it since, or asked to read
    /// again: it may say now).
    pub(crate) fn ask_again(&self, host: &ForgeHost) {
        {
            let mut learning = self.0.lock();
            if learning
                .learned
                .iter()
                .any(|(learned, _)| learned == host.name())
            {
                return;
            }
            learning.asked.retain(|asked| asked != host.name());
        }
        self.want([host]);
    }

    /// The hosts to ask now.
    pub(crate) fn take_wanted(&self) -> Vec<ForgeHost> {
        std::mem::take(&mut self.0.lock().wanted)
    }

    /// Notes that `host`'s ssh is on `ssh`.
    pub(crate) fn learn(&self, host: &ForgeHost, ssh: String) {
        let mut learning = self.0.lock();
        let pair = (host.name().to_owned(), ssh);
        if !learning.learned.contains(&pair) {
            learning.learned.push(pair);
            learning.generation = learning.generation.wrapping_add(1);
        }
    }

    /// What was learned: each host's name, and the name its ssh is on; and how much has been.
    pub(crate) fn learned(&self) -> (Vec<(String, String)>, u64) {
        let learning = self.0.lock();
        (learning.learned.clone(), learning.generation)
    }

    /// How much has been learned (bumped with each).
    pub(crate) fn generation(&self) -> u64 {
        self.0.lock().generation
    }
}

/// Asks `host`, through `git`'s credential helpers or the sign-in made this run, the name its
/// ssh is on, and notes it in `learning` (where it isn't the host's own); wakes `waker`.
pub(crate) fn learn_ssh_host(git: Git, host: ForgeHost, run: &ForgeRun, waker: egui::Context) {
    let learning = run.learning.clone();
    let signed = run
        .session
        .lock()
        .get(host.name())
        .map(|(_, signed)| signed.clone());
    jobs::detached(move || {
        let account = Account::new(git, None, host.clone());
        if let Some(signed) = &signed {
            account.adopt(signed);
        }
        match egit_forge::ssh_host(&account, &CancelToken::new()) {
            Ok(Some(ssh)) => {
                tracing::debug!(host = host.name(), ssh, "the name a GitLab's ssh is on");
                if !ssh.eq_ignore_ascii_case(host.hostname()) {
                    learning.learn(&host, ssh);
                    waker.request_repaint();
                }
            }
            Ok(None) => tracing::debug!(
                host = host.name(),
                "the GitLab shows no project to say where its ssh is"
            ),
            Err(error) => tracing::debug!(
                host = host.name(),
                %error,
                "the GitLab couldn't say where its ssh is"
            ),
        }
    });
}

/// A remote, and what egit took it for: its address as git reaches it, as the configuration
/// writes it where that differs (`insteadOf`), and the forge's repository it is, if any (`None`:
/// not on a host egit knows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SeenRemote {
    pub(crate) remote: BString,
    pub(crate) url: BString,
    pub(crate) written: Option<BString>,
    pub(crate) repository: Option<ForgeRepository>,
}

impl SeenRemote {
    /// The host it reaches over ssh, as either of its addresses names it.
    pub(crate) fn ssh_host(&self) -> Option<String> {
        [Some(&self.url), self.written.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|url| egit_forge::ssh_host_of(url.to_str().ok()?))
    }
}

/// The remotes as worked out: every one as seen, and the hosts egit knew then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) seen: Arc<[SeenRemote]>,
    pub(crate) hosts: Arc<[ForgeHost]>,
}

/// A pull request, and which of the forge's repositories it is in.
#[derive(Clone, Debug)]
pub(crate) struct Listed {
    pub(crate) repository: usize,
    pub(crate) pull: PullRequest,
}

/// Why pull requests couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ForgeProblem {
    /// Signing in to this host would help: there is no credential, or it was refused.
    SignIn {
        host: String,
        refused: bool,
    },
    /// The host's rate limit is used up, until `reset` (seconds since the epoch) if known.
    RateLimited {
        host: String,
        reset: Option<i64>,
    },
    Other(Problem),
}

impl ForgeProblem {
    /// What went wrong reading pull requests.
    pub(crate) fn of(error: &ForgeError) -> Self {
        match error {
            ForgeError::NotSignedIn { host } => Self::SignIn {
                host: host.clone(),
                refused: false,
            },
            ForgeError::Refused { host } => Self::SignIn {
                host: host.clone(),
                refused: true,
            },
            ForgeError::RateLimited { host, reset } => Self::RateLimited {
                host: host.clone(),
                reset: *reset,
            },
            other => Self::Other(Problem::new(
                "The requests couldn't be read",
                other.to_string(),
            )),
        }
    }
}

/// What a read of the pull requests found so far.
#[derive(Debug, Default)]
pub(crate) struct Found {
    /// The pull requests, each repository's most recently updated first.
    pub(crate) pulls: Vec<Listed>,
    /// How many are open in each repository, once its first page landed.
    pub(crate) totals: Vec<Option<u32>>,
    pub(crate) done: bool,
    pub(crate) problem: Option<ForgeProblem>,
}

/// A head commit's pull request, as its badge shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Badge {
    /// Its repository's place in the forge's.
    pub(crate) repository: usize,
    pub(crate) number: u32,
    pub(crate) checks: CheckState,
}

/// A repository's forge.
#[derive(Debug)]
pub(crate) struct RepoForge {
    /// The remotes, being worked out.
    resolving: Bind<Resolved, Problem>,
    /// Every remote as last worked out, and the hosts known then (none until they are).
    resolved: Option<Resolved>,
    /// Why the remotes couldn't be worked out, last time.
    resolve_problem: Option<Problem>,
    /// The remotes that are forges' repositories, as last worked out.
    pub(crate) remotes: Arc<[ForgeRemote]>,
    /// The distinct repositories among them, in the remotes' order.
    pub(crate) repositories: Arc<[ForgeRepository]>,
    /// A sign-in per host.
    accounts: Vec<Arc<Account>>,
    /// The pull requests shown: the first read as it goes, then each complete read.
    shown: Arc<Mutex<Found>>,
    /// A read again, under way.
    reading: Option<Arc<Mutex<Found>>>,
    /// Whether to read again once the read under way ends.
    again: bool,
    cancel: CancelToken,
    /// The head commits' badges, by commit, and what they were made from (how many pull
    /// requests of which read).
    badges: Vec<(ObjectId, Badge)>,
    indexed: (u64, usize),
    /// Bumped whenever another read takes over.
    generation: u64,
    /// Wakes the window as pages land.
    waker: egui::Context,
    /// The pipelines (GitLab's, GitHub's workflow runs): each commit's.
    ci: Pipelines,
    /// Their artifacts downloaded, or being downloaded.
    pub(crate) downloads: Downloads,
    /// What the run shares: the sign-ins made, which its accounts take as they are made; and
    /// the names listed GitLab hosts' ssh is on.
    run: ForgeRun,
    /// How much of `run`'s ssh hosts its remotes were last worked out with.
    learned_at: u64,
    /// For a repository on another computer, this computer's git: it is this computer that
    /// reaches the forge, so its helpers keep the sign-ins, under the user's configuration
    /// here (and its prompts are relayed to the window, which those there aren't).
    here: Option<Git>,
}

impl RepoForge {
    /// A repository's forge, nothing known yet; `here` is this computer's git for a repository
    /// on another computer; `run`, what the run shares (sign-ins, ssh hosts learned).
    pub(crate) fn new(waker: &egui::Context, here: Option<Git>, run: ForgeRun) -> Self {
        Self {
            resolving: Bind::new(true),
            resolved: None,
            resolve_problem: None,
            remotes: Arc::from([]),
            repositories: Arc::from([]),
            accounts: Vec::new(),
            shown: Arc::default(),
            reading: None,
            again: false,
            cancel: CancelToken::new(),
            badges: Vec::new(),
            indexed: (0, 0),
            generation: 0,
            waker: waker.clone(),
            ci: Pipelines::new(waker),
            downloads: Downloads::default(),
            run,
            learned_at: 0,
            here,
        }
    }

    /// Works out (again) which remotes are forges' repositories, from the repository's
    /// configuration as just read: the hosts it lists, and its remotes' URLs, as git rewrites
    /// them (else as written).
    pub(crate) fn configured(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        config: &ConfigSnapshot,
    ) {
        let listed = |key| {
            config
                .get_all(key)
                .filter_map(|entry| entry.text().to_str().ok())
        };
        let hosts = ForgeHosts::with_listed(listed(GITHUB_HOSTS_KEY), listed(GITLAB_HOSTS_KEY));
        // The listed GitLab hosts are asked where their ssh is (once a run); what they said, and what
        // the configuration says, names their ssh hosts.
        self.run.learning.want(hosts.hosts());
        let (learned, generation) = self.run.learning.learned();
        self.learned_at = generation;
        let ssh = ssh_hosts(config);
        let hosts = hosts.with_ssh_hosts(
            ssh.iter()
                .chain(&learned)
                .map(|(listed, ssh)| (listed.as_str(), ssh.as_str())),
        );
        let written = written_urls(config);
        let (git, repository) = (git.clone(), Arc::clone(repository));
        self.resolving.request(jobs::blocking(move || {
            let urls = remote_urls(&git, &repository)
                .map_err(|error| Problem::git("The remotes couldn't be read", &error))?;
            // An ssh alias as the computer the repository is on resolves it.
            let alias = |alias: &str| resolve_ssh_alias_on(&git, alias);
            let seen: Arc<[SeenRemote]> = urls
                .into_iter()
                .map(|(remote, url)| {
                    let resolved = url
                        .to_str()
                        .ok()
                        .and_then(|url| forge_repository(url, &hosts, alias));
                    let written = written
                        .iter()
                        .find(|(name, _)| *name == remote)
                        .map(|(_, written)| written.clone())
                        .filter(|written| *written != url);
                    let repository = resolved.or_else(|| {
                        forge_repository(written.as_ref()?.to_str().ok()?, &hosts, alias)
                    });
                    if let Some(found) = &repository {
                        tracing::debug!(
                            remote = %remote,
                            url = %url,
                            repository = %found.full_name(),
                            host = found.host().name(),
                            "a remote on a forge"
                        );
                    } else {
                        tracing::debug!(
                            remote = %remote,
                            url = %url,
                            written = ?written,
                            "a remote on no host egit knows (an ssh host on a name of its own is \
                             named in egit.<host>.sshHost)"
                        );
                    }
                    SeenRemote {
                        remote,
                        url,
                        written,
                        repository,
                    }
                })
                .collect();
            Ok(Resolved {
                seen,
                hosts: hosts.hosts().into(),
            })
        }));
    }

    /// Takes in what its jobs finished: remotes worked out (pull requests read when they
    /// changed), a read again complete, a sign-in; and keeps the badges current.
    pub(crate) fn poll(&mut self, git: &Git, repository: &Arc<Repository>) {
        if self.resolving.is_finished() {
            match self.resolving.take() {
                Some(Ok(resolved)) => {
                    let remotes: Arc<[ForgeRemote]> = resolved
                        .seen
                        .iter()
                        .filter_map(|seen| {
                            Some(ForgeRemote {
                                remote: seen.remote.clone(),
                                repository: seen.repository.clone()?,
                            })
                        })
                        .collect();
                    // The first time even when none is a forge's: nothing is being read then.
                    let first = self.resolved.is_none();
                    self.resolved = Some(resolved);
                    self.resolve_problem = None;
                    if first || remotes != self.remotes {
                        self.set_remotes(git, repository, remotes);
                    }
                }
                Some(Err(problem)) => {
                    tracing::warn!(%problem, "the remotes couldn't be worked out");
                    self.resolve_problem = Some(problem);
                }
                None => {}
            }
        }
        let finished = self
            .reading
            .as_ref()
            .is_some_and(|reading| reading.lock().done);
        if finished && let Some(reading) = self.reading.take() {
            let failed_outright = {
                let read = reading.lock();
                read.problem.is_some() && read.pulls.is_empty()
            };
            if failed_outright {
                // Keep what is shown, saying why it couldn't be read again.
                let problem = reading.lock().problem.take();
                self.shown.lock().problem = problem;
            } else {
                self.shown = reading;
                self.generation = self.generation.wrapping_add(1);
            }
        }
        if self.again && !self.is_reading() {
            self.again = false;
            self.refresh();
        }
        self.index();
        self.ci.poll(&self.repositories, &self.accounts);
    }

    /// Reads the pull requests and the pipelines again (after the reads under way, if any).
    pub(crate) fn refresh(&mut self) {
        if self.repositories.is_empty() {
            return;
        }
        self.ci.refresh(&self.repositories, &self.accounts);
        let first_under_way = !self.shown.lock().done && self.generation == 0;
        if self.reading.is_some() || first_under_way {
            self.again = true;
            return;
        }
        let reading = Arc::new(Mutex::new(Found::default()));
        self.start_read(Arc::clone(&reading));
        self.reading = Some(reading);
    }

    /// Takes a sign-in to `host` made elsewhere (the sign-in dialog, through another account),
    /// and reads the requests and pipelines again with it.
    pub(crate) fn signed_in(&mut self, host: &ForgeHost, signed: &egit_forge::SignedIn) {
        if let Some(account) = self.account(host) {
            account.adopt(signed);
            self.refresh();
        }
    }

    /// Forgets the sign-in to `host` (signed out elsewhere), and reads the requests again: none
    /// can be, which says signing in would help.
    pub(crate) fn signed_out(&mut self, host: &ForgeHost) {
        if let Some(account) = self.account(host) {
            account.signed_out();
            self.refresh();
        }
    }

    /// The host signing in to would help (nobody is, or the host refused the credentials), and
    /// whether it refused; none while a read is under way or the requests could be read.
    pub(crate) fn needs_sign_in(&self) -> Option<(ForgeHost, bool)> {
        let found = self.shown.lock();
        let Some(ForgeProblem::SignIn { host, refused }) = &found.problem else {
            return None;
        };
        let refused = *refused;
        let host = self
            .accounts
            .iter()
            .map(|account| account.host())
            .find(|known| known.name() == host)?
            .clone();
        drop(found);
        Some((host, refused))
    }

    /// The pull requests shown.
    pub(crate) fn found(&self) -> MutexGuard<'_, Found> {
        self.shown.lock()
    }

    /// Which pull requests are shown: another read's, or more of one, is another pair.
    pub(crate) fn listed(&self) -> (u64, usize) {
        (self.generation, self.shown.lock().pulls.len())
    }

    /// Whether a read is under way.
    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some() || !self.shown.lock().done
    }

    /// Downloads `wanted` into `to` (where the user said), through the account for its
    /// repository.
    pub(crate) fn download(&mut self, wanted: ArtifactWanted, to: PathBuf) {
        let reach = self
            .repositories
            .get(wanted.repository)
            .cloned()
            .and_then(|repository| Some((self.account(repository.host())?, repository)));
        self.downloads.start(reach, wanted, to, &self.waker);
    }

    /// `commit`'s pipelines, if it ran any: the most telling shown, the others beside it.
    pub(crate) fn pipeline_of(&self, commit: &ObjectId) -> Option<&CiMark> {
        self.ci.of(commit)
    }

    /// Reads the newest pipelines again at the next poll (one retried or cancelled; tests: not
    /// waiting for it).
    pub(crate) fn read_pipelines_now(&mut self) {
        self.ci.read_newest_now();
    }

    /// Why the pipelines couldn't be read, when the last read failed.
    pub(crate) const fn pipelines_problem(&self) -> Option<&Problem> {
        self.ci.problem()
    }

    /// Whether any commit ran a pipeline (the commit table keeps room for a mark on each row).
    pub(crate) fn has_pipelines(&self) -> bool {
        self.ci.any()
    }

    /// The badges of pull requests whose head is `commit`.
    pub(crate) fn badges_at(&self, commit: &ObjectId) -> &[(ObjectId, Badge)] {
        let start = self.badges.partition_point(|(id, _)| id < commit);
        let end = start.saturating_add(
            self.badges
                .get(start..)
                .unwrap_or_default()
                .iter()
                .take_while(|(id, _)| id == commit)
                .count(),
        );
        self.badges.get(start..end).unwrap_or_default()
    }

    /// The open pull request bringing in `branch` of `head` (a repository, maybe a fork), if
    /// one is listed: its repository's place and its number.
    pub(crate) fn pull_request_for(
        &self,
        head: &ForgeRepository,
        branch: &str,
    ) -> Option<(usize, u32)> {
        let found = self.shown.lock();
        found
            .pulls
            .iter()
            .find(|listed| {
                let pull = &listed.pull;
                pull.head_branch == branch
                    && if pull.cross_repository {
                        pull.head_repository
                            .as_deref()
                            .is_some_and(|fork| head.is_named(fork))
                    } else {
                        self.repositories
                            .get(listed.repository)
                            .is_some_and(|repository| repository.is(head))
                    }
            })
            .map(|listed| (listed.repository, listed.pull.number))
    }

    /// The repository pull request `selected` (a repository's place, a number) is in, and its
    /// title.
    pub(crate) fn title_of(&self, selected: (usize, u32)) -> Option<String> {
        let found = self.shown.lock();
        found
            .pulls
            .iter()
            .find(|listed| (listed.repository, listed.pull.number) == selected)
            .map(|listed| listed.pull.title.clone())
    }

    /// Which forge the repositories are on: the first one's (GitHub's, before any is known).
    pub(crate) fn kind(&self) -> ForgeKind {
        self.repositories
            .first()
            .map_or(ForgeKind::GitHub, |repository| repository.host().kind())
    }

    /// What its repositories' forges call what they merge, in the plural: "pull requests",
    /// "merge requests", or both when the remotes are on each.
    pub(crate) fn nouns(&self) -> &'static str {
        let on = |kind| {
            self.repositories
                .iter()
                .any(|repository| repository.host().kind() == kind)
        };
        match (on(ForgeKind::GitHub), on(ForgeKind::GitLab)) {
            (true, true) => "pull and merge requests",
            (false, true) => "merge requests",
            _ => "pull requests",
        }
    }

    /// Which forge repository `index` is on.
    pub(crate) fn kind_of(&self, index: usize) -> ForgeKind {
        self.repositories
            .get(index)
            .map_or_else(|| self.kind(), |repository| repository.host().kind())
    }

    /// The sign-ins, a host each.
    pub(crate) fn accounts(&self) -> &[Arc<Account>] {
        &self.accounts
    }

    /// The sign-in for `host`, if one of the remotes is there.
    pub(crate) fn account(&self, host: &ForgeHost) -> Option<Arc<Account>> {
        self.accounts
            .iter()
            .find(|account| account.host() == host)
            .cloned()
    }

    /// Whether more ssh hosts were learned since its remotes were worked out.
    pub(crate) fn learned_more(&self) -> bool {
        self.run.learning.generation() != self.learned_at
    }

    /// Every remote as last worked out, and the hosts egit knew then (none until they are).
    pub(crate) const fn resolved(&self) -> Option<&Resolved> {
        self.resolved.as_ref()
    }

    /// Why the remotes couldn't be worked out, last time.
    pub(crate) const fn resolve_problem(&self) -> Option<&Problem> {
        self.resolve_problem.as_ref()
    }

    /// Where `repository` is among the forge's.
    pub(crate) fn index_of(&self, repository: &ForgeRepository) -> Option<usize> {
        self.repositories
            .iter()
            .position(|known| known.is(repository))
    }

    /// The pipelines: how each project went, the read under way, how many of their commits a
    /// history shows.
    pub(crate) const fn pipelines(&mut self) -> &mut Pipelines {
        &mut self.ci
    }

    /// The first remote that is repository `index`.
    pub(crate) fn remote_of(&self, index: usize) -> Option<&BStr> {
        let repository = self.repositories.get(index)?;
        self.remotes
            .iter()
            .find(|remote| remote.repository.is(repository))
            .map(|remote| remote.remote.as_bstr())
    }

    /// The repository a remote is, if it is on a forge.
    pub(crate) fn repository_of(&self, remote: &BStr) -> Option<&ForgeRepository> {
        self.remotes
            .iter()
            .find(|known| known.remote == remote)
            .map(|known| &known.repository)
    }

    /// Takes in a new set of remotes: an account per host (kept for a host still there), and
    /// the pull requests read again from the start.
    fn set_remotes(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        remotes: Arc<[ForgeRemote]>,
    ) {
        let mut repositories: Vec<ForgeRepository> = Vec::new();
        for remote in remotes.iter() {
            if !repositories
                .iter()
                .any(|known| known.is(&remote.repository))
            {
                repositories.push(remote.repository.clone());
            }
        }
        let mut accounts = Vec::new();
        for repository_at in &repositories {
            let host = repository_at.host();
            if accounts
                .iter()
                .any(|account: &Arc<Account>| account.host() == host)
            {
                continue;
            }
            let account = self.account(host).unwrap_or_else(|| {
                let account = Arc::new(self.here.as_ref().map_or_else(
                    || Account::new(git.clone(), Some(Arc::clone(repository)), host.clone()),
                    |here| Account::new(here.clone(), None, host.clone()),
                ));
                // Signed in this run already: whether or not a helper keeps it.
                if let Some((_, signed)) = self.run.session.lock().get(host.name()) {
                    account.adopt(signed);
                }
                account
            });
            accounts.push(account);
        }
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.remotes = remotes;
        self.repositories = repositories.into();
        self.accounts = accounts;
        self.reading = None;
        self.again = false;
        self.shown = Arc::default();
        self.generation = 0;
        self.badges.clear();
        self.indexed = (0, 0);
        if self.repositories.is_empty() {
            self.shown.lock().done = true;
        } else {
            self.start_read(Arc::clone(&self.shown));
        }
        self.ci.reset(&self.repositories, &self.accounts);
    }

    /// Reads every repository's open pull requests into `into`, a page at a time.
    fn start_read(&self, into: Arc<Mutex<Found>>) {
        let repositories = Arc::clone(&self.repositories);
        let accounts = self.accounts.clone();
        let (cancel, waker) = (self.cancel.clone(), self.waker.clone());
        into.lock().totals = vec![None; repositories.len()];
        jobs::detached(move || {
            // A host that needs signing in fails the same way for each of its repositories.
            let mut failed_hosts: Vec<&ForgeHost> = Vec::new();
            for (index, repository) in repositories.iter().enumerate() {
                let host = repository.host();
                let Some(account) = accounts.iter().find(|account| account.host() == host) else {
                    continue;
                };
                if failed_hosts.contains(&host) {
                    continue;
                }
                let read = pull_requests(account, repository, LIMIT, &cancel, |page| {
                    let mut found = into.lock();
                    if let Some(total) = found.totals.get_mut(index) {
                        *total = Some(page.total);
                    }
                    found
                        .pulls
                        .extend(page.pulls.into_iter().map(|pull| Listed {
                            repository: index,
                            pull,
                        }));
                    drop(found);
                    waker.request_repaint();
                });
                if let Err(error) = read {
                    if matches!(error, ForgeError::Cancelled) {
                        return;
                    }
                    tracing::debug!(%error, repository = %repository.full_name(), "requests unread");
                    let problem = ForgeProblem::of(&error);
                    if matches!(
                        problem,
                        ForgeProblem::SignIn { .. } | ForgeProblem::RateLimited { .. }
                    ) {
                        failed_hosts.push(host);
                    }
                    into.lock().problem.get_or_insert(problem);
                }
            }
            into.lock().done = true;
            waker.request_repaint();
        });
    }

    /// Makes the badges again when the pull requests shown changed.
    fn index(&mut self) {
        let found = self.shown.lock();
        let now = (self.generation, found.pulls.len());
        if now == self.indexed {
            return;
        }
        self.badges.clear();
        self.badges.extend(found.pulls.iter().filter_map(|listed| {
            let head = listed.pull.head_commit?;
            Some((
                head,
                Badge {
                    repository: listed.repository,
                    number: listed.pull.number,
                    checks: listed.pull.checks,
                },
            ))
        }));
        drop(found);
        self.badges.sort_unstable_by_key(|(id, _)| *id);
        self.indexed = now;
    }
}

impl Drop for RepoForge {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Each remote's first URL as the configuration writes it (`remote.<name>.url`).
fn written_urls(config: &ConfigSnapshot) -> Vec<(BString, BString)> {
    let mut urls: Vec<(BString, BString)> = Vec::new();
    for entry in config.entries() {
        let Some(name) = entry
            .key
            .strip_prefix(b"remote.")
            .and_then(|rest| rest.strip_suffix(b".url"))
        else {
            continue;
        };
        if !urls.iter().any(|(known, _)| known == name) {
            urls.push((BString::from(name), BString::from(entry.text().as_bytes())));
        }
    }
    urls
}
