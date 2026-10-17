//! Signing in to a GitHub host with the credential git's helpers keep for it: taken without
//! asking for work nobody watches, asked for (through egit's prompts, or a helper's own window)
//! when the user signs in; kept by the helpers (`approve`) once the host accepts it, forgotten
//! (`reject`) when it refuses it, as git itself does with a remote's.

use std::sync::Arc;

use egit_git::{
    Asking, CancelToken, Credential, CredentialPlace, Git, Repository, approve_credential,
    fill_credential, reject_credential,
};
use parking_lot::Mutex;

use crate::error::ForgeError;
use crate::host::GitHubHost;
use crate::http::{Answer, Http};

/// One host's sign-in, under one repository's configuration (a repository may name credential
/// helpers of its own).
#[derive(Debug)]
pub struct Account {
    http: Http,
    git: Git,
    repository: Option<Arc<Repository>>,
    state: Mutex<State>,
}

/// What is known of the sign-in.
#[derive(Debug, Default)]
struct State {
    credential: Option<Arc<Credential>>,
    /// Whether the helpers were told the credential works.
    approved: bool,
    /// Who the host says the credential is.
    login: Option<String>,
}

impl Account {
    /// `host`'s sign-in, with `repository`'s configuration (the user's without one).
    #[must_use]
    pub fn new(git: Git, repository: Option<Arc<Repository>>, host: GitHubHost) -> Self {
        Self {
            http: Http::new(host),
            git,
            repository,
            state: Mutex::default(),
        }
    }

    /// The host.
    #[must_use]
    pub const fn host(&self) -> &GitHubHost {
        self.http.host()
    }

    /// Who is signed in, once the host has said.
    #[must_use]
    pub fn login(&self) -> Option<String> {
        self.state.lock().login.clone()
    }

    /// Signs in: the credential from git's helpers, or the user (asked as `context` says);
    /// checked with the host, which says whose it is; then kept by the helpers.
    ///
    /// # Errors
    ///
    /// [`ForgeError::NotSignedIn`] when the user declined, [`ForgeError::Refused`] when the
    /// host refused the credential (the helpers forget it), and any failure to reach the host.
    pub fn sign_in(&self, context: &str, cancel: &CancelToken) -> Result<String, ForgeError> {
        let credential = self
            .fill(Asking::As(context), cancel)?
            .ok_or_else(|| self.not_signed_in())?;
        self.state.lock().credential = Some(credential);
        let login = crate::github::viewer(self, cancel)?;
        self.state.lock().login = Some(login.clone());
        Ok(login)
    }

    /// Posts `body` to `url` with the credential (filled without asking when there is none
    /// yet); a refused credential is forgotten, an accepted one kept by the helpers.
    pub(crate) fn post(
        &self,
        url: &str,
        body: &[u8],
        cancel: &CancelToken,
    ) -> Result<Answer, ForgeError> {
        let known = self.state.lock().credential.clone();
        let credential = if let Some(credential) = known {
            credential
        } else {
            let filled = self
                .fill(Asking::Never, cancel)?
                .ok_or_else(|| self.not_signed_in())?;
            let mut state = self.state.lock();
            Arc::clone(state.credential.get_or_insert(filled))
        };
        if cancel.is_cancelled() {
            return Err(ForgeError::Cancelled);
        }
        let answer = self.http.post(url, body, credential.password())?;
        if answer.status == 401 {
            self.forget(&credential);
            return Err(ForgeError::Refused {
                host: self.host().name().to_owned(),
            });
        }
        if (200..300).contains(&answer.status) {
            self.keep(&credential);
        }
        Ok(answer)
    }

    /// Notes who the credential is, from an answer that said so.
    pub(crate) fn note_login(&self, login: &str) {
        self.state.lock().login = Some(login.to_owned());
    }

    /// The credential git's helpers (or the user) give.
    fn fill(
        &self,
        asking: Asking<'_>,
        cancel: &CancelToken,
    ) -> Result<Option<Arc<Credential>>, ForgeError> {
        let place = CredentialPlace {
            protocol: self.host().protocol(),
            host: self.host().name(),
        };
        let filled = fill_credential(&self.git, self.repository.as_deref(), place, asking, cancel)?;
        Ok(filled.map(Arc::new))
    }

    /// Has the helpers keep `credential`, once.
    fn keep(&self, credential: &Arc<Credential>) {
        let mut state = self.state.lock();
        let current = state
            .credential
            .as_ref()
            .is_some_and(|known| Arc::ptr_eq(known, credential));
        if !current || state.approved {
            return;
        }
        state.approved = true;
        drop(state);
        if let Err(error) = approve_credential(&self.git, self.repository.as_deref(), credential) {
            tracing::warn!(%error, host = self.host().name(), "the credential couldn't be kept");
        }
    }

    /// Has the helpers forget `credential`, and forgets it here (unless another replaced it).
    fn forget(&self, credential: &Arc<Credential>) {
        let mut state = self.state.lock();
        if state
            .credential
            .as_ref()
            .is_some_and(|known| Arc::ptr_eq(known, credential))
        {
            *state = State::default();
        }
        drop(state);
        if let Err(error) = reject_credential(&self.git, self.repository.as_deref(), credential) {
            tracing::warn!(%error, host = self.host().name(), "the credential couldn't be forgotten");
        }
    }

    fn not_signed_in(&self) -> ForgeError {
        ForgeError::NotSignedIn {
            host: self.host().name().to_owned(),
        }
    }
}
