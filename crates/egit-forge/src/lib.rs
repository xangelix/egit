//! egit's interface to forges' web APIs: GitHub's (github.com and GitHub Enterprise Server)
//! and GitLab's (gitlab.com and self-managed). Which repository a remote is, signing in with the
//! credentials git's helpers keep, and pull requests (GitHub's) or merge requests (GitLab's):
//! read, detailed, and opened; and GitLab's CI/CD pipelines: each commit's latest, and one
//! whole, its stages and jobs. No egui.
//!
//! And people's pictures, from GitHub and Gravatar, by their email (when the preferences ask).
//!
//! egit's own network connections are these alone; git makes every other. They go over
//! HTTPS through rustls on aws-lc-rs, checked against the system's trust store.

mod account;
mod answer;
mod api;
mod error;
mod github;
mod gitlab;
mod host;
mod http;
mod model;
mod pictures;
mod pipelines;
mod repository;
#[cfg(feature = "testing")]
pub mod testing;

pub use account::Account;
pub use api::{create_pull_request, pull_request_details, pull_requests, repository_info};
pub use error::ForgeError;
pub use host::{ForgeHost, ForgeHosts, ForgeKind};
pub use model::{
    Check, CheckState, Mergeable, NewPullRequest, Opened, PullRequest, PullRequestDetails,
    PullRequestPage, RepositoryInfo, Review, ReviewDecision, ReviewState,
};
pub use pictures::{Fetched, PictureSources, Pictures, Source as PictureSource};
pub use pipelines::{
    CiGroup, CiStatus, Job, Pipeline, PipelineDetails, Stage, has_pipelines, pipeline_details,
    pipelines,
};
pub use repository::{ForgeRepository, forge_repository, resolve_ssh_alias, resolve_ssh_alias_on};
