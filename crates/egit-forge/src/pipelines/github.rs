//! GitHub Actions' workflow runs as pipelines, through GitHub's REST API (GraphQL has none):
//! a repository's latest runs (`GET /repos/:owner/:name/actions/runs`, the newest first), one
//! whole (`…/actions/runs/:id` and its jobs of every attempt, `…/runs/:id/jobs?filter=all`:
//! each job's latest run, the attempts before it as its earlier runs; a matrix's jobs, or a
//! reusable workflow's, grouped as a stage), re-running its failed jobs or all of them,
//! cancelling it, and re-running one job (GitHub cancels no single job, and has no manual
//! one). What each job needs is its workflow file's word ([`super::Workflow`]), read by the
//! caller.
//!
//! Whether the account may change runs is whether it may push to the repository (GitHub's
//! `permissions`), asked once in a while and remembered by the account.
//!
//! A job's state comes with its steps; its log is GitHub's redirect to a file of it, fetched
//! from there without the token (a credential never travels to another place).

use std::collections::HashMap;
use std::time::Duration;

use egit_git::{CancelToken, ObjectId};
use serde::Deserialize;

use super::{
    CiAction, CiStatus, EarlierRun, Job, JobActions, PAGE, Pipeline, PipelineActions,
    PipelineDetails, PipelineListing, grouped,
};
use crate::account::Account;
use crate::answer::{cancelled, not_found, parse_time, read_answer};
use crate::artifacts::{Artifact, ArtifactKind, ArtifactSource};
use crate::error::ForgeError;
use crate::http::Answer;
use crate::job_log::{JobRun, JobStep, LogRead};
use crate::repository::ForgeRepository;

/// The most pages of a run's jobs read: a thousand jobs, of every attempt.
const MOST_JOB_PAGES: u32 = 10;

/// How many times, a second apart, a job re-run is looked for in its run's new attempt.
const LOOKS_FOR_RERUN: u32 = 5;

/// The most of a log read: its file, sent whole.
const MOST_LOG: u64 = 512 * 1024 * 1024;

/// A workflow run, as GitHub gives it (its fields named as GitHub names them).
#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct Run {
    id: u64,
    name: Option<String>,
    head_branch: Option<String>,
    head_sha: Option<String>,
    path: Option<String>,
    run_number: Option<u64>,
    run_attempt: Option<u32>,
    event: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    workflow_id: Option<u64>,
    created_at: Option<String>,
    updated_at: Option<String>,
    run_started_at: Option<String>,
    html_url: Option<String>,
    actor: Option<Login>,
    triggering_actor: Option<Login>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct RunsPage {
    #[serde(default)]
    workflow_runs: Vec<Run>,
}

/// A job, as GitHub gives it.
#[derive(Deserialize)]
struct RestJob {
    id: u64,
    run_id: Option<u64>,
    run_attempt: Option<u32>,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    html_url: Option<String>,
    workflow_name: Option<String>,
    #[serde(default)]
    steps: Vec<RestStep>,
}

#[derive(Deserialize)]
struct RestStep {
    name: Option<String>,
    number: Option<u32>,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Deserialize)]
struct JobsPage {
    #[serde(default)]
    jobs: Vec<RestJob>,
}

#[derive(Deserialize)]
struct RepositoryPermissions {
    permissions: Option<Permissions>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_excessive_bools)]
struct Permissions {
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    maintain: bool,
    #[serde(default)]
    push: bool,
}

impl Run {
    fn status(&self) -> CiStatus {
        CiStatus::from_github(self.status.as_deref(), self.conclusion.as_deref())
    }

    /// Whether it has ended.
    fn ended(&self) -> bool {
        self.status.as_deref() == Some("completed")
    }

    /// The run as a pipeline, unless its commit isn't one.
    fn pipeline(&self, repository: &ForgeRepository) -> Option<Pipeline> {
        let commit = ObjectId::from_hex(self.head_sha.as_deref()?.as_bytes())?;
        Some(Pipeline {
            id: self.id,
            iid: self.id,
            name: self.name.clone().filter(|name| !name.is_empty()),
            number: self.run_number.unwrap_or(self.id),
            series: self.workflow_id.unwrap_or(0),
            attempt: self.run_attempt.unwrap_or(1).max(1),
            commit,
            status: self.status(),
            warnings: false,
            git_ref: self.head_branch.clone(),
            created: self.created_at.as_deref().and_then(parse_time).unwrap_or(0),
            url: self
                .html_url
                .clone()
                .unwrap_or_else(|| format!("{}/actions/runs/{}", repository.web_url(), self.id)),
        })
    }

