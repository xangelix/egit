//! GitHub's API against a fake on the loopback address, signed in with a sandbox's credential
//! helper: pull requests read a page at a time, details, the repository, opening a pull request
//! (and its refusal), and refused credentials (forgotten), no credentials, rate limits, and
//! repositories that aren't there. And Actions' workflow runs as pipelines: listed, each
//! workflow its series; one whole (its jobs grouped by matrix, each job's earlier attempts,
//! laid over by its workflow's needs); a job's state with its steps and its log through
//! GitHub's redirect (fetched without the token); re-runs, cancelling, and refusals; a run's
//! artifacts listed and downloaded (their digest checked; one expired).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::Arc;

use egit_forge::testing::{
    FakeArtifact, FakeForge, FakeJob, FakePipeline, FakePull, FakeRepository,
};
use egit_forge::{
    Account, ArtifactKind, CheckState, CiAction, CiStatus, ForgeError, ForgeHost, ForgeKind,
    ForgeRepository, JobLog, LogRead, NewPullRequest, ReviewDecision, ReviewState, Workflow,
    ci_act, create_pull_request, download_artifact, job, pipeline_details, pipelines,
    pull_request_details, pull_requests, read_log, repository_info, start_pipeline, workflows,
};
use egit_git::testing::Sandbox;
use egit_git::{CancelToken, ObjectId, Wait as _};

const TOKEN: &str = "t0ken";

/// A fake serving `pulls` open pull requests on `octo/tool`, and a sandbox whose credential
/// helper keeps `token` for it.
fn setup(pulls: u32, token: &str) -> (FakeForge, Sandbox, Account, ForgeRepository) {
    let fake = FakeForge::start(ForgeKind::GitHub, TOKEN, "ada").unwrap();
    fake.state().repositories.push(FakeRepository {
        path: "octo/tool".to_owned(),
        id: 1,
        default_branch: "main".to_owned(),
        parent: Some("upstream/tool".to_owned()),
        pipelines: Vec::new(),
        pulls: (0..pulls)
            .rev()
            .map(|number| FakePull {
                number: number + 1,
                title: format!("Change {}", number + 1),
                author: "bo".to_owned(),
                head_branch: format!("topic-{}", number + 1),
                base_branch: "main".to_owned(),
                head_commit: format!("{:040x}", number + 1),
                review: Some("APPROVED".to_owned()),
                checks: Some("FAILURE".to_owned()),
                ..FakePull::default()
            })
            .collect(),
    });
    let sandbox = Sandbox::new().unwrap();
    let file = sandbox.path("credentials");
    std::fs::write(&file, format!("http://ada:{token}@{}\n", fake.address())).unwrap();
    sandbox
        .run(
            sandbox.home(),
            "config",
            &[
                "--global",
                "credential.helper",
                &format!("store --file={}", file.display()),
            ],
        )
        .unwrap();
    let host = ForgeHost::parse(ForgeKind::GitHub, &fake.host_entry()).unwrap();
    let account = Account::new(sandbox.git().clone(), None, host.clone());
    let repository = ForgeRepository::new(host, "octo/tool").unwrap();
    (fake, sandbox, account, repository)
}

#[test]
fn pull_requests_come_a_page_at_a_time() {
    let (fake, _sandbox, account, repository) = setup(250, TOKEN);
    let mut pages = Vec::new();
    pull_requests(&account, &repository, 1_000, &CancelToken::new(), |page| {
        pages.push(page);
    })
    .wait()
    .unwrap();
    let sizes: Vec<usize> = pages.iter().map(|page| page.pulls.len()).collect();
    assert_eq!(sizes, [100, 100, 50]);
    assert_eq!(pages[0].total, 250);
    let first = &pages[0].pulls[0];
    assert_eq!(first.number, 250);
    assert_eq!(first.title, "Change 250");
    assert_eq!(first.head_branch, "topic-250");
    assert_eq!(first.review, ReviewDecision::Approved);
    assert_eq!(first.checks, CheckState::Failure);
    assert_eq!(
        first.head_commit,
        ObjectId::from_hex(format!("{:040x}", 250).as_bytes())
    );
    assert!(!first.cross_repository);
    assert_eq!(first.updated, 1_714_558_830);
    // Each request carried the token, and signed in as the host said.
    assert!(
        fake.state()
            .requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer t0ken"))
    );
}

