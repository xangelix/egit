//! GitLab's pipelines, from a fake GitLab on the loopback address: each commit's latest
//! pipeline marked beside its name (the newest of several; none for a commit without one), the
//! mark following its pipeline as it finishes; a mark's click showing the pipeline (its stages
//! in order, their jobs), a job opening its page in the browser, Show the commit going back to
//! the history with it selected, and Escape closing the page.

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeForge, FakeJob, FakePipeline};
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    commit_of, forge_clone, harness, history_of, open_path, profile_loaded, pull_requests_read,
    render, settle, wait_for_label,
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
