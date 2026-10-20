//! Signing in to GitHub and GitLab hosts, from wherever egit offers it (a repository's banner,
//! its top bar, its sidebar, its requests' page, the profile): one dialog, its host, the token
//! typed, the sign-in under way and how it went; who is signed in to each host the profile
//! lists (asked once it shows); and signing out.
//!
//! The dialog signs in through the account it was opened for: a repository's (its own
//! credential helpers, if it names any), or the user's. A sign-in that went through is handed to
//! every open repository's account for that host, which reads its requests and pipelines again.

use std::collections::HashMap;
use std::sync::Arc;

use egit_forge::{Account, ForgeError, ForgeHost, SignedIn};
use egit_git::{CancelToken, Git};
use egui_async::Bind;
use zeroize::Zeroizing;

use crate::jobs;
use crate::problem::Problem;

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
        self.signing.request(jobs::blocking(move || {
            account
                .sign_in_with_token(&token, &CancelToken::new())
                .map_err(|error| refusal(&host, &error))
        }));
    }

    /// Signs in through git's credential helper (Git Credential Manager signs in in the
    /// browser), or git's own questions.
    pub(crate) fn with_helper(&mut self) {
        let account = Arc::clone(&self.account);
        let host = self.host.clone();
        self.signing.request(jobs::blocking(move || {
            let context = format!("Signing in to {}", host.name());
            account
                .sign_in(&context, &CancelToken::new())
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
}

impl SignIns {
    /// Opens the dialog for `host`, signing in through `account`.
    pub(crate) fn open(&mut self, host: ForgeHost, account: Arc<Account>) {
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
            asking.request(jobs::blocking(move || {
                account
                    .who(&CancelToken::new())
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
        signing.request(jobs::blocking(move || {
            account.sign_out(&CancelToken::new()).map_err(|error| {
                Problem::new(format!("Signing out of {name} failed"), error.to_string())
            })
        }));
        self.signing_out = Some((host, signing));
    }

    /// Takes in a sign-in or sign-out that finished: who is signed in where follows it, and it
    /// is returned for the repositories to follow too.
    pub(crate) fn poll(&mut self) -> Option<Landed> {
        if let Some(dialog) = &mut self.dialog
            && dialog.signing.is_finished()
            && let Some(Ok(signed)) = dialog.signing.read_as_ref()
            && dialog.signed.is_none()
        {
            let signed = signed.clone();
            dialog.signed = Some((signed.login.clone(), signed.kept));
            let host = dialog.host.clone();
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
            return Some(Landed::Out(host));
        }
        None
    }
}
