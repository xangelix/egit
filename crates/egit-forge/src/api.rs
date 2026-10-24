//! The forges' APIs, each call sent to its repository's forge: GitHub's pull requests or
//! GitLab's merge requests, alike to the caller.

use egit_git::CancelToken;

use crate::account::Account;
use crate::error::ForgeError;
use crate::host::ForgeKind;
use crate::model::{NewPullRequest, Opened, PullRequestDetails, PullRequestPage, RepositoryInfo};
use crate::repository::ForgeRepository;
use crate::{github, gitlab};

/// Reads `repository`'s open pull (or merge) requests, the most recently updated first, a page
/// at a time (each handed to `found` as it lands), up to `limit` of them.
///
/// # Errors
///
/// Any of [`ForgeError`]'s, from the page that failed (the pages before it were handed over).
pub async fn pull_requests(
    account: &Account,
    repository: &ForgeRepository,
    limit: u32,
    cancel: &CancelToken,
    found: impl FnMut(PullRequestPage),
) -> Result<(), ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitHub => github::pull_requests(account, repository, limit, cancel, found).await,
        ForgeKind::GitLab => {
            gitlab::merge_requests(account, repository, limit, cancel, found).await
        }
    }
}

/// Reads pull (or merge) request `number`'s details.
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub async fn pull_request_details(
    account: &Account,
    repository: &ForgeRepository,
    number: u32,
    cancel: &CancelToken,
) -> Result<PullRequestDetails, ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitHub => {
            github::pull_request_details(account, repository, number, cancel).await
        }
        ForgeKind::GitLab => {
            gitlab::merge_request_details(account, repository, number, cancel).await
        }
    }
}

/// Reads `repository`'s default branch and the repository it was forked from.
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub async fn repository_info(
    account: &Account,
    repository: &ForgeRepository,
    cancel: &CancelToken,
) -> Result<RepositoryInfo, ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitHub => github::repository_info(account, repository, cancel).await,
        ForgeKind::GitLab => gitlab::project_info(account, repository, cancel).await,
    }
}

/// Opens a pull (or merge) request into `repository`.
///
/// # Errors
///
/// Any of [`ForgeError`]'s; [`ForgeError::Status`] with the forge's reason when it refuses
/// (one is already open for the branch, nothing to merge, …).
pub async fn create_pull_request(
    account: &Account,
    repository: &ForgeRepository,
    new: &NewPullRequest,
    cancel: &CancelToken,
) -> Result<Opened, ForgeError> {
    match repository.host().kind() {
        ForgeKind::GitHub => github::create_pull_request(account, repository, new, cancel).await,
        ForgeKind::GitLab => gitlab::create_merge_request(account, repository, new, cancel).await,
    }
}

/// The name the host's ssh is on, as the host says: its projects' ssh addresses give it.
///
/// It may not be the name its web pages are on (a self-managed GitLab's
/// `gitlab_shell_ssh_host`). `None` for GitHub (its ssh is on its own name), or a GitLab that
/// shows the account no project.
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub async fn ssh_host(
    account: &Account,
    cancel: &CancelToken,
) -> Result<Option<String>, ForgeError> {
    match account.host().kind() {
        ForgeKind::GitHub => Ok(None),
        ForgeKind::GitLab => gitlab::ssh_host(account, cancel).await,
    }
}

/// Who the account's credential is, as its host says (which checks the host accepts it).
pub(crate) async fn viewer(account: &Account, cancel: &CancelToken) -> Result<String, ForgeError> {
    match account.host().kind() {
        ForgeKind::GitHub => github::viewer(account, cancel).await,
        ForgeKind::GitLab => gitlab::current_user(account, cancel).await,
    }
}
