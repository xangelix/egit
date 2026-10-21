//! GitLab's API against a fake on the loopback address, signed in with a sandbox's credential
//! helper: merge requests read a page at a time from a project in nested groups, details, the
//! project and the one it was forked from, opening a merge request (in the project, and from a
//! fork; a draft by its title) and its refusal, and refused credentials (forgotten), no
//! credentials, rate limits, projects that aren't there, and signing in; pipelines a page at a
//! time, and one whole (its stages in order, its jobs and their pages).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use egit_forge::testing::{FakeForge, FakeJob, FakePipeline, FakePull, FakeRepository};
use egit_forge::{
    Account, CheckState, CiGroup, CiStatus, ForgeError, ForgeHost, ForgeKind, ForgeRepository,
    Mergeable, NewPullRequest, PipelineListing, ReviewDecision, ReviewState, create_pull_request,
    pipeline_details, pipelines, pull_request_details, pull_requests, repository_info, ssh_host,
};
use egit_git::testing::Sandbox;
use egit_git::{CancelToken, ObjectId};

const TOKEN: &str = "glpat-t0ken";

/// A fake GitLab serving `requests` open merge requests on `team/sub/tool` (and a fork of it,
/// `ada/tool`), and a sandbox whose credential helper keeps `token` for it.
fn setup(requests: u32, token: &str) -> (FakeForge, Sandbox, Account, ForgeRepository) {
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    fake.state().repositories.extend([
        FakeRepository {
            path: "team/sub/tool".to_owned(),
            id: 41,
            default_branch: "main".to_owned(),
            parent: None,
            pipelines: Vec::new(),
            pulls: (0..requests)
                .rev()
                .map(|number| FakePull {
                    number: number + 1,
                    title: format!("Change {}", number + 1),
                    author: "bo".to_owned(),
                    head_branch: format!("topic-{}", number + 1),
                    base_branch: "main".to_owned(),
                    head_commit: format!("{:040x}", number + 1),
                    review: Some("APPROVED".to_owned()),
                    checks: Some("FAILED".to_owned()),
                    ..FakePull::default()
                })
                .collect(),
        },
        FakeRepository {
            path: "ada/tool".to_owned(),
            id: 42,
            default_branch: "main".to_owned(),
            parent: Some("team/sub/tool".to_owned()),
            pipelines: Vec::new(),
            pulls: Vec::new(),
        },
    ]);
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
    let host = ForgeHost::parse(ForgeKind::GitLab, &fake.host_entry()).unwrap();
    let account = Account::new(sandbox.git().clone(), None, host.clone());
    let project = ForgeRepository::new(host, "team/sub/tool").unwrap();
    (fake, sandbox, account, project)
}

#[test]
fn merge_requests_come_a_page_at_a_time() {
    let (fake, _sandbox, account, project) = setup(150, TOKEN);
    let mut pages = Vec::new();
    pull_requests(&account, &project, 1_000, &CancelToken::new(), |page| {
        pages.push(page);
    })
    .unwrap();
    let sizes: Vec<usize> = pages.iter().map(|page| page.pulls.len()).collect();
    assert_eq!(sizes, [100, 50]);
    assert_eq!(pages[0].total, 150);
    let first = &pages[0].pulls[0];
    assert_eq!(first.number, 150);
    assert_eq!(first.title, "Change 150");
    assert_eq!(first.head_branch, "topic-150");
    assert_eq!(first.review, ReviewDecision::Approved);
    assert_eq!(first.checks, CheckState::Failure);
    assert_eq!(
        first.head_commit,
        ObjectId::from_hex(format!("{:040x}", 150).as_bytes())
    );
    assert!(!first.cross_repository);
    assert_eq!(first.updated, 1_714_558_830);
    let requests = fake.state().requests.clone();
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer glpat-t0ken"))
    );
    assert_eq!(requests[0].body["variables"]["path"], "team/sub/tool");
}

