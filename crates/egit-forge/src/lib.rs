//! egit's interface to forges' web APIs: GitHub's (github.com and GitHub Enterprise Server),
//! for now. Which repository a remote is, signing in with the credentials git's helpers keep,
//! and pull requests: read, detailed, and created. No egui.
//!
//! egit's own network connections are these alone; git makes every other. They go over
//! HTTPS through rustls on aws-lc-rs, checked against the system's trust store.

mod account;
mod error;
mod github;
mod host;
mod http;
mod repository;
#[cfg(feature = "testing")]
pub mod testing;

pub use account::Account;
pub use error::ForgeError;
pub use github::{
    Check, CheckState, Mergeable, NewPullRequest, Opened, PullRequest, PullRequestDetails,
    PullRequestPage, RepositoryInfo, Review, ReviewDecision, ReviewState, create_pull_request,
    pull_request_details, pull_requests, repository_info,
};
pub use host::{ForgeHost, ForgeHosts};
pub use repository::{ForgeRepository, forge_repository, resolve_ssh_alias};
