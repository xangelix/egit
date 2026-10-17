//! A repository's forge: which of its remotes are GitHub repositories (on github.com, or on an
//! Enterprise host the configuration lists as `egit.githubHost`), an account per host signed in
//! with git's credentials, and their open pull requests, read in the background (a page at a
//! time, shown as each lands) and read again when the remotes may have moved; the badges they
//! put on their head commits.
//!
//! A read again takes over only once complete, so the list never empties or jumps meanwhile.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_forge::{
    Account, CheckState, ForgeError, ForgeHost, ForgeHosts, ForgeRepository, PullRequest,
    forge_repository, pull_requests, resolve_ssh_alias,
};
use egit_git::{CancelToken, ConfigSnapshot, Git, ObjectId, Repository, remote_urls};
use egui_async::Bind;
use parking_lot::{Mutex, MutexGuard};

use crate::jobs;
use crate::problem::Problem;

/// The key listing GitHub Enterprise hosts (one value each).
pub(crate) const HOSTS_KEY: &str = "egit.githubHost";

/// The most open pull requests read for each repository: the most recently updated.
pub(crate) const LIMIT: u32 = 1_000;

/// A remote that is a GitHub repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForgeRemote {
    pub(crate) remote: BString,
    pub(crate) repository: ForgeRepository,
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
                "The pull requests couldn't be read",
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
    /// The remotes that are GitHub repositories, being worked out.
    resolving: Bind<Arc<[ForgeRemote]>, Problem>,
    /// What they were worked out as, last.
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
    /// A sign-in under way (or done): who it is.
    signing_in: Bind<String, Problem>,
    /// Whether a sign-in is under way.
    pub(crate) signing: bool,
    /// Why the last sign-in failed, if it did.
    pub(crate) sign_in_problem: Option<Problem>,
    /// Wakes the window as pages land.
    waker: egui::Context,
}

impl RepoForge {
    /// A repository's forge, nothing known yet.
    pub(crate) fn new(waker: &egui::Context) -> Self {
        Self {
            resolving: Bind::new(true),
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
            signing_in: Bind::new(true),
            signing: false,
            sign_in_problem: None,
            waker: waker.clone(),
        }
    }

    /// Works out (again) which remotes are GitHub repositories, from the repository's
    /// configuration as just read: the hosts it lists, and its remotes' URLs, as git rewrites
    /// them (else as written).
    pub(crate) fn configured(
        &mut self,
        git: &Git,
        repository: &Arc<Repository>,
        config: &ConfigSnapshot,
    ) {
        let hosts = ForgeHosts::with_enterprise(
            config
                .get_all(HOSTS_KEY)
                .filter_map(|entry| entry.text().to_str().ok()),
        );
        let written = written_urls(config);
        let (git, repository) = (git.clone(), Arc::clone(repository));
        self.resolving.request(jobs::blocking(move || {
            let urls = remote_urls(&git, &repository)
                .map_err(|error| Problem::git("The remotes couldn't be read", &error))?;
            Ok(urls
                .into_iter()
                .filter_map(|(remote, url)| {
                    let resolved = url
                        .to_str()
                        .ok()
                        .and_then(|url| forge_repository(url, &hosts, resolve_ssh_alias));
                    let repository = resolved.or_else(|| {
                        let written = written
                            .iter()
                            .find(|(name, _)| *name == remote)
                            .and_then(|(_, url)| url.to_str().ok())?;
                        forge_repository(written, &hosts, resolve_ssh_alias)
                    })?;
                    Some(ForgeRemote { remote, repository })
                })
                .collect())
        }));
    }

    /// Takes in what its jobs finished: remotes worked out (pull requests read when they
    /// changed), a read again complete, a sign-in; and keeps the badges current.
    pub(crate) fn poll(&mut self, git: &Git, repository: &Arc<Repository>) {
        if self.resolving.is_finished()
            && let Some(Ok(remotes)) = self.resolving.take()
            && remotes != self.remotes
        {
            self.set_remotes(git, repository, remotes);
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
        if self.signing_in.is_finished() {
            match self.signing_in.take() {
                // Read with the new credential.
                Some(Ok(_)) => self.refresh(),
                Some(Err(problem)) => self.sign_in_problem = Some(problem),
                None => {}
            }
        }
        self.signing = self.signing_in.is_pending();
        self.index();
    }

    /// Reads the pull requests again (after the one under way, if any).
    pub(crate) fn refresh(&mut self) {
        if self.repositories.is_empty() {
            return;
        }
        let first_under_way = !self.shown.lock().done && self.generation == 0;
        if self.reading.is_some() || first_under_way {
            self.again = true;
            return;
        }
        let reading = Arc::new(Mutex::new(Found::default()));
        self.start_read(Arc::clone(&reading));
        self.reading = Some(reading);
    }

    /// Signs in to `host`, asking the user as git's helpers do; the pull requests are read again
    /// once it worked.
    pub(crate) fn sign_in(&mut self, host: &str) {
        let Some(account) = self
            .accounts
            .iter()
            .find(|account| account.host().name() == host)
            .cloned()
        else {
            return;
        };
        self.sign_in_problem = None;
        self.signing = true;
        self.signing_in.request(jobs::blocking(move || {
            let context = format!("Signing in to {}", account.host().name());
            account
                .sign_in(&context, &CancelToken::new())
                .map_err(|error| match error {
                    ForgeError::NotSignedIn { host } => Problem::new(
                        format!("Not signed in to {host}"),
                        "No credentials were given.",
                    ),
                    other => Problem::new("Signing in failed", other.to_string()),
                })
        }));
    }

    /// The pull requests shown.
    pub(crate) fn found(&self) -> MutexGuard<'_, Found> {
        self.shown.lock()
    }

    /// Whether a read is under way.
    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some() || !self.shown.lock().done
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

    /// The first remote that is repository `index`.
    pub(crate) fn remote_of(&self, index: usize) -> Option<&BStr> {
        let repository = self.repositories.get(index)?;
        self.remotes
            .iter()
            .find(|remote| remote.repository.is(repository))
            .map(|remote| remote.remote.as_bstr())
    }

    /// The repository a remote is, if it is one of GitHub's.
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
                Arc::new(Account::new(
                    git.clone(),
                    Some(Arc::clone(repository)),
                    host.clone(),
                ))
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
                    tracing::debug!(%error, repository = %repository.full_name(), "pull requests unread");
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
