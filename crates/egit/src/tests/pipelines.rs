//! GitLab's pipelines, from a fake GitLab on the loopback address: each commit's latest
//! pipeline marked beside its name (the newest of several; none for a commit without one), the
//! mark following its pipeline as it finishes; a mark's click showing the pipeline (its stages
//! in order, their jobs), a job opening its page in the browser, Show the commit going back to
//! the history with it selected, and Escape closing the page. Pipelines GitLab refuses say
//! why under the commit table. The Overview says what each remote was taken for (a project, or
//! nothing: a host egit doesn't know), what was read, and how many of the commits the
//! pipelines ran for the history shows; and when GitLab lists none to the account. Jobs laid out
//! by stage and by what they wait for; a job's earlier run's log; a job retried (refused, then
//! not: its new run shown), cancelled, and started; the pipeline cancelled and its failed jobs
//! retried, its mark following.

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeForge, FakeJob, FakePipeline};
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::screens::{
    click_when_enabled, commit_of, forge_clone, harness, history_of, into_view, open_path,
    profile_loaded, pull_requests_read, render, settle, show_panel, wait_for_label,
};
use crate::app::EgitApp;
use crate::theme::icon;

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

    // A job shows its log (none, here), its page on GitLab a button away; Escape goes back.
    harness
        .get_by_role_and_label(Role::Button, "lint: failed")
        .click();
    wait_for_label(
        &mut harness,
        "This job has no log (it may have been erased).",
    );
    harness
        .get_by_label(&format!("{}  Open on GitLab", icon::ARROW_SQUARE_OUT))
        .click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        [format!(
            "http://{}/team/sub/tool/-/jobs/900302",
            fake.address()
        )]
    );
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "compile: passed");

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
    harness.get_by_label_contains("3 read; the newest, Pipeline #9003, ran for main");
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

/// The address of `work`'s origin moved to ssh on a name and port of its own, as a
/// self-managed GitLab's often is (`gitlab_shell_ssh_host`; reached here through the bare
/// repository), the fake listed in the user's settings.
const ON_ITS_OWN: &str = "ssh://git@git-ssh.example.invalid:2222/team/sub/tool.git";

fn ssh_on_a_name_of_its_own(sandbox: &Sandbox, fake: &FakeForge, work: &std::path::Path) {
    sandbox
        .run(work, "remote", &["set-url", "origin", ON_ITS_OWN])
        .unwrap();
    let bare = sandbox.path("origin.git");
    let rewrite = format!("url.file://{}.insteadOf", bare.to_str().unwrap());
    sandbox
        .run(work, "config", &["--add", &rewrite, ON_ITS_OWN])
        .unwrap();
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "egit.gitlabHost", &fake.host_entry()],
        )
        .unwrap();
}

#[test]
fn a_gitlab_s_ssh_on_a_name_of_its_own_is_learned_from_it_unasked() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    fake.state().ssh_host = Some("git-ssh.example.invalid".to_owned());
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    fake.state().repositories[0].pipelines = vec![pipeline(9_001, &head, "SUCCESS", "SUCCESS")];
    ssh_on_a_name_of_its_own(&sandbox, &fake, &work);
    // Signed in to it as the user (a helper keeping the token everywhere).
    let store = sandbox.path("credentials");
    let helper = format!(
        "store --file={}",
        store.display().to_string().replace('\\', "/")
    );
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "credential.helper", &helper],
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    // Nothing asked of anyone: the GitLab said where its ssh is, and the project is read.
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    wait_for_label(&mut harness, "Pipeline #9001: passed");
    show_panel(&mut harness, "Overview");
    wait_for_label(&mut harness, "its ssh on git-ssh.example.invalid");
    // Learned for this run, nothing written to the settings.
    let written = sandbox
        .run(sandbox.home(), "config", &["--global", "--list"])
        .unwrap();
    assert!(
        !String::from_utf8(written.stdout)
            .unwrap()
            .contains("sshhost")
    );
}

#[test]
fn an_ssh_host_on_a_name_of_its_own_is_tied_to_its_gitlab_in_a_click() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    fake.state().repositories[0].pipelines = vec![pipeline(9_001, &head, "SUCCESS", "SUCCESS")];
    // Not signed in as the user: the GitLab can't be asked, so the user says.
    ssh_on_a_name_of_its_own(&sandbox, &fake, &work);
    let ssh = ON_ITS_OWN;
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
    wait_for_label(&mut harness, "2 open");
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

