//! Signing in to GitHub and GitLab hosts, from wherever egit offers it (a repository's banner,
//! its top bar, its sidebar, its requests' page, the profile): one dialog, its host, the token
//! typed, the sign-in under way and how it went; who is signed in to each host the profile
//! lists (asked once it shows); and signing out.
//!
//! The dialog signs in through the account it was opened for: a repository's (its own
//! credential helpers, if it names any), or the user's. A sign-in that went through is kept for
//! the run ([`SessionSignIns`]) and handed to every open repository's account for that host,
//! which reads its requests and pipelines again; a repository opened (or its remotes worked out)
//! later takes it too, so it holds even where no credential helper keeps it.
//!
//! Where none does, egit offers the helpers installed ([`KEEPERS`]): one chosen is added to the
//! user's configuration (`credential.helper`), and the run's sign-ins are handed to it.

use std::collections::HashMap;
use std::sync::Arc;

use egit_forge::{Account, ForgeError, ForgeHost, SignedIn};
use egit_git::sync::Mutex;
use egit_git::{
    Asking, CancelToken, ConfigTarget, CredentialPlace, Git, add_config_value, approve_credential,
    fill_credential, installed_helpers,
};
use egui_async::Bind;
use zeroize::Zeroizing;

use crate::jobs::{self, WriteLock};
use crate::problem::Problem;

/// The sign-ins made this run, by host name (with the host): every repository's account for
/// the host takes one, whenever it is made.
pub(crate) type SessionSignIns = Arc<Mutex<HashMap<String, (ForgeHost, SignedIn)>>>;

/// How many sign-ins a helper kept, of how many it was given.
type KeptOf = (usize, usize);

/// A helper set up: its words, and how it went.
pub(crate) type SetUp = (&'static str, Result<KeptOf, Problem>);

/// A credential helper egit can set up to keep sign-ins: its name (`git-credential-<name>`),
/// its value in `credential.helper`, its words, and where it keeps them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Keeper {
    pub(crate) name: &'static str,
    pub(crate) value: &'static str,
    pub(crate) words: &'static str,
    pub(crate) about: &'static str,
}

/// The helpers offered, the most fitting first, where installed.
pub(crate) const KEEPERS: [Keeper; 6] = [
    Keeper {
        name: "libsecret",
        value: "libsecret",
        words: "The system keychain (libsecret)",
        about: "Your desktop's keyring (GNOME Keyring, KWallet), encrypted and unlocked as you log in",
    },
    Keeper {
        name: "osxkeychain",
        value: "osxkeychain",
        words: "The macOS keychain",
        about: "Your login keychain, encrypted",
    },
    Keeper {
        name: "manager",
        value: "manager",
        words: "Git Credential Manager",
        about: "Git Credential Manager: encrypted in the system's own store, and it can sign in in the browser",
    },
    Keeper {
        name: "wincred",
        value: "wincred",
        words: "Windows' Credential Manager",
        about: "Windows' Credential Manager, encrypted",
    },
    Keeper {
        name: "cache",
        value: "cache --timeout=86400",
        words: "In memory, for a day (cache)",
        about: "Held in memory by git for a day, then forgotten: never written to disk",
    },
    Keeper {
        name: "store",
        value: "store",
        words: "In a file, unencrypted (store)",
        about: "Written as plain text to ~/.git-credentials: anyone who can read your files can read it",
    },
];

/// The sign-in dialog.
pub(crate) struct SignInDialog {
    pub(crate) host: ForgeHost,
    /// The account it signs in through: a repository's, or the user's.
    account: Arc<Account>,
    /// The token, as typed.
    pub(crate) token: Zeroizing<String>,
    /// The sign-in under way, or how it went.
    pub(crate) signing: Bind<SignedIn, Problem>,
    /// Who it signed in as, and whether git's helpers keep it, once it went through.
    pub(crate) signed: Option<(String, bool)>,
}

impl SignInDialog {
    /// Signs in with the token typed.
    pub(crate) fn with_token(&mut self) {
        let token = std::mem::take(&mut self.token);
        let account = Arc::clone(&self.account);
        let host = self.host.clone();
        self.signing.request(jobs::blocking(async move {
            account
                .sign_in_with_token(&token, &CancelToken::new())
                .await
                .map_err(|error| refusal(&host, &error))
        }));
    }

    /// Signs in through git's credential helper (Git Credential Manager signs in in the
    /// browser), or git's own questions.
    pub(crate) fn with_helper(&mut self) {
        let account = Arc::clone(&self.account);
        let host = self.host.clone();
        self.signing.request(jobs::blocking(async move {
            let context = format!("Signing in to {}", host.name());
            account
                .sign_in(&context, &CancelToken::new())
                .await
                .map_err(|error| refusal(&host, &error))
        }));
    }
}

