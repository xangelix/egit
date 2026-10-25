//! Signing in to a forge's host with the credential git's helpers keep for it: taken without
//! asking for work nobody watches, asked for (through egit's prompts, or a helper's own window)
//! when the user signs in, or given as a token; kept by the helpers (`approve`) once the host
//! accepts it, forgotten (`reject`) when it refuses it, as git itself does with a remote's.
//!
//! A token given is checked first by an account of its own that never touches the helpers, so
//! a mistyped one makes them forget nothing; accepted, it is kept under the login the host says
//! it is, where git finds it for the host's HTTPS remotes too.

use std::sync::Arc;

use egit_git::{
    Asking, CancelToken, Credential, CredentialPlace, Git, Repository, approve_credential,
    fill_credential, reject_credential,
};

/// A sign-in that went through.
#[derive(Clone, Debug)]
pub struct SignedIn {
    /// Who the host says it is.
    pub login: String,
    /// Whether git's credential helpers keep it (else it lasts until egit quits).
    pub kept: bool,
    /// The credential, for the host's other accounts (other repositories' sign-ins) to take.
    pub credential: Arc<Credential>,
}
use egit_git::sync::Mutex;

use crate::error::ForgeError;
use crate::host::ForgeHost;
use crate::http::{Answer, Http, Sink, Streamed, Taking};

/// One host's sign-in, under one repository's configuration (a repository may name credential
/// helpers of its own).
#[derive(Debug)]
pub struct Account {
    http: Http,
    git: Git,
    repository: Option<Arc<Repository>>,
    state: Mutex<State>,
    /// Whether it only checks a credential given, never telling the helpers anything.
    probing: bool,
}

/// What is known of the sign-in.
#[derive(Debug, Default)]
struct State {
    credential: Option<Arc<Credential>>,
    /// Whether the helpers were told the credential works.
    approved: bool,
    /// Who the host says the credential is.
    login: Option<String>,
    /// Whether the host refused the last credential (forgotten since): finding none after
    /// that is still a refusal, until one is accepted.
    refused: bool,
    /// Whether it may push to each repository (by path), as the host said, and when: what it
    /// may do to their CI/CD runs (GitHub's).
    pushes: Vec<(String, web_time::Instant, bool)>,
}

/// How long what an account may do in a repository is believed.
const PERMISSION_LASTS: std::time::Duration = std::time::Duration::from_secs(600);

impl Account {
    /// `host`'s sign-in, with `repository`'s configuration (the user's without one).
    #[must_use]
    pub fn new(git: Git, repository: Option<Arc<Repository>>, host: ForgeHost) -> Self {
        Self {
            http: Http::new(host, &git),
            git,
            repository,
            state: Mutex::default(),
            probing: false,
        }
    }