#[test]
fn a_merge_request_s_details_and_the_project_are_read() {
    let (fake, _sandbox, account, project) = setup(3, TOKEN);
    {
        let mut state = fake.state();
        let pull = &mut state.repositories[0].pulls[0];
        pull.body = "Fixes everything.".to_owned();
        pull.check_runs = vec![
            ("build".to_owned(), "SUCCESS".to_owned(), None),
            ("lint".to_owned(), "RUNNING".to_owned(), None),
        ];
        drop(state);
    }
    let details = pull_request_details(&account, &project, 3, &CancelToken::new()).unwrap();
    assert_eq!(details.body, "Fixes everything.");
    assert_eq!(
        (details.additions, details.deletions, details.changed_files),
        (10, 2, 3)
    );
    assert_eq!(details.mergeable, Mergeable::Yes);
    assert_eq!(details.checks.len(), 2);
    assert_eq!(details.checks[0].state, CheckState::Success);
    assert_eq!(details.checks[1].state, CheckState::Pending);
    assert_eq!(
        details.checks[0].url.as_deref(),
        Some(format!("http://{}/team/sub/tool/-/jobs/build", fake.address()).as_str())
    );
    assert_eq!(details.reviews[0].state, ReviewState::Approved);
    assert_eq!(details.labels, ["bug"]);
    assert!(matches!(
        pull_request_details(&account, &project, 99, &CancelToken::new()),
        Err(ForgeError::NotFound { .. })
    ));

    let info = repository_info(&account, &project, &CancelToken::new()).unwrap();
    assert_eq!(info.default_branch.as_deref(), Some("main"));
    assert_eq!(info.parent, None);
    let fork = ForgeRepository::new(project.host().clone(), "ada/tool").unwrap();
    let info = repository_info(&account, &fork, &CancelToken::new()).unwrap();
    let (parent, default) = info.parent.unwrap();
    assert!(parent.is(&project));
    assert_eq!(default.as_deref(), Some("main"));
}

#[test]
fn a_merge_request_is_opened_from_the_project_and_from_a_fork() {
    let (fake, _sandbox, account, project) = setup(1, TOKEN);
    let new = NewPullRequest {
        title: "Add a feature".to_owned(),
        body: "It does things.".to_owned(),
        head_branch: "feature".to_owned(),
        head: None,
        base_branch: "main".to_owned(),
        draft: true,
    };
    let opened = create_pull_request(&account, &project, &new, &CancelToken::new()).unwrap();
    assert_eq!(opened.number, 2);
    let sent = fake.state().requests.last().unwrap().clone();
    assert_eq!(sent.method, "POST");
    assert_eq!(
        sent.path,
        "/api/v4/projects/team%2Fsub%2Ftool/merge_requests"
    );
    assert_eq!(sent.body["source_branch"], "feature");
    assert_eq!(sent.body["target_branch"], "main");
    assert_eq!(sent.body["title"], "Draft: Add a feature");
    assert_eq!(sent.body["description"], "It does things.");
    assert!(sent.body.get("target_project_id").is_none());
    match create_pull_request(&account, &project, &new, &CancelToken::new()) {
        Err(ForgeError::Status {
            status: 409,
            message,
            ..
        }) => assert_eq!(
            message,
            "Another open merge request already exists for this source branch: !2"
        ),
        other => panic!("{other:?}"),
    }

    // From the fork: made in the fork, aimed at the project by its id.
    let fork = ForgeRepository::new(project.host().clone(), "ada/tool").unwrap();
    let from_fork = NewPullRequest {
        head: Some(fork),
        draft: false,
        ..new
    };
    let opened = create_pull_request(&account, &project, &from_fork, &CancelToken::new()).unwrap();
    assert_eq!(opened.number, 3);
    let sent = fake.state().requests.last().unwrap().clone();
    assert_eq!(sent.path, "/api/v4/projects/ada%2Ftool/merge_requests");
    assert_eq!(sent.body["target_project_id"], 41);
    assert_eq!(sent.body["title"], "Add a feature");
    let listed = fake.state().repositories[0].pulls[0].clone();
    assert_eq!(listed.fork.as_deref(), Some("ada/tool"));
}