/// A runner's line: its timestamp (a second after 10:21:00), then `text`.
fn runner(second: u32, text: &str) -> String {
    format!("2024-05-01T10:21:{second:02}.000000Z 00O {text}\n")
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

#[test]
fn a_job_s_log_shows_and_follows_it_as_it_runs() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    let mut running = pipeline(9_004, &head, "RUNNING", "RUNNING");
    let unit = &mut running.stages[1].2[0];
    unit.id = 4_321;
    unit.log = [
        runner(
            0,
            "\u{1b}[0Ksection_start:1714558860:prepare_executor\r\u{1b}[0K\u{1b}[36;1mPreparing the docker executor\u{1b}[0;m",
        ),
        runner(1, "Using docker image rust:1.99"),
        runner(
            4,
            "\u{1b}[0Ksection_end:1714558864:prepare_executor\r\u{1b}[0K\u{1b}[0Ksection_start:1714558864:step_script\r\u{1b}[0K\u{1b}[36;1mExecuting \"step_script\" stage of the job script\u{1b}[0;m",
        ),
        runner(5, "\u{1b}[32;1m$ cargo test\u{1b}[0;m"),
        runner(9, "running 3 tests"),
    ]
    .concat();
    fake.state().repositories[0].pipelines = vec![running];
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, "Pipeline #9004: running");
    harness
        .get_by_role_and_label(Role::Button, "Pipeline #9004: running")
        .click();

    // A job's click shows its log: its sections' markers and the runner's timestamps out.
    wait_for_label(&mut harness, "unit: running");
    harness
        .get_by_role_and_label(Role::Button, "unit: running")
        .click();
    settle(&mut harness, "the log", log_holds("running 3 tests"));
    let (log, following) = harness.state().state.repos[0]
        .history_view
        .job_log()
        .unwrap();
    assert_eq!(
        log,
        "Preparing the docker executor\nUsing docker image rust:1.99\nExecuting \"step_script\" \
         stage of the job script\n$ cargo test\nrunning 3 tests\n"
    );
    assert!(following);
    harness.get_by_label_contains("Sections");
    render(&mut harness, "job-log");

    // As it runs, what it writes comes; once it ends, it says so and stops.
    fake.state().repositories[0].pipelines[0].stages[1].2[0]
        .log
        .push_str(&runner(12, "test parse ... ok"));
    settle(
        &mut harness,
        "the next line",
        log_holds("test parse ... ok"),
    );
    fake.state().repositories[0].pipelines[0].stages[1].2[0]
        .log
        .push_str(&runner(14, "\u{1b}[32;1mJob succeeded\u{1b}[0;m"));
    fake.state().repositories[0].pipelines[0].stages[1].2[0].status = "SUCCESS".to_owned();
    settle(&mut harness, "the job's end", log_holds("Job succeeded"));
    wait_for_label(&mut harness, &format!("{}  unit", icon::CHECK_CIRCLE));

    // Escape goes back to the pipeline.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    wait_for_label(&mut harness, "compile: passed");
    assert!(
        harness.state().state.repos[0]
            .history_view
            .job_log()
            .is_none()
    );
}

/// A job of `stage` numbered `id`, as `status` says, waiting for `needs` (none: its stages).
fn job(id: u64, name: &str, status: &str, needs: Option<&[&str]>) -> FakeJob {
    FakeJob {
        id,
        name: name.to_owned(),
        status: status.to_owned(),
        needs: needs.map(|needs| needs.iter().map(|&need| need.to_owned()).collect()),
        duration: Some(30),
        ..FakeJob::default()
    }
}

/// The left edge of the card or row named `label`.
fn left_of(harness: &egui_kittest::Harness<'_, EgitApp>, label: &str) -> f32 {
    harness
        .get_by_role_and_label(Role::Button, label)
        .rect()
        .left()
}

/// Steps until the job's run whose log shows is `run`.
fn run_shown(run: u64) -> impl Fn(&mut EgitApp) -> bool {
    move |app| app.state.repos[0].history_view.job_run() == Some(run)
}

/// A failed pipeline of `head`: unit ran twice (the first run kept), docs needs nothing,
/// production waits for unit alone.
fn ran_twice(head: &str) -> FakePipeline {
    let mut unit = job(601, "unit", "FAILED", Some(&["compile"]));
    unit.log = "second try failed\n".to_owned();
    let mut first = job(501, "unit", "FAILED", Some(&["compile"]));
    first.log = "first try failed\n".to_owned();
    unit.earlier = vec![first];
    let mut failed = pipeline(9_006, head, "FAILED", "FAILED");
    failed.stages = vec![
        (
            "build".to_owned(),
            "success".to_owned(),
            vec![job(600, "compile", "SUCCESS", None)],
        ),
        (
            "test".to_owned(),
            "failed".to_owned(),
            vec![
                unit,
                job(602, "e2e", "RUNNING", None),
                job(604, "docs", "SUCCESS", Some(&[])),
            ],
        ),
        (
            "deploy".to_owned(),
            "manual".to_owned(),
            vec![job(603, "production", "MANUAL", Some(&["unit"]))],
        ),
    ];
    failed
}