    /// The host.
    #[must_use]
    pub const fn host(&self) -> &ForgeHost {
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
    pub async fn sign_in(
        &self,
        context: &str,
        cancel: &CancelToken,
    ) -> Result<SignedIn, ForgeError> {
        let credential = self
            .fill(Asking::As(context), None, cancel)
            .await?
            .ok_or_else(|| self.not_signed_in())?;
        self.state.lock().credential = Some(Arc::clone(&credential));
        let login = crate::api::viewer(self, cancel).await?;
        self.state.lock().login = Some(login.clone());
        let kept = self.is_kept(&credential, cancel).await;
        Ok(SignedIn {
            login,
            kept,
            credential,
        })
    }

    /// Signs in with `token`, given by the user: checked with the host (which says whose it
    /// is) without the helpers hearing of it, then kept by them under that login, so git uses
    /// it for the host's HTTPS remotes too.
    ///
    /// # Errors
    ///
    /// [`ForgeError::Refused`] when the host refuses it (or it can't be a token: empty, or
    /// holding a line break), and any failure to reach the host.
    pub async fn sign_in_with_token(
        &self,
        token: &str,
        cancel: &CancelToken,
    ) -> Result<SignedIn, ForgeError> {
        let token = token.trim();
        let refused = || ForgeError::Refused {
            host: self.host().name().to_owned(),
        };
        let probe = Credential::given(self.place(None), token).ok_or_else(refused)?;
        let checking = Self {
            http: Http::new(self.host().clone(), &self.git),
            git: self.git.clone(),
            repository: self.repository.clone(),
            state: Mutex::new(State {
                credential: Some(Arc::new(probe)),
                ..State::default()
            }),
            probing: true,
        };
        let login = crate::api::viewer(&checking, cancel).await?;
        let credential =
            Arc::new(Credential::given(self.place(Some(&login)), token).ok_or_else(refused)?);
        *self.state.lock() = State {
            credential: Some(Arc::clone(&credential)),
            login: Some(login.clone()),
            ..State::default()
        };
        self.keep(&credential).await;
        let kept = self.is_kept(&credential, cancel).await;
        Ok(SignedIn {
            login,
            kept,
            credential,
        })
    }

    /// Takes the sign-in another account of the host made (a repository's, the profile's).
    pub fn adopt(&self, signed: &SignedIn) {
        *self.state.lock() = State {
            credential: Some(Arc::clone(&signed.credential)),
            login: Some(signed.login.clone()),
            approved: true,
            refused: false,
            pushes: Vec::new(),
        };
    }

    /// Who is signed in: the login the host said, else asked of it with what git's helpers
    /// keep (never asking anyone); `None` with nothing kept.
    ///
    /// # Errors
    ///
    /// [`ForgeError::Refused`] when the host refuses what the helpers keep (they forget it),
    /// and any failure to reach the host.
    pub async fn who(&self, cancel: &CancelToken) -> Result<Option<String>, ForgeError> {
        if let Some(login) = self.login() {
            return Ok(Some(login));
        }
        match crate::api::viewer(self, cancel).await {
            Ok(login) => Ok(Some(login)),
            Err(ForgeError::NotSignedIn { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Signs out: git's helpers forget the credential (git asks again the next time it reaches
    /// the host over HTTPS), and so does this account.
    ///
    /// # Errors
    ///
    /// When git's helpers fail.
    pub async fn sign_out(&self, cancel: &CancelToken) -> Result<(), ForgeError> {
        let known = self.state.lock().credential.clone();
        let credential = match known {
            Some(credential) => Some(credential),
            None => self.fill(Asking::Never, None, cancel).await?,
        };
        *self.state.lock() = State::default();
        if let Some(credential) = credential {
            reject_credential(&self.git, self.repository.as_deref(), &credential).await?;
        }
        Ok(())
    }

    /// Forgets the sign-in here alone (another account of the host signed out).
    pub fn signed_out(&self) {
        *self.state.lock() = State::default();
    }

    /// Sends `body` to `url` (a GET without one) with the credential, filled without asking
    /// when there is none yet; a refused credential is forgotten, an accepted one kept by the
    /// helpers.
    pub(crate) async fn send(
        &self,
        url: &str,
        body: Option<&[u8]>,
        cancel: &CancelToken,
    ) -> Result<Answer, ForgeError> {
        let credential = self.credential(cancel).await?;
        let answer = self.http.send(url, body, credential.password()).await?;
        self.answered(&credential, answer.status).await?;
        Ok(answer)
    }

    /// GETs `url` with the credential as [`Self::send`] does, its body handed to `sink` a part
    /// at a time as it comes, with its status and `Content-Range` (`from`: a range of it;
    /// `limit`: the most read).
    pub(crate) async fn stream(
        &self,
        url: &str,
        (from, limit): (Option<u64>, u64),
        sink: &mut Sink<'_>,
        cancel: &CancelToken,
    ) -> Result<Streamed, ForgeError> {
        let credential = self.credential(cancel).await?;
        let streamed = self
            .http
            .stream(url, credential.password(), (from, limit), sink)
            .await?;
        self.answered(&credential, streamed.status).await?;
        Ok(streamed)
    }

    /// GETs `url`, one of the host's, for a download (an artifact), with the credential, its
    /// body taken as `taking` says; a redirect said, not followed.
    pub(crate) async fn download(
        &self,
        url: &str,
        taking: &mut Taking<'_>,
        cancel: &CancelToken,
    ) -> Result<Streamed, ForgeError> {
        let credential = self.credential(cancel).await?;
        let streamed = self
            .http
            .download(url, Some(credential.password()), taking)
            .await?;
        self.answered(&credential, streamed.status).await?;
        Ok(streamed)
    }

    /// GETs `url`, a place the host pointed to (an artifact's or a log's file, its address
    /// signed), without the credential, its body taken as `taking` says.
    pub(crate) async fn download_public(
        &self,
        url: &str,
        taking: &mut Taking<'_>,
        cancel: &CancelToken,
    ) -> Result<Streamed, ForgeError> {
        if cancel.is_cancelled() {
            return Err(ForgeError::Cancelled);
        }
        self.http.download(url, None, taking).await
    }

    /// Whether it may push to the repository at `path`, as the host said lately.
    pub(crate) fn remembered_push(&self, path: &str) -> Option<bool> {
        let now = web_time::Instant::now();
        self.state
            .lock()
            .pushes
            .iter()
            .find(|(held, when, _)| {
                held == path && now.saturating_duration_since(*when) < PERMISSION_LASTS
            })
            .map(|(_, _, may)| *may)
    }

    /// Notes whether it may push to the repository at `path`, as the host said just now.
    pub(crate) fn remember_push(&self, path: &str, may: bool) {
        let mut state = self.state.lock();
        state.pushes.retain(|(held, _, _)| held != path);
        state
            .pushes
            .push((path.to_owned(), web_time::Instant::now(), may));
    }

    /// The credential to send: the one known, else filled without asking.
    async fn credential(&self, cancel: &CancelToken) -> Result<Arc<Credential>, ForgeError> {
        let known = self.state.lock().credential.clone();
        let credential = if let Some(credential) = known {
            credential
        } else {
            let filled = self
                .fill(Asking::Never, None, cancel)
                .await?
                .ok_or_else(|| {
                    if self.state.lock().refused {
                        ForgeError::Refused {
                            host: self.host().name().to_owned(),
                        }
                    } else {
                        self.not_signed_in()
                    }
                })?;
            let mut state = self.state.lock();
            Arc::clone(state.credential.get_or_insert(filled))
        };
        if cancel.is_cancelled() {
            return Err(ForgeError::Cancelled);
        }
        Ok(credential)
    }

    /// Takes in how the host answered `credential`: refused, it is forgotten; accepted, kept.
    async fn answered(&self, credential: &Arc<Credential>, status: u16) -> Result<(), ForgeError> {
        if status == 401 {
            self.forget(credential).await;
            return Err(ForgeError::Refused {
                host: self.host().name().to_owned(),
            });
        }
        if (200..300).contains(&status) {
            self.keep(credential).await;
        }
        Ok(())
    }

    /// Notes who the credential is, from an answer that said so.
    pub(crate) fn note_login(&self, login: &str) {
        self.state.lock().login = Some(login.to_owned());
    }

    /// Where its credential is kept: the host, for `username` (`None`: whichever).
    fn place<'a>(&'a self, username: Option<&'a str>) -> CredentialPlace<'a> {
        CredentialPlace {
            protocol: self.host().protocol(),
            host: self.host().name(),
            username,
        }
    }

    /// The credential git's helpers (or the user) give, for `username` (`None`: whichever).
    async fn fill(
        &self,
        asking: Asking<'_>,
        username: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<Option<Arc<Credential>>, ForgeError> {
        let place = self.place(username);
        let filled =
            fill_credential(&self.git, self.repository.as_deref(), place, asking, cancel).await?;
        Ok(filled.map(Arc::new))
    }

    /// Whether git's helpers give `credential` back (none keeps one where none is set up).
    async fn is_kept(&self, credential: &Credential, cancel: &CancelToken) -> bool {
        let username = Some(credential.username()).filter(|name| !name.is_empty());
        self.fill(Asking::Never, username, cancel)
            .await
            .ok()
            .flatten()
            .is_some_and(|kept| kept.password() == credential.password())
    }

    /// Has the helpers keep `credential`, once.
    async fn keep(&self, credential: &Arc<Credential>) {
        // Noted here first, in a scope of its own: the lock is never held while git runs.
        let approving = {
            let mut state = self.state.lock();
            let current = state
                .credential
                .as_ref()
                .is_some_and(|known| Arc::ptr_eq(known, credential));
            let approving = current && !state.approved && !self.probing;
            if approving {
                state.approved = true;
                state.refused = false;
            }
            approving
        };
        if !approving {
            return;
        }
        if let Err(error) =
            approve_credential(&self.git, self.repository.as_deref(), credential).await
        {
            tracing::warn!(%error, host = self.host().name(), "the credential couldn't be kept");
        }
    }

    /// Has the helpers forget `credential`, and forgets it here (unless another replaced it),
    /// remembering that the host refused it.
    async fn forget(&self, credential: &Arc<Credential>) {
        {
            let mut state = self.state.lock();
            if state
                .credential
                .as_ref()
                .is_some_and(|known| Arc::ptr_eq(known, credential))
            {
                *state = State {
                    refused: true,
                    ..State::default()
                };
            }
        }
        // A token being checked was never theirs to forget.
        if self.probing {
            return;
        }
        if let Err(error) =
            reject_credential(&self.git, self.repository.as_deref(), credential).await
        {
            tracing::warn!(%error, host = self.host().name(), "the credential couldn't be forgotten");
        }
    }

    fn not_signed_in(&self) -> ForgeError {
        ForgeError::NotSignedIn {
            host: self.host().name().to_owned(),
        }
    }
}