    /// Its workflow's file in the repository (`.github/workflows/ci.yml`); none for one GitHub
    /// makes itself (`dynamic/…`: Pages, code scanning's default setup).
    fn workflow(&self) -> Option<String> {
        let path = self.path.as_deref()?;
        let path = path.split('@').next().unwrap_or(path);
        path.starts_with(".github/").then(|| path.to_owned())
    }
}

impl RestJob {
    fn status(&self) -> CiStatus {
        CiStatus::from_github(self.status.as_deref(), self.conclusion.as_deref())
    }

    fn started(&self) -> Option<i64> {
        self.started_at.as_deref().and_then(parse_time)
    }

    fn finished(&self) -> Option<i64> {
        self.completed_at.as_deref().and_then(parse_time)
    }

    /// How long it ran, once it ended.
    fn duration(&self) -> Option<u64> {
        let (started, finished) = (self.started()?, self.finished()?);
        u64::try_from(finished.saturating_sub(started)).ok()
    }

    fn earlier(&self) -> EarlierRun {
        EarlierRun {
            id: self.id,
            status: self.status(),
            duration: self.duration(),
            started: self.started(),
            url: self.html_url.clone(),
        }
    }
}

/// The stage a job is of, by its name alone: its matrix's values (`test (ubuntu, 1.80)`) and a
/// reusable workflow's job (`build / compile`) left out.
pub(super) fn group_of(name: &str) -> &str {
    let caller = name.split(" / ").next().unwrap_or(name);
    caller.split(" (").next().unwrap_or(caller).trim()
}

/// `path` of `repository` in GitHub's REST API (`/actions/runs`).
fn rest(account: &Account, repository: &ForgeRepository, path: &str) -> String {
    account
        .host()
        .rest_url(&format!("/repos/{}{path}", repository.path()))
}

/// GETs `url` and reads its JSON.
fn get<T: serde::de::DeserializeOwned>(
    account: &Account,
    url: &str,
    what: &str,
    cancel: &CancelToken,
) -> Result<T, ForgeError> {
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    let answer = account.send(url, None, cancel)?;
    read_answer(account, &answer, what)
}

/// Whether `answer` says yes (any success: GitHub's are often empty), else its error.
fn succeeded(account: &Account, answer: &Answer, what: &str) -> Result<(), ForgeError> {
    if (200..300).contains(&answer.status) {
        return Ok(());
    }
    read_answer::<serde_json::Value>(account, answer, what).map(|_| ())
}

/// Reads `repository`'s latest workflow runs, the newest first, a page at a time.
pub(super) fn pipelines(
    account: &Account,
    repository: &ForgeRepository,
    limit: u32,
    cancel: &CancelToken,
    found: &mut dyn FnMut(Vec<Pipeline>),
) -> Result<PipelineListing, ForgeError> {
    let what = format!("{}'s workflow runs", repository.full_name());
    let mut read = 0_u32;
    let mut page = 1_u32;
    loop {
        let per_page = PAGE.min(limit.saturating_sub(read));
        if per_page == 0 {
            return Ok(PipelineListing::Listed);
        }
        let url = rest(
            account,
            repository,
            &format!("/actions/runs?per_page={PAGE}&page={page}&exclude_pull_requests=true"),
        );
        let runs: RunsPage = match get(account, &url, &what, cancel) {
            Ok(runs) => runs,
            // Actions the account can't see (or turned off) answer as no repository.
            Err(ForgeError::NotFound { .. }) if read == 0 => {
                tracing::debug!(repository = %repository.full_name(), "GitHub shows no Actions");
                return Ok(PipelineListing::Hidden);
            }
            Err(error) => return Err(error),
        };
        let given = runs.workflow_runs.len();
        let pipelines: Vec<Pipeline> = runs
            .workflow_runs
            .iter()
            .take(usize::try_from(per_page).unwrap_or(usize::MAX))
            .filter_map(|run| run.pipeline(repository))
            .collect();
        read = read.saturating_add(u32::try_from(pipelines.len()).unwrap_or(u32::MAX));
        found(pipelines);
        if given < usize::try_from(PAGE).unwrap_or(usize::MAX) {
            return Ok(PipelineListing::Listed);
        }
        page = page.saturating_add(1);
    }
}

