//! GitHub Actions' workflow runs as pipelines, from a fake GitHub on the loopback address: a
//! commit's runs of two workflows marked as one (the failure shown, the other run a menu away
//! on its page), the run's jobs laid out by what its workflow's file says they need (read at
//! its commit here), a failed job's log through GitHub's redirect, its failed jobs re-run (a
//! second attempt), a running job's steps as they go, the run cancelled (asked first), a job
//! re-run from its row, and the mark following.

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeForge, FakeJob, FakePipeline};
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    click_when_enabled, commit_of, forge_clone, harness, history_of, open_path, profile_loaded,
    render, settle, wait_for_label,
};
use crate::app::EgitApp;
use crate::theme::icon;

const TOKEN: &str = "ghp-t0ken";

/// The CI workflow: build, a matrix of tests needing it, and a deploy needing the tests.
const WORKFLOW: &str = "\
name: CI
on: push
jobs:
  build:
    runs-on: ubuntu-latest
  test:
    needs: build
    strategy:
      matrix:
        os: [ubuntu, macos]
    runs-on: ${{ matrix.os }}-latest
    steps:
      - run: |
          make test
  deploy:
    needs: [test]
    runs-on: ubuntu-latest
";

/// A job of the fake run, with its steps as it stands.
fn job(id: u64, name: &str, status: &str) -> FakeJob {
    FakeJob {
        id,
        name: name.to_owned(),
        status: status.to_owned(),
        duration: Some(30),
        steps: vec![
            ("Set up job".to_owned(), "SUCCESS".to_owned()),
            ("Run make".to_owned(), status.to_owned()),
        ],
        ..FakeJob::default()
    }
}

/// A run of `workflow` (its name, id, and file) on `sha`.
fn run(
    (id, number): (u64, u64),
    workflow: (&str, u64, &str),
    sha: &str,
    (status, jobs): (&str, Vec<FakeJob>),
) -> FakePipeline {
    FakePipeline {
        id,
        iid: number,
        sha: sha.to_owned(),
        git_ref: "main".to_owned(),
        status: status.to_owned(),
        workflow: (workflow.0.to_owned(), workflow.1, workflow.2.to_owned()),
        stages: vec![(String::new(), String::new(), jobs)],
        ..FakePipeline::default()
    }
}

/// Whether the job's log shown holds `text`.
fn log_holds(text: &'static str) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        app.state.repos[0]
            .history_view
            .job_log()
            .is_some_and(|(log, _)| log.contains(text))
    }
}

/// The left edge of the card named `label`.
fn left_of(harness: &egui_kittest::Harness<'_, EgitApp>, label: &str) -> f32 {
    harness
        .get_by_role_and_label(Role::Button, label)
        .rect()
        .left()
}

/// A fake GitHub serving `octo/tool`, cloned with a commit adding the CI workflow, which ran
/// (its macOS test failed) as did the Docs workflow (passed); the window showing its history.
fn ran() -> (Sandbox, FakeForge, egui_kittest::Harness<'static, EgitApp>) {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitHub, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "octo/tool", TOKEN);
    sandbox
        .commit_file(&work, ".github/workflows/ci.yml", WORKFLOW, "Add CI")
        .unwrap();
    let head = commit_of(&sandbox, &work, "HEAD");
    let mut failed = job(803, "test (macos)", "FAILED");
    failed.log = "\u{feff}2024-05-01T10:21:00.1234567Z ##[group]Run make test\n\
                  2024-05-01T10:21:01.0000000Z FAIL parse\n\
                  2024-05-01T10:21:02.0000000Z ##[endgroup]\n\
                  2024-05-01T10:21:03.0000000Z ##[error]Process completed with exit code 2.\n"
        .to_owned();
    let ci = run(
        (8001, 57),
        ("CI", 11, ".github/workflows/ci.yml"),
        &head,
        (
            "FAILED",
            vec![
                job(801, "build", "SUCCESS"),
                job(802, "test (ubuntu)", "SUCCESS"),
                failed,
                job(804, "deploy", "SKIPPED"),
            ],
        ),
    );
    let docs = run(
        (8002, 9),
        ("Docs", 12, ".github/workflows/docs.yml"),
        &head,
        ("SUCCESS", vec![job(811, "pages", "SUCCESS")]),
    );
    fake.state().repositories[0].pipelines = vec![docs, ci];
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(3));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    (sandbox, fake, harness)
}

