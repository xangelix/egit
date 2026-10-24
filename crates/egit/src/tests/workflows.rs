//! GitHub Actions' workflow runs as pipelines, from a fake GitHub on the loopback address: a
//! commit's runs of two workflows marked as one (the failure shown, the other run a menu away
//! on its page), the run's jobs laid out by what its workflow's file says they need (read at
//! its commit here), a failed job's log through GitHub's redirect, its failed jobs re-run (a
//! second attempt), a running job's steps as they go, the run cancelled (asked first), a job
//! re-run from its row, and the mark following. The run's artifacts downloaded where the save
//! dialog says (each checked against its digest: one that doesn't match isn't saved), an
//! expired one listed but not offered.

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeArtifact, FakeForge, FakeJob, FakePipeline};
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::screens::{
    click_when_enabled, commit_of, forge_clone, harness, history_of, open_path, profile_loaded,
    render, settle, wait_for_label,
};
use crate::app::EgitApp;
use crate::downloads::Stage;
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
    failed.log = "\u{feff}2024-05-01T10:21:00.1234567Z Current runner version: '2.316.1'\n\
                  2024-05-01T10:21:01.0000000Z ##[group]Run make test\n\
                  2024-05-01T10:21:01.0000000Z make test\n\
                  2024-05-01T10:21:01.0000000Z ##[endgroup]\n\
                  2024-05-01T10:21:01.5000000Z FAIL parse\n\
                  2024-05-01T10:21:02.0000000Z ##[error]Process completed with exit code 2.\n"
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
    harness.get_by_role_and_label(Role::Button, "CI #57");

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
    // Its steps head what they wrote, as GitHub shows them: the one that failed open.
    settle(&mut harness, "its steps", |app| {
        app.state.repos[0]
            .history_view
            .job_log_shown()
            .is_some_and(|shown| {
                shown.starts_with("Set up job  ⋯ 1 line\nRun make\nRun make test  ⋯ 1 line\n")
            })
    });
    wait_for_label(
        &mut harness,
        "Line 7: Error: Process completed with exit code 2.",
    );
    render(&mut harness, "workflow-job-log");
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
        .get_by_role_and_label(Role::Button, "CI #57")
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
    wait_for_label(&mut harness, "test (macos): cancelled");

    // A job run again from its row; the mark follows.
    click_when_enabled(&mut harness, "Re-run test (macos)");
    wait_for_label(&mut harness, "test (macos): pending");
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "CI #57: running");
}

/// Steps until the tab's one download has ended as `stage` says.
fn download_ended(ended: fn(&Stage) -> bool) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        let downloads = app.state.repos[0].forge.downloads.list();
        downloads.len() == 1 && downloads.iter().all(|download| ended(&download.stage()))
    }
}

#[test]
fn a_run_s_artifacts_are_downloaded_and_checked_against_their_digests() {
    const DIST: &str = "PK\u{3}\u{4} tool for linux";
    let (sandbox, fake, mut harness) = ran();
    let artifact = |id, name: &str, contents: &str| FakeArtifact {
        id,
        name: name.to_owned(),
        contents: contents.to_owned(),
        ..FakeArtifact::default()
    };
    fake.state().repositories[0].pipelines[1].artifacts = vec![
        artifact(71, "dist-linux", DIST),
        FakeArtifact {
            expired: true,
            ..artifact(72, "coverage", "lines")
        },
        FakeArtifact {
            digest: format!("sha256:{}", "0".repeat(64)),
            ..artifact(73, "dist-macos", "changed on the way")
        },
    ];
    let out = sandbox.path("out");
    std::fs::create_dir(&out).unwrap();
    harness.state_mut().state.files = crate::desktop::FileDialogs::in_folder(out.clone());
    wait_for_label(&mut harness, "CI #57: failed");
    harness
        .get_by_role_and_label(Role::Button, "CI #57: failed")
        .click();
    wait_for_label(&mut harness, "test (macos): failed");

    // The run's own, each a zip; the expired one listed, not offered.
    harness.get_by_label("Artifacts").click();
    let dist = format!("Download dist-linux.zip · {} bytes", DIST.len());
    wait_for_label(&mut harness, &dist);
    assert!(
        harness
            .get_by_label("Download coverage.zip · 5 bytes")
            .accesskit_node()
            .is_disabled()
    );
    render(&mut harness, "workflow-artifacts");
    harness.get_by_label(&dist).click();
    settle(
        &mut harness,
        "the artifact saved",
        download_ended(|stage| *stage == Stage::Saved),
    );
    assert_eq!(
        std::fs::read_to_string(out.join("dist-linux.zip")).unwrap(),
        DIST
    );
    wait_for_label(&mut harness, "Dismiss");
    harness.get_by_label("Dismiss").click();
    harness.run_steps(2);

    // Not what its digest says: said, and nothing left on disk.
    harness.get_by_label("Artifacts").click();
    wait_for_label(&mut harness, "Download dist-macos.zip · 18 bytes");
    harness
        .get_by_label("Download dist-macos.zip · 18 bytes")
        .click();
    settle(
        &mut harness,
        "the download refused",
        download_ended(|stage| matches!(stage, Stage::Failed(_))),
    );
    harness.get_by_label_contains("dist-macos.zip doesn't match the SHA-256 digest");
    assert!(!out.join("dist-macos.zip").exists());
    assert!(!out.join("dist-macos.zip.part").exists());
    render(&mut harness, "workflow-artifact-refused");
}

#[test]
fn a_workflow_is_run_on_a_branch_from_its_menu() {
    let (_sandbox, fake, mut harness) = ran();
    settle(&mut harness, "the workflows read", |app| {
        app.state.repos[0].forge.workflows_of(0).is_some()
    });
    // The commit table's badge (the details may show one too): its workflows in a submenu.
    harness
        .get_all_by_label(&format!("{} main", icon::GIT_BRANCH))
        .next()
        .unwrap()
        .click_secondary();
    wait_for_label(&mut harness, "Run a workflow on main");
    harness.get_by_label("Run a workflow on main").hover();
    wait_for_label(&mut harness, "Docs");
    render(&mut harness, "branch-menu-workflows");
    // The audit's pointer went over every control: the submenu opened again.
    if harness.query_by_label("Docs").is_none() {
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        harness
            .get_all_by_label(&format!("{} main", icon::GIT_BRANCH))
            .next()
            .unwrap()
            .click_secondary();
        wait_for_label(&mut harness, "Run a workflow on main");
        harness.get_by_label("Run a workflow on main").hover();
        wait_for_label(&mut harness, "Docs");
    }
    harness.get_by_label("Docs").click();
    wait_for_label(&mut harness, "Started Docs on main.");
    let made = &fake.state().repositories[0].pipelines[0];
    assert_eq!((made.workflow.1, made.git_ref.as_str()), (12, "main"));
}
