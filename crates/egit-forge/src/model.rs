//! What the forges' APIs give, alike for each: pull requests (GitHub's) and merge requests
//! (GitLab's), their details, checks, and reviews; a repository's default branch and the one it
//! was forked from; a request to open, and what was opened.

use egit_git::ObjectId;

use crate::repository::ForgeRepository;

/// A pull request (GitHub's) or merge request (GitLab's), as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// Its number in the repository.
    pub number: u32,
    /// Its title.
    pub title: String,
    /// Its author's login (`ghost` for a deleted account).
    pub author: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// The branch it brings in.
    pub head_branch: String,
    /// The repository that branch is in (`owner/name`), when known (a deleted fork isn't).
    pub head_repository: Option<String>,
    /// Whether that repository is another than the base's (a fork).
    pub cross_repository: bool,
    /// The branch it goes into.
    pub base_branch: String,
    /// Its head commit.
    pub head_commit: Option<ObjectId>,
    /// Where its reviews stand.
    pub review: ReviewDecision,
    /// Its head commit's checks, all together.
    pub checks: CheckState,
    /// When it last changed (seconds since the Unix epoch).
    pub updated: i64,
    /// Its web page.
    pub url: String,
}

/// Where a pull request's reviews stand, as the repository's rules see them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewDecision {
    /// No review is required, or none is known.
    #[default]
    None,
    /// A review is required and none approved yet.
    Required,
    /// Approved.
    Approved,
    /// A reviewer asked for changes.
    ChangesRequested,
}

/// A check's state (or a commit's, all its checks together).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckState {
    /// No checks.
    #[default]
    None,
    /// Skipped, neutral, or stale: neither passed nor failed.
    Neutral,
    /// Passed.
    Success,
    /// Queued or running.
    Pending,
    /// Failed (or errored, timed out, was cancelled, needs action).
    Failure,
}

/// A page of a repository's open pull requests.
#[derive(Clone, Debug, Default)]
pub struct PullRequestPage {
    /// The pull requests, the most recently updated first.
    pub pulls: Vec<PullRequest>,
    /// How many are open in all.
    pub total: u32,
    /// Where the next page starts, if there is one.
    pub next: Option<String>,
}

/// A pull request's details, read when it is looked at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequestDetails {
    /// Its description, as written (Markdown).
    pub body: String,
    /// Lines added and removed, and files changed.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// Files changed.
    pub changed_files: u32,
    /// Its commits.
    pub commits: u32,
    /// Whether it merges cleanly.
    pub mergeable: Mergeable,
    /// Its head commit's checks, one by one.
    pub checks: Vec<Check>,
    /// Each reviewer's latest review that approved or asked for changes.
    pub reviews: Vec<Review>,
    /// Its labels.
    pub labels: Vec<String>,
}

/// Whether a pull request merges cleanly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mergeable {
    /// Not known yet (GitHub works it out in the background).
    #[default]
    Unknown,
    /// It merges cleanly.
    Yes,
    /// It conflicts with its base.
    Conflicting,
}

/// One check of a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Its name.
    pub name: String,
    /// Its state.
    pub state: CheckState,
    /// Its page, if it has one.
    pub url: Option<String>,
}

/// A reviewer's latest opinion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// The reviewer's login.
    pub author: String,
    /// What they said.
    pub state: ReviewState,
}

/// What a review said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    /// Approved.
    Approved,
    /// Asked for changes.
    ChangesRequested,
    /// Commented.
    Commented,
    /// Was dismissed.
    Dismissed,
    /// Isn't submitted yet.
    Pending,
}

/// A repository's default branch, and the repository it was forked from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryInfo {
    /// Its default branch (none in an empty repository).
    pub default_branch: Option<String>,
    /// The repository it was forked from, on the same host, and that one's default branch.
    pub parent: Option<(ForgeRepository, Option<String>)>,
}

/// A pull request to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPullRequest {
    /// Its title.
    pub title: String,
    /// Its description.
    pub body: String,
    /// The branch it brings in.
    pub head_branch: String,
    /// The repository that branch is in, when it isn't the one the request goes into (a fork).
    pub head: Option<ForgeRepository>,
    /// The branch it goes into.
    pub base_branch: String,
    /// Whether it is a draft.
    pub draft: bool,
}

/// A pull request opened: its number and page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// Its number.
    pub number: u32,
    /// Its web page.
    pub url: String,
}