/// Reads workflow run `id` of `repository` whole.
pub(super) fn pipeline_details(
    account: &Account,
    repository: &ForgeRepository,
    id: u64,
    cancel: &CancelToken,
) -> Result<PipelineDetails, ForgeError> {
    let what = format!("{}'s workflow run {id}", repository.full_name());
    let run: Run = get(
        account,
        &rest(account, repository, &format!("/actions/runs/{id}")),
        &what,
        cancel,
    )?;
    let pipeline = run
        .pipeline(repository)
        .ok_or_else(|| ForgeError::Malformed {
            host: account.host().name().to_owned(),
            detail: format!("{what} names no commit"),
        })?;
    let mut all = Vec::new();
    for page in 1..=MOST_JOB_PAGES {
        let url = rest(
            account,
            repository,
            &format!("/actions/runs/{id}/jobs?filter=all&per_page={PAGE}&page={page}"),
        );
        let read: JobsPage = get(account, &url, &what, cancel)?;
        let count = read.jobs.len();
        all.extend(read.jobs);
        if count < usize::try_from(PAGE).unwrap_or(usize::MAX) {
            break;
        }
    }
    let may = may_push(account, repository, cancel).unwrap_or_else(|error| {
        // Unknown: offered, and GitHub's refusal said where it refuses.
        tracing::debug!(%error, "what the account may do in the repository is unknown");
        true
    });
    let ended = run.ended();
    let jobs = latest_jobs(all, ended, may);
    let retryable = ended
        && jobs
            .iter()
            .any(|job| matches!(job.status, CiStatus::Failed | CiStatus::Canceled));
    let stages = grouped(jobs, |job| {
        let group = group_of(&job.name);
        (format!("?{group}"), group.to_owned())
    });
    let time = |text: &Option<String>| text.as_deref().and_then(parse_time);
    Ok(PipelineDetails {
        pipeline,
        started: time(&run.run_started_at).or_else(|| time(&run.created_at)),
        finished: ended.then(|| time(&run.updated_at)).flatten(),
        duration: None,
        user: run
            .triggering_actor
            .as_ref()
            .or(run.actor.as_ref())
            .map(|actor| actor.login.clone()),
        source: run.event.clone(),
        stages,
        actions: PipelineActions {
            retry: retryable,
            cancel: !ended,
            rerun: ended,
        },
        can_update: may,
        ordered_stages: false,
        workflow: run.workflow(),
        artifacts: run_artifacts(account, repository, id, cancel)?,
    })
}