/// Each layout of its jobs: by what each waits for (docs needs nothing: the first column;
/// production one after unit), then by stage (docs in its own stage's column), then the list.
fn laid_out(harness: &mut egui_kittest::Harness<'_, EgitApp>) {
    harness.get_by_label("Dependencies").click();
    harness.run_steps(2);
    assert_eq!(
        left_of(harness, "docs: passed"),
        left_of(harness, "compile: passed")
    );
    assert_eq!(
        left_of(harness, "e2e: running"),
        left_of(harness, "unit: failed")
    );
    assert!(left_of(harness, "compile: passed") < left_of(harness, "unit: failed"));
    assert!(left_of(harness, "unit: failed") < left_of(harness, "production: manual"));
    render(harness, "pipeline-dependencies");
    // By stage: docs in its own stage's column.
    harness.get_by_label("Stages").click();
    harness.run_steps(2);
    assert_eq!(
        left_of(harness, "docs: passed"),
        left_of(harness, "unit: failed")
    );
    harness
        .get_by_role_and_label(Role::Button, "unit: failed")
        .hover();
    harness.run_steps(2);
    render(harness, "pipeline-stages");
    harness.get_by_label("List").click();
    harness.run_steps(2);
}

#[test]
fn jobs_are_laid_out_by_what_they_wait_for_and_retried_cancelled_and_started() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    let head = commit_of(&sandbox, &work, "HEAD");
    fake.state().repositories[0].pipelines = vec![ran_twice(&head)];
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    wait_for_label(&mut harness, "Pipeline #9006: failed");
    harness
        .get_by_role_and_label(Role::Button, "Pipeline #9006: failed")
        .click();
    wait_for_label(&mut harness, "unit: failed");
    harness.get_by_label("2 runs");

    laid_out(&mut harness);

    // An earlier run's log, from the job's menu; then the latest's, from the runs' menu.
    harness
        .get_by_role_and_label(Role::Button, "unit: failed")
        .click_secondary();
    wait_for_label(&mut harness, "Earlier runs");
    harness.get_by_label("Earlier runs").hover();
    harness.run_steps(2);
    harness.get_by_label_contains("Run #501 · failed").click();
    settle(&mut harness, "the first run", run_shown(501));
    settle(&mut harness, "its log", log_holds("first try failed"));
    harness.get_by_label_contains("Run 1 of 2").click();
    harness.run_steps(2);
    harness.get_by_label_contains("Latest #601").click();
    settle(
        &mut harness,
        "the latest's log",
        log_holds("second try failed"),
    );

    // Retried where GitLab refuses: said; then where it doesn't: the new run shows.
    let retry = format!("{}  Retry the job", icon::ARROW_CLOCKWISE);
    fake.state().read_only = true;
    click_when_enabled(&mut harness, &retry);
    wait_for_label(&mut harness, "Dismiss");
    harness.get_by_label_contains("The job couldn't be retried");
    harness.get_by_label_contains("Developer and above");
    render(&mut harness, "job-log-refused");
    harness.get_by_label("Dismiss").click();
    fake.state().read_only = false;
    click_when_enabled(&mut harness, &retry);
    settle(&mut harness, "the new run", run_shown(990_001));
    harness.get_by_label_contains("Run 3 of 3");

    // Back on the pipeline, read again: the job pending, three runs.
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "unit: pending");
    harness.get_by_label("3 runs");

    // A job cancelled and another started from their rows.
    click_when_enabled(&mut harness, "Cancel e2e");
    wait_for_label(&mut harness, "e2e: canceled");
    click_when_enabled(&mut harness, "Start production");
    wait_for_label(&mut harness, "production: pending");

    // The pipeline cancelled (asked first), then its failed and cancelled jobs retried.
    click_when_enabled(
        &mut harness,
        &format!("{}  Cancel the pipeline", icon::PROHIBIT),
    );
    wait_for_label(&mut harness, "Cancel it");
    harness.get_by_label("Cancel it").click();
    wait_for_label(&mut harness, "unit: canceled");
    wait_for_label(&mut harness, "production: canceled");
    click_when_enabled(
        &mut harness,
        &format!("{}  Retry failed jobs", icon::ARROW_CLOCKWISE),
    );
    wait_for_label(&mut harness, "unit: pending");
    wait_for_label(&mut harness, "4 runs");
    assert!(shows(&harness, "e2e: pending"));
    assert!(shows(&harness, "compile: passed"));

    // Its mark follows: running again.
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, "Pipeline #9006: running");
}