#[test]
fn reading_stops_at_the_limit() {
    let (fake, _sandbox, account, repository) = setup(250, TOKEN);
    let mut read = 0;
    pull_requests(&account, &repository, 120, &CancelToken::new(), |page| {
        read += page.pulls.len();
    })
    .wait()
    .unwrap();
    assert_eq!(read, 120);
    let firsts: Vec<u64> = fake
        .state()
        .requests
        .iter()
        .map(|request| request.body["variables"]["first"].as_u64().unwrap())
        .collect();
    assert_eq!(firsts, [100, 20]);
}

#[test]
fn a_pull_request_s_details_and_the_repository_are_read() {
    let (fake, _sandbox, account, repository) = setup(3, TOKEN);
    {
        let mut state = fake.state();
        let pull = &mut state.repositories[0].pulls[0];
        pull.body = "Fixes everything.".to_owned();
        pull.check_runs = vec![
            (
                "build".to_owned(),
                "COMPLETED".to_owned(),
                Some("SUCCESS".to_owned()),
            ),
            ("lint".to_owned(), "IN_PROGRESS".to_owned(), None),
        ];
        drop(state);
    }
    let details = pull_request_details(&account, &repository, 3, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(details.body, "Fixes everything.");
    assert_eq!(details.checks.len(), 2);
    assert_eq!(details.checks[0].state, CheckState::Success);
    assert_eq!(details.checks[1].state, CheckState::Pending);
    assert_eq!(details.reviews[0].state, ReviewState::Approved);
    assert_eq!(details.labels, ["bug"]);
    assert!(matches!(
        pull_request_details(&account, &repository, 99, &CancelToken::new()).wait(),
        Err(ForgeError::NotFound { .. })
    ));

    let info = repository_info(&account, &repository, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(info.default_branch.as_deref(), Some("main"));
    let (parent, default) = info.parent.unwrap();
    assert_eq!(parent.path(), "upstream/tool");
    assert_eq!(parent.host(), repository.host());
    assert_eq!(default.as_deref(), Some("main"));
}

#[test]
fn a_pull_request_is_opened_and_a_second_for_its_branch_refused() {
    let (fake, _sandbox, account, repository) = setup(1, TOKEN);
    let new = NewPullRequest {
        title: "Add a feature".to_owned(),
        body: "It does things.".to_owned(),
        head_branch: "feature".to_owned(),
        head: Some(ForgeRepository::new(repository.host().clone(), "ada/tool").unwrap()),
        base_branch: "main".to_owned(),
        draft: true,
    };
    let opened = create_pull_request(&account, &repository, &new, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(opened.number, 2);
    let sent = fake.state().requests.last().unwrap().body.clone();
    assert_eq!(sent["head"], "ada:feature");
    assert_eq!(sent["base"], "main");
    assert_eq!(sent["title"], "Add a feature");
    assert_eq!(sent["draft"], true);
    match create_pull_request(&account, &repository, &new, &CancelToken::new()).wait() {
        Err(ForgeError::Status {
            status: 422,
            message,
            ..
        }) => assert_eq!(message, "A pull request already exists for ada:feature."),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_refused_credential_is_forgotten() {
    let (_fake, sandbox, account, repository) = setup(1, "wrong");
    let result = pull_requests(&account, &repository, 100, &CancelToken::new(), |_| {}).wait();
    assert!(matches!(result, Err(ForgeError::Refused { .. })));
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    assert!(!kept.contains("wrong"), "{kept}");
    // Then there is none, and nothing asks: still refused, saying why there is none.
    let result = pull_requests(&account, &repository, 100, &CancelToken::new(), |_| {}).wait();
    assert!(matches!(result, Err(ForgeError::Refused { .. })));
    // A fresh account (a new session) has none, and isn't signed in.
    let fresh = Account::new(sandbox.git().clone(), None, repository.host().clone());
    let result = pull_requests(&fresh, &repository, 100, &CancelToken::new(), |_| {}).wait();
    assert!(matches!(result, Err(ForgeError::NotSignedIn { .. })));
}

#[test]
fn signing_in_says_who_and_keeps_the_credential() {
    let (_fake, sandbox, account, _repository) = setup(0, TOKEN);
    std::fs::write(sandbox.path("credentials"), "").unwrap();
    // Nothing kept, and the prompt (unanswerable here) declined: not signed in.
    assert!(matches!(
        account.sign_in("Signing in", &CancelToken::new()).wait(),
        Err(ForgeError::NotSignedIn { .. })
    ));
    let (fake, sandbox, account, _repository) = setup(0, TOKEN);
    assert_eq!(
        account
            .sign_in("Signing in", &CancelToken::new())
            .wait()
            .unwrap()
            .login,
        "ada"
    );
    assert_eq!(account.login().as_deref(), Some("ada"));
    // The helper wrote it again, its own way (the port's colon encoded): approved.
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    let port = fake.address().rsplit_once(':').unwrap().1.to_owned();
    assert!(
        kept.contains(&format!("ada:t0ken@127.0.0.1%3a{port}")),
        "{kept}"
    );
}

#[test]
fn rate_limits_and_missing_repositories_say_so() {
    let (fake, _sandbox, account, repository) = setup(1, TOKEN);
    let other = ForgeRepository::new(repository.host().clone(), "octo/missing").unwrap();
    assert!(matches!(
        pull_requests(&account, &other, 100, &CancelToken::new(), |_| {}).wait(),
        Err(ForgeError::NotFound { .. })
    ));
    fake.state().rate_limited = true;
    match pull_requests(&account, &repository, 100, &CancelToken::new(), |_| {}).wait() {
        Err(ForgeError::RateLimited { reset, .. }) => assert_eq!(reset, Some(1_700_000_000)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_cancelled_read_stops() {
    let (_fake, _sandbox, account, repository) = setup(1, TOKEN);
    let cancel = CancelToken::new();
    cancel.cancel();
    let account = Arc::new(account);
    assert!(matches!(
        pull_requests(&account, &repository, 100, &cancel, |_| {}).wait(),
        Err(ForgeError::Cancelled)
    ));
}

#[test]
fn signing_in_asks_the_user_and_has_the_helper_keep_what_worked() {
    // An askpass program of this computer's: git elsewhere asks through its own.
    if egit_git::testing::on_another_computer() {
        return;
    }
    let (fake, sandbox, _, _) = setup(0, TOKEN);
    std::fs::write(sandbox.path("credentials"), "").unwrap();
    // The user, as git's askpass program asks them: a username, then a token as the password.
    let askpass = sandbox.path("askpass.sh");
    std::fs::write(
        &askpass,
        "#!/bin/sh\ncase \"$1\" in Username*) echo ada ;; *) echo t0ken ;; esac\n",
    )
    .unwrap();
    egit_git::testing::set_mode(&askpass, 0o700).unwrap();
    let git = sandbox
        .git_with([("GIT_ASKPASS", askpass.as_os_str())])
        .unwrap();
    let host = ForgeHost::parse(ForgeKind::GitHub, &fake.host_entry()).unwrap();
    let account = Account::new(git, None, host);
    assert_eq!(
        account
            .sign_in("Signing in", &CancelToken::new())
            .wait()
            .unwrap()
            .login,
        "ada"
    );
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    assert!(kept.contains("ada:t0ken@127.0.0.1"), "{kept}");
}

/// A job of a fake workflow run.
fn job_of(id: u64, name: &str, status: &str) -> FakeJob {
    FakeJob {
        id,
        name: name.to_owned(),
        status: status.to_owned(),
        duration: Some(42),
        steps: vec![
            ("Set up job".to_owned(), "SUCCESS".to_owned()),
            ("Run make".to_owned(), status.to_owned()),
        ],
        ..FakeJob::default()
    }
}

/// A run of workflow `workflow` (its name and id) on commit `commit`, as `status` says.
fn run_of(
    (id, number): (u64, u64),
    (workflow, workflow_id): (&str, u64),
    commit: u8,
    (status, jobs): (&str, Vec<FakeJob>),
) -> FakePipeline {
    FakePipeline {
        id,
        iid: number,
        sha: format!("{commit:040x}"),
        git_ref: "main".to_owned(),
        status: status.to_owned(),
        workflow: (
            workflow.to_owned(),
            workflow_id,
            format!(".github/workflows/{}.yml", workflow.to_lowercase()),
        ),
        stages: vec![(String::new(), String::new(), jobs)],
        ..FakePipeline::default()
    }
}

/// `octo/tool`'s CI run on commit 1: build passed, a matrix of tests (one failed), a deploy
/// that ran twice (its first attempt failed); and its Docs run, and CI on commit 2 running.
fn with_runs(fake: &FakeForge) {
    let mut deploy = job_of(705, "deploy", "SKIPPED");
    let mut first = job_of(604, "deploy", "FAILED");
    "the first attempt\n".clone_into(&mut first.log);
    deploy.earlier = vec![first];
    let mut failed = job_of(603, "test (macos)", "FAILED");
    "2024-05-01T10:21:00.1234567Z ##[group]Run make test\n\
                  2024-05-01T10:21:01.0000000Z \u{1b}[31mFAIL\u{1b}[0m parse\n\
                  2024-05-01T10:21:02.0000000Z ##[endgroup]\n\
                  2024-05-01T10:21:03.0000000Z ##[error]Process completed with exit code 1.\n"
        .clone_into(&mut failed.log);
    let ci = run_of(
        (7001, 57),
        ("CI", 11),
        1,
        (
            "FAILED",
            vec![
                job_of(601, "build", "SUCCESS"),
                job_of(602, "test (ubuntu)", "SUCCESS"),
                failed,
                deploy,
            ],
        ),
    );
    let docs = run_of(
        (7002, 9),
        ("Docs", 12),
        1,
        ("SUCCESS", vec![job_of(611, "pages", "SUCCESS")]),
    );
    let running = run_of(
        (7003, 58),
        ("CI", 11),
        2,
        ("RUNNING", vec![job_of(621, "build", "RUNNING")]),
    );
    fake.state().repositories[0].pipelines = vec![running, docs, ci];
}

const WORKFLOW: &str = "\
name: CI
on: push
jobs:
  build:
    runs-on: ubuntu-latest
  test:
    needs: build
    continue-on-error: true
    strategy:
      matrix:
        os: [ubuntu, macos]
    name: test
  deploy:
    name: deploy
    needs: [build, test]
";

#[test]
fn workflow_runs_are_pipelines_each_workflow_a_series_of_its_own() {
    let (fake, _sandbox, account, repository) = setup(0, TOKEN);
    with_runs(&fake);
    let mut read = Vec::new();
    pipelines(&account, &repository, 500, &CancelToken::new(), |page| {
        read.extend(page);
    })
    .wait()
    .unwrap();
    let said: Vec<(String, u64, CiStatus, u8)> = read
        .iter()
        .map(|run| {
            (
                run.title(),
                run.series,
                run.status,
                run.commit.as_bytes()[19],
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            ("CI #58".to_owned(), 11, CiStatus::Running, 2),
            ("Docs #9".to_owned(), 12, CiStatus::Success, 1),
            ("CI #57".to_owned(), 11, CiStatus::Failed, 1),
        ]
    );
    assert_eq!(read[2].iid, 7001);
    assert_eq!(read[2].git_ref.as_deref(), Some("main"));
    assert!(read[2].url.ends_with("/octo/tool/actions/runs/7001"));
}

#[test]
fn a_workflow_s_run_is_started_on_a_branch_and_found() {
    let (fake, _sandbox, account, repository) = setup(0, TOKEN);
    with_runs(&fake);
    let cancel = CancelToken::new();
    // The workflows, by name.
    let listed: Vec<(u64, String, String)> = workflows(&account, &repository, &cancel)
        .wait()
        .unwrap()
        .into_iter()
        .map(|workflow| (workflow.id, workflow.name, workflow.path))
        .collect();
    assert_eq!(
        listed,
        [
            (11, "CI".to_owned(), ".github/workflows/ci.yml".to_owned()),
            (
                12,
                "Docs".to_owned(),
                ".github/workflows/docs.yml".to_owned()
            ),
        ]
    );
    // Docs run on topic: GitHub makes the run a moment later, found and given back.
    let made = start_pipeline(&account, &repository, (Some(12), "topic"), &cancel)
        .wait()
        .unwrap();
    assert_eq!(made, Some(7004));
    let mut read = Vec::new();
    pipelines(&account, &repository, 500, &cancel, |page| {
        read.extend(page);
    })
    .wait()
    .unwrap();
    assert_eq!(
        (read[0].iid, read[0].git_ref.as_deref(), read[0].series),
        (7004, Some("topic"), 12)
    );
    // A workflow it can't start (no workflow_dispatch, or none such): GitHub's words.
    let refused = start_pipeline(&account, &repository, (Some(99), "topic"), &cancel).wait();
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("workflow_dispatch")),
        "{refused:?}"
    );
    // On GitHub, a run is of a workflow.
    assert!(
        start_pipeline(&account, &repository, (None, "topic"), &cancel)
            .wait()
            .is_err()
    );
}

#[test]
fn a_run_is_read_whole_its_matrix_together_and_laid_over_by_its_workflow() {
    let (fake, _sandbox, account, repository) = setup(0, TOKEN);
    with_runs(&fake);
    let mut details = pipeline_details(&account, &repository, 7001, &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(details.pipeline.title(), "CI #57");
    assert!(!details.ordered_stages && details.can_update);
    assert!(details.actions.retry && details.actions.rerun && !details.actions.cancel);
    assert_eq!(
        details.workflow.as_deref(),
        Some(".github/workflows/ci.yml")
    );
    assert_eq!(details.user.as_deref(), Some("ada"));
    let stages = |details: &egit_forge::PipelineDetails| -> Vec<(String, Vec<String>)> {
        details
            .stages
            .iter()
            .map(|stage| {
                (
                    stage.name.clone(),
                    stage.jobs.iter().map(|job| job.name.clone()).collect(),
                )
            })
            .collect()
    };
    assert_eq!(
        stages(&details),
        [
            ("build".to_owned(), vec!["build".to_owned()]),
            (
                "test".to_owned(),
                vec!["test (ubuntu)".to_owned(), "test (macos)".to_owned()]
            ),
            ("deploy".to_owned(), vec!["deploy".to_owned()]),
        ]
    );
    // The test stage failed; deploy's latest attempt was skipped, its first failed.
    assert_eq!(details.stages[1].status, CiStatus::Failed);
    let deploy = &details.stages[2].jobs[0];
    assert_eq!((deploy.id, deploy.status), (Some(705), CiStatus::Skipped));
    assert_eq!(deploy.earlier.len(), 1);
    assert_eq!(
        (deploy.earlier[0].id, deploy.earlier[0].status),
        (604, CiStatus::Failed)
    );
    assert!(deploy.actions.retry && !deploy.actions.cancel && !deploy.actions.play);
    // Without its workflow, a job waits for none; with it, as it says.
    assert_eq!(deploy.needs.as_deref(), Some(&[][..]));
    details.with_workflow(&Workflow::parse(WORKFLOW));
    let needs = |details: &egit_forge::PipelineDetails, name: &str| {
        let job = details
            .stages
            .iter()
            .flat_map(|stage| &stage.jobs)
            .find(|job| job.name == name)
            .unwrap();
        (job.needs.clone().unwrap(), job.allow_failure)
    };
    assert_eq!(needs(&details, "build"), (Vec::<String>::new(), false));
    assert_eq!(
        needs(&details, "test (macos)"),
        (vec!["build".to_owned()], true)
    );
    assert_eq!(
        needs(&details, "deploy").0,
        ["build", "test (ubuntu)", "test (macos)"]
    );
    assert_eq!(stages(&details).len(), 3);
}

#[test]
fn a_job_s_steps_and_log_are_read_the_log_through_github_s_redirect() {
    let (fake, _sandbox, account, repository) = setup(0, TOKEN);
    with_runs(&fake);
    let cancel = CancelToken::new();
    let failed = job(&account, &repository, 603, &cancel).wait().unwrap();
    assert_eq!(failed.status, CiStatus::Failed);
    assert_eq!(failed.duration, Some(42.0));
    assert_eq!(
        failed
            .steps
            .iter()
            .map(|step| (step.number, step.name.as_str(), step.status))
            .collect::<Vec<_>>(),
        [
            (1, "Set up job", CiStatus::Success),
            (2, "Run make", CiStatus::Failed)
        ]
    );
    let mut log = JobLog::github();
    let read = read_log(
        &account,
        &repository,
        (603, 0, true),
        &cancel,
        &mut |bytes| {
            log.feed(bytes);
        },
    )
    .wait()
    .unwrap();
    assert!(matches!(read, LogRead::Read { ranged: false, .. }));
    let parts = log.parts();
    assert_eq!(
        String::from_utf8(parts.text).unwrap(),
        "Run make test\nFAIL parse\nError: Process completed with exit code 1.\n"
    );
    assert_eq!(parts.sections.len(), 1);
    // The file, at the address GitHub pointed to, was fetched without the token.
    let state = fake.state();
    let file = state
        .requests
        .iter()
        .find(|request| request.path.starts_with("/blob/"))
        .unwrap();
    assert_eq!(file.authorization, None);
    drop(state);
    // A job under way has no log yet; an earlier attempt's is there.
    let missing = read_log(&account, &repository, (621, 0, true), &cancel, &mut |_| {})
        .wait()
        .unwrap();
    assert_eq!(missing, LogRead::Missing);
    let mut first = Vec::new();
    read_log(
        &account,
        &repository,
        (604, 0, true),
        &cancel,
        &mut |bytes| {
            first.extend_from_slice(bytes);
        },
    )
    .wait()
    .unwrap();
    assert_eq!(first, b"the first attempt\n");
}

#[test]
fn runs_are_re_run_and_cancelled_and_a_job_s_new_run_found() {
    let (fake, _sandbox, account, repository) = setup(0, TOKEN);
    with_runs(&fake);
    let cancel = CancelToken::new();
    let read = |id| {
        pipeline_details(&account, &repository, id, &CancelToken::new())
            .wait()
            .unwrap()
    };
    // A job re-run: its new run found in the run's new attempt.
    let again = ci_act(&account, &repository, CiAction::RetryJob(603), &cancel)
        .wait()
        .unwrap()
        .unwrap();
    assert_ne!(again, 603);
    let details = read(7001);
    assert_eq!(details.pipeline.attempt, 2);
    let macos = details
        .stages
        .iter()
        .flat_map(|stage| &stage.jobs)
        .find(|job| job.name == "test (macos)")
        .unwrap();
    assert_eq!(macos.id, Some(again));
    assert_eq!(macos.earlier[0].id, 603);
    // A run under way cancelled; then its failed jobs re-run, and all of them.
    ci_act(
        &account,
        &repository,
        CiAction::CancelPipeline(7003),
        &cancel,
    )
    .wait()
    .unwrap();
    assert_eq!(read(7003).pipeline.status, CiStatus::Canceled);
    ci_act(
        &account,
        &repository,
        CiAction::RetryPipeline(7003),
        &cancel,
    )
    .wait()
    .unwrap();
    let retried = read(7003);
    assert_eq!(retried.pipeline.status, CiStatus::Running);
    assert_eq!(retried.stages[0].jobs[0].earlier.len(), 1);
    ci_act(
        &account,
        &repository,
        CiAction::RerunPipeline(7002),
        &cancel,
    )
    .wait()
    .unwrap();
    assert_eq!(read(7002).pipeline.attempt, 2);
    // GitHub has no single job's cancel, nor a manual job; and a reader may not.
    assert!(matches!(
        ci_act(&account, &repository, CiAction::CancelJob(601), &cancel).wait(),
        Err(ForgeError::NotFound { .. })
    ));
    fake.state().read_only = true;
    assert!(matches!(
        ci_act(
            &account,
            &repository,
            CiAction::RerunPipeline(7002),
            &cancel
        )
        .wait(),
        Err(ForgeError::Forbidden { .. })
    ));
}

#[test]
fn a_token_given_is_checked_kept_under_its_login_and_signed_out() {
    let (fake, sandbox, account, repository) = setup(1, "old");
    let file = sandbox.path("credentials");
    // A mistyped token: refused, and what the helper kept stays.
    assert!(matches!(
        account
            .sign_in_with_token("mistyped", &CancelToken::new())
            .wait(),
        Err(ForgeError::Refused { .. })
    ));
    assert!(std::fs::read_to_string(&file).unwrap().contains(":old@"));

    // The right one: whose the host says, kept under that login.
    let signed = account
        .sign_in_with_token(&format!(" {TOKEN}\n"), &CancelToken::new())
        .wait()
        .unwrap();
    assert_eq!(signed.login, "ada");
    assert!(signed.kept);
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains(&format!("ada:{TOKEN}@"))
    );
    assert_eq!(
        account.who(&CancelToken::new()).wait().unwrap().as_deref(),
        Some("ada")
    );

    // Another account of the host takes it, whatever the helpers give first.
    let host = ForgeHost::parse(ForgeKind::GitHub, &fake.host_entry()).unwrap();
    let other = Account::new(sandbox.git().clone(), None, host.clone());
    other.adopt(&signed);
    pull_requests(&other, &repository, 10, &CancelToken::new(), |_| {})
        .wait()
        .unwrap();

    // Signed out: the helper forgets it, and nobody is signed in.
    account.sign_out(&CancelToken::new()).wait().unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains(TOKEN));
    other.signed_out();
    // The old one, for the same login, was replaced when the new one was kept.
    assert!(!std::fs::read_to_string(&file).unwrap().contains(":old@"));
    let fresh = Account::new(sandbox.git().clone(), None, host);
    assert_eq!(fresh.who(&CancelToken::new()).wait().unwrap(), None);
}

#[test]
fn a_token_no_helper_keeps_lasts_until_egit_quits() {
    let (fake, _sandbox, _, repository) = setup(1, TOKEN);
    let bare = egit_git::testing::Sandbox::new().unwrap();
    let host = ForgeHost::parse(ForgeKind::GitHub, &fake.host_entry()).unwrap();
    let account = Account::new(bare.git().clone(), None, host);
    let signed = account
        .sign_in_with_token(TOKEN, &CancelToken::new())
        .wait()
        .unwrap();
    assert!(!signed.kept);
    // Signed in all the same, for as long as the account lasts.
    pull_requests(&account, &repository, 10, &CancelToken::new(), |_| {})
        .wait()
        .unwrap();
}

#[test]
fn a_run_s_artifacts_are_listed_and_downloaded_their_digest_checked() {
    let (fake, _sandbox, account, repository) = setup(0, TOKEN);
    with_runs(&fake);
    fake.state().repositories[0].pipelines[2].artifacts = vec![
        FakeArtifact {
            id: 91,
            name: "dist-linux".to_owned(),
            contents: "PK the linux build".to_owned(),
            ..FakeArtifact::default()
        },
        FakeArtifact {
            id: 92,
            name: "coverage".to_owned(),
            contents: "PK old".to_owned(),
            expired: true,
            ..FakeArtifact::default()
        },
        FakeArtifact {
            id: 93,
            name: "changed".to_owned(),
            contents: "PK not what was listed".to_owned(),
            digest: format!("sha256:{}", "0".repeat(64)),
            ..FakeArtifact::default()
        },
    ];
    let details = pipeline_details(&account, &repository, 7001, &CancelToken::new())
        .wait()
        .unwrap();
    let artifacts = &details.artifacts;
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| (
                artifact.file_name.as_str(),
                artifact.kind,
                artifact.size,
                artifact.expired
            ))
            .collect::<Vec<_>>(),
        [
            ("changed.zip", ArtifactKind::Uploaded, Some(22), false),
            ("coverage.zip", ArtifactKind::Uploaded, Some(6), true),
            ("dist-linux.zip", ArtifactKind::Uploaded, Some(18), false),
        ]
    );
    let download = |which: usize| {
        let mut bytes = Vec::new();
        download_artifact(
            &account,
            &repository,
            &artifacts[which],
            &CancelToken::new(),
            &mut |part| {
                bytes.extend_from_slice(part);
                true
            },
        )
        .wait()
        .map(|_| String::from_utf8(bytes).unwrap())
    };
    assert_eq!(download(2).unwrap(), "PK the linux build");
    let asked = fake
        .state()
        .requests
        .iter()
        .find(|request| request.path == "/blob/artifact/gh/91")
        .map(|request| request.authorization.clone());
    assert_eq!(asked, Some(None));
    // Expired, GitHub says so; not what its digest says, refused.
    assert!(matches!(download(1), Err(ForgeError::Said { .. })));
    assert!(matches!(download(0), Err(ForgeError::Mismatched { .. })));
}