#[test]
fn a_commit_s_workflow_runs_are_marked_shown_and_run_again() {
    let (_sandbox, fake, mut harness) = ran();
    // Two workflows' runs, one mark: the failure.
    wait_for_label(&mut harness, "CI #57: failed");
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Docs #9: passed")
            .is_none()
    );
    harness
        .get_by_role_and_label(Role::Button, "CI #57: failed")
        .click();
    wait_for_label(&mut harness, "test (macos): failed");
    harness.get_by_label_contains("1 more workflow run for this commit");

    // By what each needs, as the workflow's file says: no stages to lay out by.
    harness.get_by_label("Dependencies").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Stages").is_none());
    assert!(left_of(&harness, "build: passed") < left_of(&harness, "test (ubuntu): passed"));
    assert_eq!(
        left_of(&harness, "test (ubuntu): passed"),
        left_of(&harness, "test (macos): failed")
    );
    assert!(left_of(&harness, "test (macos): failed") < left_of(&harness, "deploy: skipped"));
    render(&mut harness, "workflow-run-graph");
    harness.get_by_label("List").click();
    harness.run_steps(2);

    // A failed job's log, through GitHub's redirect.
    harness
        .get_by_role_and_label(Role::Button, "test (macos): failed")
        .click();
    settle(&mut harness, "the log", log_holds("FAIL parse"));
    settle(
        &mut harness,
        "the error said",
        log_holds("Error: Process completed with exit code 2."),
    );
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "test (macos): failed");

    // Its failed jobs run again: a second attempt.
    click_when_enabled(
        &mut harness,
        &format!("{}  Re-run failed jobs", icon::ARROW_CLOCKWISE),
    );
    wait_for_label(&mut harness, "test (macos): pending");
    harness.get_by_label_contains("2 (run again 1 time)");
    harness.get_by_label("2 runs");

    // The commit's other workflow's run, from the menu.
    harness
        .get_by_label_contains("1 more workflow run for this commit")
        .click();
    wait_for_label(&mut harness, "Docs #9: passed");
    harness.get_by_label("Docs #9: passed").click();
    wait_for_label(&mut harness, "pages: passed");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);

    // Running: its steps show as they go; cancelled, asked first.
    if let Some(macos) = fake.state().repositories[0].pipelines[1].stages[0]
        .2
        .get_mut(2)
    {
        "RUNNING".clone_into(&mut macos.status);
        macos.steps = vec![
            ("Set up job".to_owned(), "SUCCESS".to_owned()),
            ("Run make".to_owned(), "RUNNING".to_owned()),
        ];
    }
    harness.state_mut().state.repos[0]
        .forge
        .read_pipelines_now();
    wait_for_label(&mut harness, "CI #57: running");
    harness
        .get_by_role_and_label(Role::Button, "CI #57: running")
        .click();
    wait_for_label(&mut harness, "test (macos): running");
    harness
        .get_by_role_and_label(Role::Button, "test (macos): running")
        .click();
    wait_for_label(&mut harness, "Run make");
    harness.get_by_label_contains("GitHub gives its log once it has ended");
    render(&mut harness, "workflow-job-steps");
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "test (macos): running");
    click_when_enabled(
        &mut harness,
        &format!("{}  Cancel workflow", icon::PROHIBIT),
    );
    wait_for_label(&mut harness, "Cancel it");
    harness.get_by_label("Cancel it").click();
    wait_for_label(&mut harness, "test (macos): canceled");

    // A job run again from its row; the mark follows.
    click_when_enabled(&mut harness, "Re-run test (macos)");
    wait_for_label(&mut harness, "test (macos): pending");
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "CI #57: running");
}