/// A GitHub artifact, as GitHub lists a run's.
#[derive(Deserialize)]
struct RestArtifact {
    id: u64,
    name: Option<String>,
    size_in_bytes: Option<u64>,
    #[serde(default)]
    expired: bool,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct ArtifactsPage {
    #[serde(default)]
    artifacts: Vec<RestArtifact>,
}

/// What run `id` uploaded (its first hundred artifacts, by name).
fn run_artifacts(
    account: &Account,
    repository: &ForgeRepository,
    id: u64,
    cancel: &CancelToken,
) -> Result<Vec<Artifact>, ForgeError> {
    let what = format!("{}'s workflow run {id}'s artifacts", repository.full_name());
    let url = rest(
        account,
        repository,
        &format!("/actions/runs/{id}/artifacts?per_page={PAGE}"),
    );
    let page: ArtifactsPage = get(account, &url, &what, cancel)?;
    let mut artifacts: Vec<Artifact> = page
        .artifacts
        .into_iter()
        .map(|artifact| {
            let name = artifact.name.unwrap_or_else(|| artifact.id.to_string());
            Artifact {
                file_name: format!("{name}.zip"),
                name,
                kind: ArtifactKind::Uploaded,
                size: artifact.size_in_bytes,
                expired: artifact.expired,
                digest: artifact.digest,
                source: ArtifactSource::GitHub { id: artifact.id },
            }
        })
        .collect();
    artifacts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(artifacts)
}

/// Each job's latest run (by its name: the highest attempt), in the order they were made, the
/// attempts before it as its earlier runs.
fn latest_jobs(all: Vec<RestJob>, ended: bool, may: bool) -> Vec<Job> {
    let mut by_name: HashMap<String, Vec<RestJob>> = HashMap::new();
    for job in all {
        by_name
            .entry(job.name.clone().unwrap_or_default())
            .or_default()
            .push(job);
    }
    let mut jobs: Vec<(u64, Job)> = by_name
        .into_iter()
        .filter_map(|(name, mut runs)| {
            runs.sort_by_key(|run| std::cmp::Reverse((run.run_attempt.unwrap_or(1), run.id)));
            let mut runs = runs.into_iter();
            let latest = runs.next()?;
            let status = latest.status();
            let job = Job {
                id: Some(latest.id),
                name,
                status,
                allow_failure: false,
                manual: false,
                duration: latest.duration(),
                started: latest.started(),
                url: latest.html_url.clone(),
                // Its workflow's file says; none known: it waits for none.
                needs: Some(Vec::new()),
                actions: JobActions {
                    retry: ended && latest.status.as_deref() == Some("completed"),
                    cancel: false,
                    play: false,
                },
                can_update: may,
                earlier: runs.map(|run| run.earlier()).collect(),
                artifacts: Vec::new(),
            };
            // The first attempt's number orders it among the others.
            let first = job.earlier.last().map_or(latest.id, |run| run.id);
            Some((first, job))
        })
        .collect();
    jobs.sort_by_key(|(first, _)| *first);
    jobs.into_iter().map(|(_, job)| job).collect()
}

/// Whether the account may push to `repository` (and so re-run and cancel its runs), as GitHub
/// says, remembered a while.
fn may_push(
    account: &Account,
    repository: &ForgeRepository,
    cancel: &CancelToken,
) -> Result<bool, ForgeError> {
    if let Some(known) = account.remembered_push(repository.path()) {
        return Ok(known);
    }
    let what = repository.full_name();
    let read: RepositoryPermissions = get(account, &rest(account, repository, ""), &what, cancel)?;
    let may = read
        .permissions
        .is_some_and(|may| may.push || may.maintain || may.admin);
    account.remember_push(repository.path(), may);
    Ok(may)
}

/// Asks `action` of `repository`'s GitHub: a run's failed jobs re-run
/// (`POST …/runs/:id/rerun-failed-jobs`), all of them (`…/rerun`), the run cancelled
/// (`…/cancel`), or a job re-run (`…/jobs/:id/rerun`; its new run looked for in the run's new
/// attempt, a second apart a few times, and returned).
pub(super) fn ci_act(
    account: &Account,
    repository: &ForgeRepository,
    action: CiAction,
    cancel: &CancelToken,
) -> Result<Option<u64>, ForgeError> {
    let what = format!("{}'s {action:?}", repository.full_name());
    let post = |path: String| -> Result<(), ForgeError> {
        let answer = account.send(&rest(account, repository, &path), Some(b"{}"), cancel)?;
        succeeded(account, &answer, &what)
    };
    match action {
        CiAction::RetryPipeline(id) => {
            post(format!("/actions/runs/{id}/rerun-failed-jobs"))?;
            Ok(Some(id))
        }
        CiAction::RerunPipeline(id) => {
            post(format!("/actions/runs/{id}/rerun"))?;
            Ok(Some(id))
        }
        CiAction::CancelPipeline(id) => {
            post(format!("/actions/runs/{id}/cancel"))?;
            Ok(Some(id))
        }
        CiAction::RetryJob(id) => {
            let job: RestJob = get(
                account,
                &rest(account, repository, &format!("/actions/jobs/{id}")),
                &what,
                cancel,
            )?;
            post(format!("/actions/jobs/{id}/rerun"))?;
            rerun_of(account, repository, &job, cancel)
        }
        CiAction::CancelJob(_) | CiAction::PlayJob(_) => Err(not_found(account, what)),
    }
}

/// The run of `job` its re-run made, once its run's latest attempt has one; none found after a
/// few looks.
fn rerun_of(
    account: &Account,
    repository: &ForgeRepository,
    job: &RestJob,
    cancel: &CancelToken,
) -> Result<Option<u64>, ForgeError> {
    let Some(run) = job.run_id else {
        return Ok(None);
    };
    let what = format!("{}'s workflow run {run}", repository.full_name());
    for look in 0..LOOKS_FOR_RERUN {
        if look > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let url = rest(
            account,
            repository,
            &format!("/actions/runs/{run}/jobs?filter=latest&per_page={PAGE}"),
        );
        let read: JobsPage = get(account, &url, &what, cancel)?;
        if let Some(again) = read
            .jobs
            .iter()
            .find(|again| again.name == job.name && again.id != job.id)
        {
            return Ok(Some(again.id));
        }
    }
    Ok(None)
}

/// How GitHub job `id` stands, with its steps.
pub(crate) fn github_job(
    account: &Account,
    repository: &ForgeRepository,
    id: u64,
    cancel: &CancelToken,
) -> Result<JobRun, ForgeError> {
    let what = format!("{} job {id}", repository.full_name());
    let job: RestJob = get(
        account,
        &rest(account, repository, &format!("/actions/jobs/{id}")),
        &what,
        cancel,
    )?;
    let status = job.status();
    let failure = match job.conclusion.as_deref() {
        Some("timed_out") => Some("timed out".to_owned()),
        Some("startup_failure") => Some("it couldn't start".to_owned()),
        _ => None,
    };
    let steps = job
        .steps
        .iter()
        .map(|step| JobStep {
            number: step.number.unwrap_or(0),
            name: step.name.clone().unwrap_or_default(),
            status: CiStatus::from_github(step.status.as_deref(), step.conclusion.as_deref()),
            started: step.started_at.as_deref().and_then(parse_time),
            finished: step.completed_at.as_deref().and_then(parse_time),
        })
        .collect();
    Ok(JobRun {
        id: job.id,
        name: job.name.clone().unwrap_or_default(),
        stage: job.workflow_name.clone(),
        status,
        started: job.started(),
        finished: job.finished(),
        duration: job
            .duration()
            .map(|seconds| f64::from(u32::try_from(seconds).unwrap_or(u32::MAX))),
        failure,
        url: job.html_url,
        steps,
    })
}

/// Reads GitHub job `id`'s log after its first `from` bytes, handing the new ones to `sink`:
/// GitHub answers with a redirect to a file of it (its whole, as far as it is written), fetched
/// without the token; nothing yet is [`LogRead::Missing`].
pub(crate) fn github_log(
    account: &Account,
    repository: &ForgeRepository,
    (id, from): (u64, u64),
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&[u8]),
) -> Result<LogRead, ForgeError> {
    let what = format!("{} job {id}'s log", repository.full_name());
    let answer = account.send(
        &rest(account, repository, &format!("/actions/jobs/{id}/logs")),
        None,
        cancel,
    )?;
    let (mut seen, mut given) = (0_u64, 0_u64);
    let mut take = |part: &[u8]| {
        let length = u64::try_from(part.len()).unwrap_or(u64::MAX);
        let skip = usize::try_from(from.saturating_sub(seen)).unwrap_or(usize::MAX);
        seen = seen.saturating_add(length);
        if let Some(new) = part.get(skip..).filter(|new| !new.is_empty()) {
            given = given.saturating_add(u64::try_from(new.len()).unwrap_or(u64::MAX));
            sink(new);
        }
    };
    let status = match (answer.status, answer.location.as_deref()) {
        (301 | 302 | 303 | 307 | 308, Some(location)) => {
            let mut kept = 0_u64;
            let streamed = account.download_public(
                location,
                &mut |part| {
                    kept = kept.saturating_add(u64::try_from(part.len()).unwrap_or(u64::MAX));
                    take(part);
                    kept < MOST_LOG
                },
                cancel,
            )?;
            if !(200..300).contains(&streamed.status) && streamed.status != 404 {
                return Err(ForgeError::Status {
                    host: account.host().name().to_owned(),
                    status: streamed.status,
                    message: crate::answer::reason(&streamed.said),
                });
            }
            streamed.status
        }
        (200..=299, _) => {
            take(&answer.body);
            answer.status
        }
        (404 | 410, _) => 404,
        _ => {
            succeeded(account, &answer, &what)?;
            404
        }
    };
    Ok(match status {
        404 => LogRead::Missing,
        _ if seen < from => LogRead::Shorter,
        _ => LogRead::Read {
            new: given,
            ranged: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_s_stage_is_its_name_without_its_matrix_or_called_job() {
        assert_eq!(group_of("build"), "build");
        assert_eq!(group_of("test (ubuntu-latest, 1.80)"), "test");
        assert_eq!(group_of("release / publish (pypi)"), "release");
    }
}
