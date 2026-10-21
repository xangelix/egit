//! GitLab's pipelines, from a fake GitLab on the loopback address: each commit's latest
//! pipeline marked beside its name (the newest of several; none for a commit without one), the
//! mark following its pipeline as it finishes; a mark's click showing the pipeline (its stages
//! in order, their jobs), a job opening its page in the browser, Show the commit going back to
//! the history with it selected, and Escape closing the page. Pipelines GitLab refuses say
//! why under the commit table. The Overview says what each remote was taken for (a project, or
//! nothing: a host egit doesn't know), what was read, and how many of the commits the
//! pipelines ran for the history shows; and when GitLab lists none to the account.

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeForge, FakeJob, FakePipeline};
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::screens::{
    commit_of, forge_clone, harness, history_of, into_view, open_path, profile_loaded,
    pull_requests_read, render, settle, show_panel, wait_for_label,
};
use crate::app::EgitApp;

const TOKEN: &str = "glpat-t0ken";

/// A pipeline of `sha`, its build passed, its tests as `tests` says, and a manual deploy.
fn pipeline(id: u64, sha: &str, status: &str, tests: &str) -> FakePipeline {
    FakePipeline {
        id,
        iid: id.saturating_sub(9_000),
        sha: sha.to_owned(),
        git_ref: "main".to_owned(),
        status: status.to_owned(),
        created: format!("2024-05-01T10:{:02}:00Z", id.saturating_sub(9_000)),
        stages: vec![
            (
                "build".to_owned(),
                "success".to_owned(),
                vec![FakeJob {
                    name: "compile".to_owned(),
                    status: "SUCCESS".to_owned(),
                    duration: Some(95),
                    ..FakeJob::default()
                }],
            ),
            (
                "test".to_owned(),
                tests.to_lowercase(),
                vec![
                    FakeJob {
                        name: "unit".to_owned(),
                        status: tests.to_owned(),
                        ..FakeJob::default()
                    },
                    FakeJob {
                        name: "lint".to_owned(),
                        status: "FAILED".to_owned(),
                        allow_failure: true,
                        duration: Some(12),
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
    }
}

/// Whether a button named `label` is in the window.
fn shows(harness: &egui_kittest::Harness<'_, EgitApp>, label: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Button, label)
        .is_some()
}

#[test]
fn each_commit_s_pipeline_is_marked_and_shown_down_to_its_jobs() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, topic, _fork) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    // The newest first: main's running (an older one of it passed), the topic's failed.
    fake.state().repositories[0].pipelines = vec![
        pipeline(9_003, &head, "RUNNING", "RUNNING"),
        pipeline(9_002, &topic, "FAILED", "FAILED"),
        pipeline(9_001, &head, "SUCCESS", "SUCCESS"),
    ];
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, "Pipeline #9003: running");
    wait_for_label(&mut harness, "Pipeline #9002: failed");
    assert!(!shows(&harness, "Pipeline #9001: passed"));
    render(&mut harness, "pipeline-marks");

    // Finished there: the mark follows, read again while it was under way.
    fake.state().repositories[0].pipelines[0].status = "SUCCESS".to_owned();
    fake.state().repositories[0].pipelines[0].stages[1].1 = "success".to_owned();
    fake.state().repositories[0].pipelines[0].stages[1].2[0].status = "SUCCESS".to_owned();
    harness.state_mut().state.repos[0]
        .forge
        .read_pipelines_now();
    wait_for_label(&mut harness, "Pipeline #9003: passed");

    // Its page: the stages in order, their jobs.
    harness
        .get_by_role_and_label(Role::Button, "Pipeline #9003: passed")
        .click();
    wait_for_label(&mut harness, "compile: passed");
    for job in ["unit: passed", "lint: failed", "production: manual"] {
        harness.get_by_role_and_label(Role::Button, job);
    }
    let stages: Vec<f32> = ["build", "test", "deploy"]
        .iter()
        .map(|stage| harness.get_by_label(stage).rect().top())
        .collect();
    assert!(
        stages.windows(2).all(|pair| pair[0] < pair[1]),
        "{stages:?}"
    );
    harness.get_by_label("may fail");
    render(&mut harness, "pipeline");

    // A job opens its page on GitLab.
    harness
        .get_by_role_and_label(Role::Button, "lint: failed")
        .click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        [format!(
            "http://{}/team/sub/tool/-/jobs/lint",
            fake.address()
        )]
    );

    // Show the commit: back in the history, it selected.
    harness.get_by_label("Show the commit").click();
    harness.run_steps(3);
    assert!(shows(&harness, "Pipeline #9003: passed"));
    let repo = &harness.state().state.repos[0];
    let selected = repo
        .history_view
        .selected_row()
        .and_then(|row| repo.history.shown.id(row));
    assert_eq!(selected, egit_git::ObjectId::from_hex(head.as_bytes()));

    // And Escape closes the page.
    harness
        .get_by_role_and_label(Role::Button, "Pipeline #9002: failed")
        .click();
    wait_for_label(&mut harness, "unit: failed");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(!shows(&harness, "unit: failed"));
}