/// Why signing in to `host` failed, in words a person can act on.
fn refusal(host: &ForgeHost, error: &ForgeError) -> Problem {
    let name = host.name();
    match error {
        ForgeError::Refused { .. } => Problem::new(
            format!("{name} refused the token"),
            format!(
                "Check it is whole (copied as {} showed it) and hasn't expired, and that it \
                 has the scope egit asks for when it opens the token's page.",
                host.kind().name()
            ),
        ),
        ForgeError::NotSignedIn { .. } => Problem::new(
            format!("Not signed in to {name}"),
            "No credentials were given.",
        ),
        other => Problem::new(format!("Signing in to {name} failed"), other.to_string()),
    }
}

/// The window's sign-ins.
#[derive(Default)]
pub(crate) struct SignIns {
    /// The dialog, while it is open.
    pub(crate) dialog: Option<SignInDialog>,
    /// The sign-ins made this run.
    pub(crate) session: SessionSignIns,
    /// The helpers installed that egit could set up, once looked for.
    keepers: Option<Bind<Vec<Keeper>, Problem>>,
    /// A helper being set up: its words, and how many of the run's sign-ins it then keeps.
    keeping: Option<(&'static str, Bind<KeptOf, Problem>)>,
    /// How setting the last one up went: its words, and how many sign-ins it kept of how many.
    kept: Option<SetUp>,
    /// Who is signed in to each host, by its name, as asked (`None`: nobody).
    who: HashMap<String, Bind<Option<String>, Problem>>,
    /// A sign-out under way: the host's, and how it went.
    signing_out: Option<(ForgeHost, Bind<(), Problem>)>,
}

/// What finished, for the window to pass on to its repositories.
pub(crate) enum Landed {
    /// Signed in to a host.
    In(ForgeHost, SignedIn),
    /// Signed out of a host.
    Out(ForgeHost),
    /// A credential helper was set up in the user's configuration.
    Kept,
}

/// Keeping sign-ins past this run, as it stands: the helpers to choose from (once looked for),
/// one being set up, and how the last went.
#[derive(Clone, Debug, Default)]
pub(crate) struct Keeping {
    /// The helpers installed that egit could set up; `None` while they are looked for.
    pub(crate) keepers: Option<Vec<Keeper>>,
    /// The words of the one being set up.
    pub(crate) setting_up: Option<&'static str>,
    /// How the last went: its words, and how many sign-ins it kept of how many.
    pub(crate) kept: Option<SetUp>,
}

impl Keeping {
    /// Whether the last helper set up went well: it kept every sign-in it was given.
    pub(crate) fn went_well(&self) -> bool {
        matches!(&self.kept, Some((_, Ok((kept, of)))) if kept == of)
    }
}

impl SignIns {
    /// The helpers installed that egit could set up, looked for once (in a job).
    pub(crate) fn keepers(&mut self, git: &Git) -> Option<&[Keeper]> {
        let keepers = self.keepers.get_or_insert_with(|| {
            let mut looking = Bind::new(true);
            let git = git.clone();
            looking.request(jobs::blocking(async move {
                let names: Vec<&str> = KEEPERS.iter().map(|keeper| keeper.name).collect();
                let found = installed_helpers(&git, &names).await.map_err(|error| {
                    Problem::git("The credential helpers couldn't be looked for", &error)
                })?;
                Ok(KEEPERS
                    .iter()
                    .copied()
                    .filter(|keeper| found.contains(&keeper.name))
                    .collect())
            }));
            looking
        });
        keepers.ok_ref().map(Vec::as_slice)
    }

    /// Whether a helper was set up (or tried) since the dialog last opened: how it went still
    /// shows.
    pub(crate) const fn has_kept(&self) -> bool {
        self.kept.is_some()
    }

    /// Keeping sign-ins past this run, as it stands (the helpers looked for once, through
    /// `git`).
    pub(crate) fn keeping(&mut self, git: &Git) -> Keeping {
        let keepers = self.keepers(git).map(<[Keeper]>::to_vec);
        Keeping {
            keepers,
            setting_up: self
                .keeping
                .as_mut()
                .and_then(|(words, job)| job.is_pending().then_some(*words)),
            kept: self.kept.clone(),
        }
    }

