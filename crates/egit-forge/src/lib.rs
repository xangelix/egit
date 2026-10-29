//! egit's interface to forges' web APIs: GitHub's (github.com and GitHub Enterprise Server)
//! and GitLab's (gitlab.com and self-managed). Which repository a remote is, signing in with the
//! credentials git's helpers keep, and pull requests (GitHub's) or merge requests (GitLab's):
//! read, detailed, and opened; and CI/CD pipelines, GitLab's and GitHub Actions' workflow
//! runs alike: each commit's, one whole (its stages and jobs, each job's earlier runs), what
//! can be asked of them, a job's log as it grows, and their artifacts downloaded. No egui.
//!
//! And people's pictures, from GitHub and Gravatar, by their email (when the preferences ask).
//!
//! egit's own network connections are these alone; git makes every other. They go over
//! HTTPS through rustls on aws-lc-rs, checked against the system's trust store.

mod account;
mod answer;
mod api;
mod artifacts;
mod error;
mod github;
mod gitlab;
#[cfg(not(target_arch = "wasm32"))]
pub mod here;
mod host;
mod http;
mod job_log;
mod model;
mod pictures;
mod pipelines;
mod repository;
#[cfg(feature = "testing")]
pub mod testing;

pub use account::{Account, SignedIn};
pub use api::{
    create_pull_request, pull_request_details, pull_requests, repository_info, ssh_host,
};
pub use artifacts::{Artifact, ArtifactKind, ArtifactSource, deliver_artifact, download_artifact};
pub use error::ForgeError;
pub use host::{ForgeHost, ForgeHosts, ForgeKind};
pub use http::Told;
pub use job_log::{
    JobLog, JobRun, JobStep, LogMark, LogParts, LogRead, MOST_TEXT, MarkKind, Run, Section, job,
    read_log, read_step_log, with_steps, with_steps_at,
};
pub use model::{
    Check, CheckState, Mergeable, NewPullRequest, Opened, PullRequest, PullRequestDetails,
    PullRequestPage, RepositoryInfo, Review, ReviewDecision, ReviewState,
};
pub use pictures::{Fetched, PictureSources, Pictures, Source as PictureSource};
pub use pipelines::{
    CiAction, CiGroup, CiStatus, EarlierRun, Job, JobActions, ListedPipeline, PageAfter, Pipeline,
    PipelineActions, PipelineDetails, PipelineListing, PipelinesPage, Stage, Workflow,
    WorkflowFile, ci_act, has_pipelines, pipeline_details, pipelines, pipelines_page,
    start_pipeline, workflows,
};
pub use repository::{
    ForgeRepository, forge_repository, resolve_ssh_alias, resolve_ssh_alias_on, ssh_host_of,
};
