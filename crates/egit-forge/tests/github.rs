//! GitHub's API against a fake on the loopback address, signed in with a sandbox's credential
//! helper: pull requests read a page at a time, details, the repository, opening a pull request
//! (and its refusal), and refused credentials (forgotten), no credentials, rate limits, and
//! repositories that aren't there.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::Arc;

use egit_forge::testing::{FakeForge, FakePull, FakeRepository};
use egit_forge::{
    Account, CheckState, ForgeError, ForgeHost, ForgeKind, ForgeRepository, NewPullRequest,
    ReviewDecision, ReviewState, create_pull_request, pull_request_details, pull_requests,
    repository_info,
};
use egit_git::testing::Sandbox;
use egit_git::{CancelToken, ObjectId};

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
    let details = pull_request_details(&account, &repository, 3, &CancelToken::new()).unwrap();
    assert_eq!(details.body, "Fixes everything.");
    assert_eq!(details.checks.len(), 2);
    assert_eq!(details.checks[0].state, CheckState::Success);
    assert_eq!(details.checks[1].state, CheckState::Pending);
    assert_eq!(details.reviews[0].state, ReviewState::Approved);
    assert_eq!(details.labels, ["bug"]);
    assert!(matches!(
        pull_request_details(&account, &repository, 99, &CancelToken::new()),
        Err(ForgeError::NotFound { .. })
    ));

    let info = repository_info(&account, &repository, &CancelToken::new()).unwrap();
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
    let opened = create_pull_request(&account, &repository, &new, &CancelToken::new()).unwrap();
    assert_eq!(opened.number, 2);
    let sent = fake.state().requests.last().unwrap().body.clone();
    assert_eq!(sent["head"], "ada:feature");
    assert_eq!(sent["base"], "main");
    assert_eq!(sent["title"], "Add a feature");
    assert_eq!(sent["draft"], true);
    match create_pull_request(&account, &repository, &new, &CancelToken::new()) {
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
    let result = pull_requests(&account, &repository, 100, &CancelToken::new(), |_| {});
    assert!(matches!(result, Err(ForgeError::Refused { .. })));
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    assert!(!kept.contains("wrong"), "{kept}");
    // Then there is none, and nothing asks: still refused, saying why there is none.
    let result = pull_requests(&account, &repository, 100, &CancelToken::new(), |_| {});
    assert!(matches!(result, Err(ForgeError::Refused { .. })));
    // A fresh account (a new session) has none, and isn't signed in.
    let fresh = Account::new(sandbox.git().clone(), None, repository.host().clone());
    let result = pull_requests(&fresh, &repository, 100, &CancelToken::new(), |_| {});
    assert!(matches!(result, Err(ForgeError::NotSignedIn { .. })));
}

#[test]
fn signing_in_says_who_and_keeps_the_credential() {
    let (_fake, sandbox, account, _repository) = setup(0, TOKEN);
    std::fs::write(sandbox.path("credentials"), "").unwrap();
    // Nothing kept, and the prompt (unanswerable here) declined: not signed in.
    assert!(matches!(
        account.sign_in("Signing in", &CancelToken::new()),
        Err(ForgeError::NotSignedIn { .. })
    ));
    let (fake, sandbox, account, _repository) = setup(0, TOKEN);
    assert_eq!(
        account.sign_in("Signing in", &CancelToken::new()).unwrap(),
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
        pull_requests(&account, &other, 100, &CancelToken::new(), |_| {}),
        Err(ForgeError::NotFound { .. })
    ));
    fake.state().rate_limited = true;
    match pull_requests(&account, &repository, 100, &CancelToken::new(), |_| {}) {
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
        pull_requests(&account, &repository, 100, &cancel, |_| {}),
        Err(ForgeError::Cancelled)
    ));
}

#[test]
fn signing_in_asks_the_user_and_has_the_helper_keep_what_worked() {
    let (fake, sandbox, _, _) = setup(0, TOKEN);
    std::fs::write(sandbox.path("credentials"), "").unwrap();
    // The user, as git's askpass program asks them: a username, then a token as the password.
    let askpass = sandbox.path("askpass.sh");
    std::fs::write(
        &askpass,
        "#!/bin/sh\ncase \"$1\" in Username*) echo ada ;; *) echo t0ken ;; esac\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &askpass,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let git = sandbox
        .git_with([("GIT_ASKPASS", askpass.as_os_str())])
        .unwrap();
    let host = ForgeHost::parse(ForgeKind::GitHub, &fake.host_entry()).unwrap();
    let account = Account::new(git, None, host);
    assert_eq!(
        account.sign_in("Signing in", &CancelToken::new()).unwrap(),
        "ada"
    );
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    assert!(kept.contains("ada:t0ken@127.0.0.1"), "{kept}");
}