    /// Sets `keeper` up in the user's configuration (added to `credential.helper`, under the
    /// user's write lock), and hands it this run's sign-ins; how many it then keeps (each filled
    /// back) lands in [`Self::keeping`].
    pub(crate) fn keep_with(&mut self, git: &Git, writes: &WriteLock, keeper: Keeper) {
        let (git, writes) = (git.clone(), writes.clone());
        let signed: Vec<(ForgeHost, SignedIn)> = self.session.lock().values().cloned().collect();
        let mut keeping = Bind::new(true);
        keeping.request(jobs::blocking(async move {
            let failed =
                |error| Problem::git(format!("{} couldn't be set up", keeper.words), &error);
            {
                let _queued = writes.hold().await;
                add_config_value(
                    &git,
                    ConfigTarget::Global,
                    "credential.helper",
                    keeper.value,
                )
                .await
                .map_err(failed)?;
            }
            let mut kept = 0_usize;
            for (host, signed) in &signed {
                approve_credential(&git, None, &signed.credential)
                    .await
                    .map_err(failed)?;
                let place = CredentialPlace {
                    protocol: host.protocol(),
                    host: host.name(),
                    username: Some(&signed.login),
                };
                let back = fill_credential(&git, None, place, Asking::Never, &CancelToken::new())
                    .await
                    .map_err(failed)?;
                if back.is_some_and(|back| back.password() == signed.credential.password()) {
                    kept = kept.saturating_add(1);
                }
            }
            Ok((kept, signed.len()))
        }));
        self.keeping = Some((keeper.words, keeping));
        self.kept = None;
    }

    /// Opens the dialog for `host`, signing in through `account`.
    pub(crate) fn open(&mut self, host: ForgeHost, account: Arc<Account>) {
        self.kept = None;
        self.dialog = Some(SignInDialog {
            host,
            account,
            token: Zeroizing::new(String::new()),
            signing: Bind::new(true),
            signed: None,
        });
    }

    /// Who is signed in to `host`, asked once (in a job) with what git's helpers keep, through
    /// `git` (the user's configuration).
    pub(crate) fn who(
        &mut self,
        git: &Git,
        host: &ForgeHost,
    ) -> Option<Result<&Option<String>, &Problem>> {
        let asked = self.who.entry(host.name().to_owned()).or_insert_with(|| {
            let mut asking = Bind::new(true);
            let account = Account::new(git.clone(), None, host.clone());
            let name = host.name().to_owned();
            asking.request(jobs::blocking(async move {
                account
                    .who(&CancelToken::new())
                    .await
                    .map_err(|error| match error {
                        ForgeError::Refused { .. } => Problem::new(
                            format!("{name} refused the credentials git keeps"),
                            "They were forgotten: sign in again.",
                        ),
                        other => {
                            Problem::new(format!("{name} couldn't be asked"), other.to_string())
                        }
                    })
            }));
            asking
        });
        asked.read_as_ref()
    }

    /// Whether a sign-out of `host` is under way.
    pub(crate) fn is_signing_out(&mut self, host: &ForgeHost) -> bool {
        self.signing_out
            .as_mut()
            .is_some_and(|(out, signing)| out == host && signing.is_pending())
    }

    /// Signs out of `host` (git's helpers forget its credential), through `git`.
    pub(crate) fn sign_out(&mut self, git: &Git, host: ForgeHost) {
        let account = Account::new(git.clone(), None, host.clone());
        let mut signing = Bind::new(true);
        let name = host.name().to_owned();
        signing.request(jobs::blocking(async move {
            account
                .sign_out(&CancelToken::new())
                .await
                .map_err(|error| {
                    Problem::new(format!("Signing out of {name} failed"), error.to_string())
                })
        }));
        self.signing_out = Some((host, signing));
    }

    /// Takes in a sign-in, sign-out, or helper set up that finished: who is signed in where
    /// follows it, and it is returned for the repositories to follow too.
    pub(crate) fn poll(&mut self) -> Option<Landed> {
        let set_up = self
            .keeping
            .as_mut()
            .is_some_and(|(_, job)| job.is_finished());
        if set_up && let Some((words, mut job)) = self.keeping.take() {
            self.kept = Some((words, job.take().unwrap_or(Ok((0, 0)))));
            return Some(Landed::Kept);
        }
        if let Some(dialog) = &mut self.dialog
            && dialog.signing.is_finished()
            && let Some(Ok(signed)) = dialog.signing.read_as_ref()
            && dialog.signed.is_none()
        {
            let signed = signed.clone();
            dialog.signed = Some((signed.login.clone(), signed.kept));
            let host = dialog.host.clone();
            self.session
                .lock()
                .insert(host.name().to_owned(), (host.clone(), signed.clone()));
            let mut known = Bind::new(true);
            known.fill(Ok(Some(signed.login.clone())));
            self.who.insert(host.name().to_owned(), known);
            return Some(Landed::In(host, signed));
        }
        let finished = self
            .signing_out
            .as_mut()
            .is_some_and(|(_, signing)| signing.is_finished());
        if finished && let Some((host, mut signing)) = self.signing_out.take() {
            if let Some(Err(problem)) = signing.take() {
                tracing::warn!(%problem, "signing out failed");
            }
            self.who.remove(host.name());
            self.session.lock().remove(host.name());
            return Some(Landed::Out(host));
        }
        None
    }
}