#[test]
fn pipelines_that_can_t_be_read_say_why_until_they_can() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    fake.state().repositories[0].pipelines = vec![pipeline(9_001, &head, "SUCCESS", "SUCCESS")];
    // GitLab's words when an account may not read them.
    let refusal = "The resource that you are attempting to access does not exist or you don't \
                   have permission to perform this action";
    fake.state().pipelines_refused = Some(refusal.to_owned());
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    // Under the commit table: why.
    settle(&mut harness, "the pipelines' problem", |app| {
        app.state.repos[0].forge.pipelines_problem().is_some()
    });
    harness.run_steps(2);
    let shown = "The pipelines couldn't be read";
    let why = harness
        .get_by_label_contains(shown)
        .accesskit_node()
        .value()
        .unwrap_or_default();
    assert!(why.contains("you don't have permission"), "{why}");
    render(&mut harness, "pipelines-unread");

    // Dismissed, it goes; read once it can be, the mark shows.
    harness
        .get_by_role_and_label(Role::Button, "Dismiss")
        .click();
    harness.run_steps(2);
    assert!(harness.query_by_label_contains(shown).is_none());
    fake.state().pipelines_refused = None;
    harness.state_mut().state.repos[0]
        .forge
        .read_pipelines_now();
    wait_for_label(&mut harness, "Pipeline #9001: passed");
    assert!(
        harness.state().state.repos[0]
            .forge
            .pipelines_problem()
            .is_none()
    );
}

#[test]
fn the_overview_says_what_each_remote_is_and_what_was_read() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, topic, fork) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    // The fork's commit isn't fetched here: its pipeline marks nothing.
    fake.state().repositories[0].pipelines = vec![
        pipeline(9_003, &head, "SUCCESS", "SUCCESS"),
        pipeline(9_002, &fork, "FAILED", "FAILED"),
        pipeline(9_001, &topic, "SUCCESS", "SUCCESS"),
    ];
    // A remote on a host egit doesn't know.
    sandbox
        .run(
            &work,
            "remote",
            &["add", "mirror", "https://git.example.invalid/team/tool.git"],
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, "Pipeline #9003: passed");

    show_panel(&mut harness, "Overview");
    let project = format!("GitLab project team/sub/tool on {}", fake.address());
    wait_for_label(&mut harness, &project);
    harness.get_by_label_contains("Nothing: its host isn't one egit knows");
    harness.get_by_label("https://git.example.invalid/team/tool.git");
    harness.get_by_label("Signed in with the credential git's helper keeps");
    harness.get_by_label("2 open");
    harness.get_by_label_contains("3 read; the newest, #3, ran for main");
    wait_for_label(
        &mut harness,
        "2 of the 3 commits the pipelines ran for: each marked beside its name.",
    );
    into_view(&mut harness, &project);
    render(&mut harness, "overview-forge");

    // GitLab listing none to the account (CI/CD off): said so, once read again.
    fake.state().pipelines_hidden = true;
    let again = format!("{}  Read again", crate::theme::icon::ARROWS_CLOCKWISE);
    into_view(&mut harness, &again);
    harness.get_by_role_and_label(Role::Button, &again).click();
    wait_for_label(
        &mut harness,
        &format!(
            "{}  GitLab lists none to this account: CI/CD may be turned off for the project, or \
             your role in it may not see its pipelines.",
            crate::theme::icon::WARNING
        ),
    );
}

#[test]
fn an_ssh_host_on_a_name_of_its_own_is_tied_to_its_gitlab_in_a_click() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    fake.state().repositories[0].pipelines = vec![pipeline(9_001, &head, "SUCCESS", "SUCCESS")];
    // Its ssh on a name and port of its own, as a self-managed GitLab's often is (reached here
    // through the bare repository).
    let ssh = "ssh://git@git-ssh.example.invalid:2222/team/sub/tool.git";
    sandbox
        .run(&work, "remote", &["set-url", "origin", ssh])
        .unwrap();
    let bare = sandbox.path("origin.git");
    let rewrite = format!("url.file://{}.insteadOf", bare.to_str().unwrap());
    sandbox
        .run(&work, "config", &["--add", &rewrite, ssh])
        .unwrap();
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "egit.gitlabHost", &fake.host_entry()],
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Not on a host egit knows: offered as the ssh host of the GitLab Profile lists.
    show_panel(&mut harness, "Overview");
    wait_for_label(&mut harness, ssh);
    harness.get_by_label_contains("Nothing: git-ssh.example.invalid isn't a host egit knows");
    harness.get_by_label_contains("git reaches it as file://");
    let tie = format!("The ssh host of {}", fake.address());
    into_view(&mut harness, &tie);
    render(&mut harness, "overview-ssh-host");
    harness.get_by_role_and_label(Role::Button, &tie).click();

    // Kept in the user's settings, the project is read: its merge requests and pipelines.
    let project = format!("GitLab project team/sub/tool on {}", fake.address());
    wait_for_label(&mut harness, &project);
    settle(&mut harness, "the pipeline", |app| {
        app.state.repos[0]
            .forge
            .pipeline_of(&egit_git::ObjectId::from_hex(head.as_bytes()).unwrap())
            .is_some()
    });
    harness.get_by_label("2 open");
    let kept = sandbox
        .run(
            sandbox.home(),
            "config",
            &[
                "--global",
                "--get-all",
                &format!("egit.{}.sshHost", fake.address()),
            ],
        )
        .unwrap();
    assert_eq!(
        String::from_utf8(kept.stdout).unwrap().trim(),
        "git-ssh.example.invalid"
    );

    // Profile lists it under its host.
    harness
        .state_mut()
        .state
        .actions
        .push(crate::app::Action::ShowProfile);
    wait_for_label(&mut harness, "    its ssh: git-ssh.example.invalid");
}