#[test]
fn a_fork_s_merge_request_is_from_another_project() {
    let (fake, _sandbox, account, project) = setup(0, TOKEN);
    fake.state().repositories[0].pulls.push(FakePull {
        number: 7,
        title: "Fork change".to_owned(),
        author: "cy".to_owned(),
        head_branch: "feature".to_owned(),
        fork: Some("cy/tool".to_owned()),
        base_branch: "main".to_owned(),
        review: Some("REVIEW_REQUIRED".to_owned()),
        checks: Some("RUNNING".to_owned()),
        ..FakePull::default()
    });
    let mut read = Vec::new();
    pull_requests(&account, &project, 100, &CancelToken::new(), |page| {
        read.extend(page.pulls);
    })
    .unwrap();
    assert!(read[0].cross_repository);
    assert_eq!(read[0].head_repository.as_deref(), Some("cy/tool"));
    assert_eq!(read[0].review, ReviewDecision::Required);
    assert_eq!(read[0].checks, CheckState::Pending);
}

#[test]
fn refused_credentials_rate_limits_and_missing_projects_say_so() {
    let (_fake, sandbox, account, project) = setup(1, "wrong");
    let result = pull_requests(&account, &project, 100, &CancelToken::new(), |_| {});
    assert!(matches!(result, Err(ForgeError::Refused { .. })));
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    assert!(!kept.contains("wrong"), "{kept}");

    let (fake, _sandbox, account, project) = setup(1, TOKEN);
    let missing = ForgeRepository::new(project.host().clone(), "team/missing").unwrap();
    assert!(matches!(
        pull_requests(&account, &missing, 100, &CancelToken::new(), |_| {}),
        Err(ForgeError::NotFound { .. })
    ));
    fake.state().rate_limited = true;
    match pull_requests(&account, &project, 100, &CancelToken::new(), |_| {}) {
        Err(ForgeError::RateLimited { reset, .. }) => assert_eq!(reset, Some(1_700_000_000)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn signing_in_says_who() {
    let (_fake, _sandbox, account, _project) = setup(0, TOKEN);
    assert_eq!(
        account
            .sign_in("Signing in", &CancelToken::new())
            .unwrap()
            .login,
        "ada"
    );
    assert_eq!(account.login().as_deref(), Some("ada"));
}

/// `count` pipelines, the newest first, by turns running, passed, failed, waiting for
/// someone, and cancelled; each with a build, a test (one running, one failing as it may), and
/// a manual deploy stage.
fn fake_pipelines(count: u64) -> Vec<FakePipeline> {
    let statuses = ["RUNNING", "SUCCESS", "FAILED", "MANUAL", "CANCELED"];
    (0..count)
        .rev()
        .map(|number| FakePipeline {
            id: 9_000 + number,
            iid: number + 1,
            sha: format!("{:040x}", number + 1),
            git_ref: "main".to_owned(),
            status: statuses[usize::try_from(number).unwrap() % statuses.len()].to_owned(),
            stages: vec![
                (
                    "build".to_owned(),
                    "success".to_owned(),
                    vec![FakeJob {
                        name: "compile".to_owned(),
                        status: "SUCCESS".to_owned(),
                        duration: Some(42),
                        ..FakeJob::default()
                    }],
                ),
                (
                    "test".to_owned(),
                    "running".to_owned(),
                    vec![
                        FakeJob {
                            name: "unit".to_owned(),
                            status: "RUNNING".to_owned(),
                            ..FakeJob::default()
                        },
                        FakeJob {
                            name: "lint".to_owned(),
                            status: "FAILED".to_owned(),
                            allow_failure: true,
                            ..FakeJob::default()
                        },
                    ],
                ),
                (
                    "deploy".to_owned(),
                    "manual".to_owned(),
                    vec![FakeJob {
                        name: "production".to_owned(),
                        status: "MANUAL".to_owned(),
                        manual: true,
                        ..FakeJob::default()
                    }],
                ),
            ],
            ..FakePipeline::default()
        })
        .collect()
}

#[test]
fn pipelines_come_a_page_at_a_time_and_one_is_read_whole() {
    let (fake, _sandbox, account, project) = setup(0, TOKEN);
    fake.state().repositories[0].pipelines = fake_pipelines(150);

    let mut pages = Vec::new();
    let listing = pipelines(&account, &project, 1_000, &CancelToken::new(), |page| {
        pages.push(page);
    })
    .unwrap();
    assert_eq!(listing, PipelineListing::Listed);
    let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
    assert_eq!(sizes, [100, 50]);
    let newest = &pages[0][0];
    assert_eq!((newest.id, newest.iid), (9_149, 150));
    assert_eq!(
        newest.commit,
        ObjectId::from_hex(format!("{:040x}", 150).as_bytes()).unwrap()
    );
    assert_eq!(newest.git_ref.as_deref(), Some("main"));
    assert_eq!(
        newest.url,
        format!("http://{}/team/sub/tool/-/pipelines/9149", fake.address())
    );
    let groups: Vec<CiGroup> = pages[0][..5]
        .iter()
        .map(|found| found.status.group())
        .collect();
    assert_eq!(
        groups,
        [
            CiGroup::Canceled,
            CiGroup::Paused,
            CiGroup::Failed,
            CiGroup::Success,
            CiGroup::Running
        ]
    );
    // A limit is kept to.
    let mut count = 0;
    pipelines(&account, &project, 30, &CancelToken::new(), |page| {
        count += page.len();
    })
    .unwrap();
    assert_eq!(count, 30);
    // A project whose CI/CD is off lists none: hidden, said so, not an error.
    fake.state().pipelines_hidden = true;
    let listing = pipelines(&account, &project, 30, &CancelToken::new(), |_| {}).unwrap();
    assert_eq!(listing, PipelineListing::Hidden);
    fake.state().pipelines_hidden = false;

    let details = pipeline_details(&account, &project, 150, &CancelToken::new()).unwrap();
    assert_eq!(details.pipeline, *newest);
    assert_eq!(details.user.as_deref(), Some("ada"));
    assert_eq!(details.duration, Some(95));
    let stages: Vec<(&str, CiStatus, usize)> = details
        .stages
        .iter()
        .map(|stage| (stage.name.as_str(), stage.status, stage.jobs.len()))
        .collect();
    assert_eq!(
        stages,
        [
            ("build", CiStatus::Success, 1),
            ("test", CiStatus::Running, 2),
            ("deploy", CiStatus::Manual, 1)
        ]
    );
    let lint = &details.stages[1].jobs[1];
    assert_eq!((lint.status, lint.allow_failure), (CiStatus::Failed, true));
    assert_eq!(details.stages[0].jobs[0].duration, Some(42));
    assert!(details.stages[2].jobs[0].manual);
    assert_eq!(
        details.stages[0].jobs[0].url.as_deref(),
        Some(format!("http://{}/team/sub/tool/-/jobs/compile", fake.address()).as_str())
    );
    // One that isn't there says so.
    assert!(matches!(
        pipeline_details(&account, &project, 999, &CancelToken::new()),
        Err(ForgeError::NotFound { .. })
    ));
}

#[test]
fn a_gitlab_says_the_name_its_ssh_is_on() {
    let (fake, _sandbox, account, _project) = setup(0, TOKEN);
    // On its own name: the fake's.
    assert_eq!(
        ssh_host(&account, &CancelToken::new()).unwrap().as_deref(),
        Some("127.0.0.1")
    );
    // On a name of its own, as its projects' ssh addresses give it.
    fake.state().ssh_host = Some("git-ssh.example.org".to_owned());
    assert_eq!(
        ssh_host(&account, &CancelToken::new()).unwrap().as_deref(),
        Some("git-ssh.example.org")
    );
    // Showing the account no project, it can't say.
    fake.state().repositories.clear();
    assert_eq!(ssh_host(&account, &CancelToken::new()).unwrap(), None);
}
