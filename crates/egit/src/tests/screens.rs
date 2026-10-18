//! The screens, driven as a user drives them (`egui_kittest`, through the accessibility tree),
//! against a sandboxed git. Where a wgpu adapter exists (a software one will do: lavapipe) each
//! screen is also rendered to `target/gui-screens/<name>.png` for review; `EGIT_GUI_RENDER=1`
//! requires rendering and `0` skips it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeForge, FakePull, FakeRepository};
use egit_git::askpass::{PromptKind, ask_as_helper};
use egit_git::testing::Sandbox;
use egit_git::{ConfigPlace, ConfigSnapshot, Git};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use crate::app::EgitApp;
use crate::forge::ForgeProblem;
use crate::settings::{Settings, StoredPath};
use crate::theme::icon;
use crate::views::prompts::Prompts;

/// Where renderings are written.
pub(super) fn screens() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gui-screens");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Whether screens are rendered (see the module's documentation).
pub(super) fn rendering() -> bool {
    static RENDERING: OnceLock<bool> = OnceLock::new();
    *RENDERING.get_or_init(|| match std::env::var("EGIT_GUI_RENDER").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new).is_ok(),
    })
}

/// The whole application around `git`, at a typical window size.
pub(super) fn harness_with(git: Git, home: &Path, theme: egui::Theme) -> Harness<'static, EgitApp> {
    harness_with_settings(git, home, theme, Settings::default())
}

/// As [`harness_with`], starting from `settings` as if remembered from the last run.
fn harness_with_settings(
    git: Git,
    home: &Path,
    theme: egui::Theme,
    settings: Settings,
) -> Harness<'static, EgitApp> {
    let home = home.to_owned();
    let builder = Harness::builder()
        .with_size([1180.0, 800.0])
        .with_theme(theme);
    let builder = if rendering() { builder.wgpu() } else { builder };
    builder.build_eframe(move |creation| {
        EgitApp::with_git(
            &creation.egui_ctx,
            git,
            settings,
            Some(home),
            Prompts::none(),
        )
    })
}

pub(super) fn harness(sandbox: &Sandbox) -> Harness<'static, EgitApp> {
    harness_with(sandbox.git().clone(), sandbox.home(), egui::Theme::Dark)
}

/// Checks the screen (`audit`) and renders it into `name.png` (where screens are rendered).
pub(super) fn render(harness: &mut Harness<'_, EgitApp>, name: &str) {
    // What the disk holds now, read and shown before the screen is checked: a status read
    // landing meanwhile (the work in progress's row coming or going) would move the rows.
    for repo in &mut harness.state_mut().state.repos {
        repo.work_tree.refresh_now();
    }
    settle(harness, "the work tree read", |app| {
        app.state
            .repos
            .iter_mut()
            .all(|repo| repo.work_tree.is_settled())
    });
    harness.run_steps(2);
    // Every element's place, to measure a screen to the point (`<name>.rects`).
    if std::env::var_os("EGIT_GUI_RECTS").is_some() {
        super::audit::dump(harness, &screens().join(format!("{name}.rects")));
    }
    super::audit::assert_clean(harness, name);
    super::help::assert_explained(harness, name);
    if !rendering() {
        return;
    }
    // The adapter is shared by every test running: under load a frame's wait can time out.
    let mut tries = 0_u8;
    let image = loop {
        match harness.render() {
            Ok(image) => break image,
            Err(error) if tries < 3 && error.contains("timed out") => {
                tries = tries.saturating_add(1);
            }
            Err(error) => panic!("{name}: {error}"),
        }
    };
    image.save(screens().join(format!("{name}.png"))).unwrap();
}

/// Fails the test, saving the screen it stopped at (`timed-out-<test>.png`) where it can.
fn gave_up(harness: &mut Harness<'_, EgitApp>, why: &str) -> ! {
    if rendering()
        && let Ok(image) = harness.render()
    {
        let thread = std::thread::current();
        let test = thread
            .name()
            .unwrap_or("test")
            .rsplit("::")
            .next()
            .unwrap_or("test");
        let _saved = image.save(screens().join(format!("timed-out-{test}.png")));
    }
    panic!("{why}");
}

/// Steps the application until `done` holds (jobs finish on their own threads).
pub(super) fn settle(
    harness: &mut Harness<'_, EgitApp>,
    what: &str,
    done: impl Fn(&mut EgitApp) -> bool,
) {
    for _ in 0..2_000 {
        harness.step();
        if done(harness.state_mut()) {
            // Let the frame that saw the result lay itself out.
            harness.run_steps(4);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    gave_up(harness, &format!("timed out waiting for {what}"));
}

/// Steps until something labelled `label` is shown (a job's result, say), or panics.
pub(super) fn wait_for_label(harness: &mut Harness<'_, EgitApp>, label: &str) {
    for _ in 0..2_000 {
        harness.step();
        if harness.query_all_by_label(label).next().is_some() {
            // A window's first frame only sizes it (its controls don't respond yet).
            harness.run_steps(2);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    gave_up(harness, &format!("timed out waiting for {label:?}"));
}

/// Steps until the button labelled `label` can be pressed (a job holding it back is done),
/// then presses it.
pub(super) fn click_when_enabled(harness: &mut Harness<'_, EgitApp>, label: &str) {
    for _ in 0..2_000 {
        harness.step();
        if harness
            .query_by_label(label)
            .is_some_and(|button| !button.accesskit_node().is_disabled())
        {
            harness.get_by_label(label).click();
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    gave_up(
        harness,
        &format!("timed out waiting for {label:?} to be enabled"),
    );
}

/// Opens the stash menu's row for the stash whose message starts with `message` (the menu
/// open), showing its Apply, Pop, and Drop….
pub(super) fn open_stash(harness: &mut Harness<'_, EgitApp>, message: &str) {
    let shown = |harness: &Harness<'_, EgitApp>, click: bool| {
        let row = harness.query_all_by_role(Role::Button).find(|node| {
            node.accesskit_node()
                .label()
                .is_some_and(|label| label.starts_with(message))
        });
        if click && let Some(row) = &row {
            row.click();
        }
        row.is_some()
    };
    for _ in 0..2_000 {
        harness.step();
        if shown(harness, false) {
            // A menu's first frame only sizes it.
            harness.run_steps(2);
            shown(harness, true);
            wait_for_label(harness, "Pop");
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    gave_up(
        harness,
        &format!("timed out waiting for the stash {message:?}"),
    );
}

pub(super) fn profile_loaded(app: &mut EgitApp) -> bool {
    app.state.user_config.is_ok()
}

fn repository_shown(app: &mut EgitApp) -> bool {
    app.state
        .repos
        .first_mut()
        .is_some_and(|repo| repo.status.is_ok() && repo.config.is_ok())
}

/// Whether the first repository's history is read, with the summaries of the rows on screen.
pub(super) fn history_shown(app: &mut EgitApp) -> bool {
    app.state.repos.first_mut().is_some_and(|repo| {
        !repo.history.is_loading()
            && !repo.history.summaries.is_reading()
            && (repo.history.shown.rows() == 0 || repo.history.summaries.len() > 0)
    })
}

/// Whether the first repository's history shows `rows` commits, their summaries read.
pub(super) fn history_of(rows: u64) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        history_shown(app)
            && app
                .state
                .repos
                .first()
                .is_some_and(|repo| repo.history.shown.rows() == rows)
    }
}

/// Clicks a repository's panel tab.
fn show_panel(harness: &mut Harness<'_, EgitApp>, panel: &str) {
    harness
        .get_by_role_and_label(Role::Button, "Repository")
        .click();
    wait_for_label(harness, panel);
    harness.get_by_label_contains(panel).click();
    harness.run_steps(4);
}

/// Types `path` into Home's path field and opens it.
pub(super) fn open_path(harness: &mut Harness<'_, EgitApp>, path: &Path) {
    let field = harness.get_by_role_and_label(Role::TextInput, "Open a repository");
    field.focus();
    field.type_text(&path.display().to_string());
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Button, "Open").click();
    harness.run_steps(2);
}

#[test]
fn home_offers_to_open_repositories_and_shows_who_you_are() {
    let sandbox = Sandbox::new().unwrap();
    for (theme, name) in [
        (egui::Theme::Dark, "home-dark"),
        (egui::Theme::Light, "home-light"),
    ] {
        let mut harness = harness_with(sandbox.git().clone(), sandbox.home(), theme);
        settle(&mut harness, "the profile", profile_loaded);
        harness.get_by_label_contains("Open folder");
        harness.get_by_label("Recent repositories");
        harness.get_by_label("Repositories you open appear here.");
        harness.get_by_label("egit tests <tests@egit.invalid>");
        harness.get_by_label_contains("git 2.");
        render(&mut harness, name);
    }
}

#[test]
fn a_typed_path_opens_its_repository() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("project").unwrap();
    sandbox
        .commit_file(&repo, "README", "hello\n", "Start the project")
        .unwrap();
    let inside = repo.join("docs");
    std::fs::create_dir(&inside).unwrap();

    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &inside);
    settle(&mut harness, "the repository", repository_shown);
    settle(&mut harness, "the history", history_shown);
    harness.get_by_role_and_label(Role::Button, "Start the project");
    show_panel(&mut harness, "Overview");

    harness.get_by_label("project");
    harness.get_by_label_contains("main");
    harness.get_by_label("Start the project");
    harness.get_by_label("None: this branch doesn't track another");
    harness.get_by_label("egit tests");
    render(&mut harness, "repository-overview");

    let app = harness.state();
    assert_eq!(app.state.settings.recent.len(), 1);
    assert_eq!(app.state.settings.recent[0].path.to_path(), repo);

    harness.get_by_label_contains("Settings").click();
    harness.run_steps(4);
    assert!(harness.get_all_by_label("user.email").count() >= 1);
    render(&mut harness, "repository-settings");
}

#[test]
fn a_folder_outside_any_repository_says_so() {
    let sandbox = Sandbox::new().unwrap();
    let plain = sandbox.path("plain");
    std::fs::create_dir(&plain).unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &plain);
    settle(&mut harness, "the problem", |app| {
        app.state.home.problem.is_some()
    });
    harness.get_by_label_contains("isn't inside a git repository");
    assert!(harness.query_by_label("Trust this folder").is_none());
    render(&mut harness, "home-not-a-repository");
}

#[test]
fn a_repository_another_user_owns_opens_once_trusted() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("theirs").unwrap();
    let suspicious = sandbox
        .git_with([("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")])
        .unwrap();
    let mut harness = harness_with(suspicious, sandbox.home(), egui::Theme::Dark);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the problem", |app| {
        app.state.home.problem.is_some()
    });
    harness.get_by_label_contains("Another user owns");
    render(&mut harness, "home-dubious-ownership");

    harness.get_by_label("Trust this folder").click();
    settle(&mut harness, "the repository", repository_shown);
    settle(&mut harness, "the history", history_shown);
    harness.get_by_label("No commits yet.");
    show_panel(&mut harness, "Overview");
    harness.get_by_label("No commits yet");
}

#[test]
fn the_profile_shows_where_settings_come_from_and_saves_edits() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    harness.get_by_label_contains("Edit your profile").click();
    harness.run_steps(4);
    harness.get_by_label("Your git profile");
    assert!(
        harness.get_all_by_label("global · ~/.gitconfig").count() >= 2,
        "the name and email should say where they're set"
    );
    harness.get_by_label(
        "No credential helper: git asks for passwords each time, and egit relays the question.",
    );
    render(&mut harness, "profile");

    let name = harness.get_all_by_role(Role::TextInput).next().unwrap();
    assert_eq!(name.value().as_deref(), Some("egit tests"));
    name.focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness
        .get_all_by_role(Role::TextInput)
        .next()
        .unwrap()
        .type_text("Zoë Example");
    harness.run_steps(2);
    harness.get_by_label("Unsaved changes.");
    harness.get_by_label_contains("Save").click();
    settle(&mut harness, "the save", |app| {
        !app.state.profile.saving.is_pending()
            && app.state.user_config.is_ok()
            && app
                .state
                .user_config
                .ok_ref()
                .and_then(|snapshot| {
                    snapshot
                        .profile()
                        .name()
                        .map(|name| name.text() == "Zoë Example")
                })
                .unwrap_or(false)
    });

    let written = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User).unwrap();
    assert_eq!(written.profile().name().unwrap().text(), "Zoë Example");
    assert!(harness.query_by_label("Unsaved changes.").is_none());
}

#[test]
fn an_invalid_email_cant_be_saved() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    harness.get_by_label_contains("Edit your profile").click();
    harness.run_steps(4);
    let email = harness.get_all_by_role(Role::TextInput).nth(1).unwrap();
    email.focus();
    email.type_text(" <oops>");
    harness.run_steps(2);
    harness.get_by_label("An email can't contain spaces, < or >, or line breaks.");
}

/// The application with git's questions relayed through a server in the sandbox.
fn harness_with_prompts(sandbox: &Sandbox) -> Harness<'static, EgitApp> {
    let git = sandbox.git().clone();
    let home = sandbox.home().to_owned();
    let base = sandbox.path("askpass");
    std::fs::create_dir(&base).unwrap();
    egit_git::testing::set_mode(&base, 0o700).unwrap();
    let builder = Harness::builder().with_size([1180.0, 800.0]);
    let builder = if rendering() { builder.wgpu() } else { builder };
    builder.build_eframe(move |creation| {
        let (prompts, _) =
            Prompts::start_with(&creation.egui_ctx, Path::new("/usr/bin/egit"), Some(&base));
        EgitApp::with_git(
            &creation.egui_ctx,
            git,
            Settings::default(),
            Some(home),
            prompts,
        )
    })
}

/// Asks the application `prompt` from another thread, as git's helper would.
fn ask(
    harness: &Harness<'_, EgitApp>,
    kind: PromptKind,
    prompt: &'static str,
    context: &'static str,
) -> std::thread::JoinHandle<Option<String>> {
    let environment = harness.state().state.prompts.environment();
    std::thread::spawn(move || {
        ask_as_helper(&environment, kind, prompt, context)
            .unwrap()
            .map(|answer| String::from_utf8(answer.to_vec()).unwrap())
    })
}

#[test]
fn a_password_is_asked_in_the_window() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness_with_prompts(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    let helper = ask(
        &harness,
        PromptKind::Secret,
        "Password for 'https://me@example.com': ",
        "Fetching origin",
    );
    settle(&mut harness, "the question", |app| {
        app.state.prompts.waiting() == 1
    });
    harness.get_by_label_contains("A password or passphrase is needed");
    harness.get_by_label("While fetching origin");
    harness.get_by_label("Password for 'https://me@example.com':");
    let field = harness.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text("correct horse");
    harness.run_steps(2);
    render(&mut harness, "prompt-password");

    harness.get_by_role_and_label(Role::Button, "OK").click();
    harness.run_steps(2);
    assert_eq!(helper.join().unwrap().as_deref(), Some("correct horse"));
    settle(&mut harness, "the dialog to close", |app| {
        app.state.prompts.waiting() == 0
    });
    assert!(
        harness
            .query_by_label_contains("A password or passphrase")
            .is_none()
    );
}

#[test]
fn a_cancelled_question_is_refused() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness_with_prompts(&sandbox);
    let helper = ask(
        &harness,
        PromptKind::Visible,
        "Username for 'https://example.com': ",
        "",
    );
    settle(&mut harness, "the question", |app| {
        app.state.prompts.waiting() == 1
    });
    harness.get_by_label_contains("A user name is needed");
    assert!(harness.query_by_label_contains("While").is_none());
    harness
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    harness.run_steps(2);
    assert_eq!(helper.join().unwrap(), None);
}

#[test]
fn a_host_question_is_answered_yes_or_no() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness_with_prompts(&sandbox);
    let helper = ask(
        &harness,
        PromptKind::YesNo,
        "The authenticity of host 'example.com' can't be established.\n\
         ED25519 key fingerprint is SHA256:Hjw2Wr8Y5xKKgWYH3GSaNDfzTtFjk6RoFKlJmUDwTGM.\n\
         Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
        "Cloning example.com/repo",
    );
    settle(&mut harness, "the question", |app| {
        app.state.prompts.waiting() == 1
    });
    harness.get_by_label_contains("Your decision is needed");
    render(&mut harness, "prompt-host");
    harness.get_by_role_and_label(Role::Button, "Yes").click();
    harness.run_steps(2);
    assert_eq!(helper.join().unwrap().as_deref(), Some("yes"));
}

#[test]
fn work_done_outside_egit_shows_up_by_itself() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("project").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the repository", repository_shown);
    settle(&mut harness, "the history", history_shown);
    harness.get_by_role_and_label(Role::Button, "First").click();
    harness.run_steps(2);

    // As from a terminal: a commit, then a new branch checked out, then a setting.
    sandbox
        .commit_file(&repo, "file", "two\n", "Second, from a terminal")
        .unwrap();
    settle(&mut harness, "the new commit", |app| {
        shown_subject(app).as_deref() == Some("Second, from a terminal")
            && app.state.repos[0].history.shown.rows() == 2
            && history_shown(app)
    });
    harness.get_by_role_and_label(Role::Button, "Second, from a terminal");
    assert_eq!(
        harness.state().state.repos[0].history_view.selected_row(),
        Some(1),
        "the selected commit stays selected, a row down"
    );
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "topic"])
        .unwrap();
    settle(&mut harness, "the new branch", |app| {
        shown_branch(app).as_deref() == Some("topic") && history_shown(app)
    });
    harness.get_by_label(&format!("{} topic", icon::GIT_BRANCH));
    sandbox
        .run(&repo, "config", &["user.email", "elsewhere@egit.invalid"])
        .unwrap();
    settle(&mut harness, "the new setting", |app| {
        app.state.repos[0]
            .config
            .ok_ref()
            .and_then(|snapshot| {
                snapshot
                    .profile()
                    .email()
                    .map(|email| email.text() == "elsewhere@egit.invalid")
            })
            .unwrap_or(false)
    });
    show_panel(&mut harness, "Overview");
    harness.get_by_label_contains("topic");
    harness.get_by_label("elsewhere@egit.invalid");
}

/// A history with branches, a merge, and a tag: `main` merged `parser` and goes on to `docs`.
pub(super) fn branchy(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("project").unwrap();
    sandbox
        .commit_file(&repo, "README", "hello\n", "Start the project")
        .unwrap();
    sandbox.run(&repo, "tag", &["v0.1"]).unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "parser"])
        .unwrap();
    sandbox
        .commit_file(&repo, "parser.rs", "fn parse() {}\n", "Add a parser")
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "parser.rs",
            "fn parse() -> bool {\n    true\n}\n",
            "Make the parser answer",
        )
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&repo, "README", "hello, world\n", "Greet the world")
        .unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &[
                "--quiet",
                "--no-ff",
                "--message",
                "Merge the parser",
                "parser",
            ],
        )
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "docs"])
        .unwrap();
    sandbox
        .commit_file(&repo, "docs.md", "# Docs\n", "Start the docs")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    repo
}

#[test]
fn the_history_shows_branches_merges_and_a_commits_changes() {
    let sandbox = Sandbox::new().unwrap();
    let repo = branchy(&sandbox);
    for (theme, name) in [
        (egui::Theme::Dark, "history-dark"),
        (egui::Theme::Light, "history-light"),
    ] {
        let mut harness = harness_with(sandbox.git().clone(), sandbox.home(), theme);
        settle(&mut harness, "the profile", profile_loaded);
        open_path(&mut harness, &repo);
        settle(&mut harness, "the history", history_shown);
        harness.get_by_label("6 commits");
        for subject in [
            "Start the docs",
            "Merge the parser",
            "Greet the world",
            "Make the parser answer",
            "Add a parser",
            "Start the project",
        ] {
            harness.get_by_role_and_label(Role::Button, subject);
        }
        for badge in [
            format!("{} main", icon::GIT_BRANCH),
            format!("{} parser", icon::GIT_BRANCH),
            format!("{} docs", icon::GIT_BRANCH),
            format!("{} v0.1", icon::TAG),
        ] {
            harness.get_by_label(&badge);
        }
        harness.get_by_label("Select a commit to see what it changed.");
        render(&mut harness, name);
    }

    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness
        .get_by_role_and_label(Role::Button, "Add a parser")
        .click();
    settle(&mut harness, "the commit's details", |app| {
        app.state.repos[0].history_view.details_shown()
    });
    settle(&mut harness, "the lines counted", |app| {
        app.state.repos[0].history_view.counts_shown()
    });
    harness.get_by_label("1 file changed");
    harness.get_by_role_and_label(Role::Label, "parser.rs");
    // How it changed: a letter, named in words.
    harness.get_by_role_and_label(Role::Button, "Added");
    // The commit's lines, and the file's.
    assert_eq!(harness.query_all_by_label("+1").count(), 2);
    assert_eq!(harness.query_all_by_label("−0").count(), 2);
    // Who wrote it: the name, and the email beneath.
    harness.get_by_label("tests@egit.invalid");
    render(&mut harness, "history-commit");

    // Down a row and back with the keyboard, then to its parent (the first commit) by its link.
    let row = harness.state().state.repos[0]
        .history_view
        .selected_row()
        .unwrap();
    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness, "the next commit's details", |app| {
        app.state.repos[0].history_view.selected_row() == Some(row + 1)
            && app.state.repos[0].history_view.details_shown()
    });
    // Its row, the row's cell, and the details' heading.
    assert_eq!(harness.get_all_by_label("Greet the world").count(), 3);
    harness.key_press(egui::Key::ArrowUp);
    settle(&mut harness, "the commit's details again", |app| {
        app.state.repos[0].history_view.selected_row() == Some(row)
            && app.state.repos[0].history_view.details_shown()
    });
    let first = harness.state().state.repos[0].history.shown.id(5).unwrap();
    harness
        .get_by_role_and_label(Role::Button, &first.abbreviated(8).to_string())
        .click();
    settle(&mut harness, "the parent's details", |app| {
        app.state.repos[0].history_view.selected_row() == Some(5)
            && app.state.repos[0].history_view.details_shown()
    });
    harness.get_by_label("None: the first commit");
}

/// The find bar's field.
fn find_field<'h>(harness: &'h Harness<'_, EgitApp>) -> egui_kittest::Node<'h> {
    harness.get_by_role_and_label(Role::TextInput, "Find")
}

/// Whether the first repository shows `parser.rs`, ready.
fn parser_shown(app: &mut EgitApp) -> bool {
    app.state.repos[0].history_view.file_shown() == Some(("parser.rs".to_owned(), true))
}

/// The application with `branchy`'s "Make the parser answer" commit's `parser.rs` open.
fn parser_change(sandbox: &Sandbox) -> Harness<'static, EgitApp> {
    let repo = branchy(sandbox);
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness
        .get_by_role_and_label(Role::Button, "Make the parser answer")
        .click();
    settle(&mut harness, "the commit's details", |app| {
        app.state.repos[0].history_view.details_shown()
    });
    settle(&mut harness, "the lines counted", |app| {
        app.state.repos[0].history_view.counts_shown()
    });
    harness
        .get_by_role_and_label(Role::Button, "parser.rs")
        .click();
    settle(&mut harness, "the diff", parser_shown);
    harness
}

#[test]
fn a_git_lfs_pointers_diff_shows_what_it_stands_for() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let pointer = |digit: &str, size: u64| {
        format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {size}\n",
            digit.repeat(64)
        )
    };
    sandbox
        .commit_file(
            &repo,
            "model.bin",
            &pointer("a", 1_200_000),
            "Train the model",
        )
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "model.bin",
            &pointer("b", 1_500_000),
            "Retrain the model",
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness
        .get_by_role_and_label(Role::Button, "Retrain the model")
        .click();
    settle(&mut harness, "the commit's details", |app| {
        app.state.repos[0].history_view.details_shown()
    });
    harness
        .get_by_role_and_label(Role::Button, "model.bin")
        .click();
    settle(&mut harness, "the diff", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("model.bin".to_owned(), true))
    });
    let stands_for = "A Git LFS file, stored outside the repository: 1.2 MB before, 1.5 MB after.";
    harness.get_by_label(stands_for);
    render(&mut harness, "history-diff-lfs");

    harness
        .get_by_role_and_label(Role::Button, "Show the pointer's changes")
        .click();
    harness.run_steps(2);
    assert!(
        harness.query_by_label(stands_for).is_none(),
        "the pointer's own diff takes the placeholder's place"
    );
}

/// The label of the history's count of files with uncommitted changes.
pub(super) fn uncommitted(files: usize) -> String {
    if files == 1 {
        "1 uncommitted file".to_owned()
    } else {
        format!("{files} uncommitted files")
    }
}

#[test]
fn the_work_tree_is_followed_as_files_change_outside_egit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, ".gitignore", "build/\n", "Ignore builds")
        .unwrap();
    sandbox
        .commit_file(&repo, "src/lib.rs", "one\n", "Add a library")
        .unwrap();
    std::fs::write(repo.join(".gitignore"), "build/\n*.log\n").unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    let shown = |files: usize| {
        let label = uncommitted(files);
        move |harness: &mut Harness<'_, EgitApp>| wait_for_label(harness, &label)
    };
    shown(1)(&mut harness);

    // A file in folders made at once.
    std::fs::create_dir_all(repo.join("notes/drafts")).unwrap();
    std::fs::write(repo.join("notes/drafts/idea.md"), "x\n").unwrap();
    shown(2)(&mut harness);
    // Another in a folder now watched.
    std::fs::write(repo.join("notes/drafts/second.md"), "x\n").unwrap();
    shown(3)(&mut harness);

    // Empty folders are watched once seen, so a file put in them later is seen too.
    std::fs::create_dir_all(repo.join("later/deeper")).unwrap();
    let deeper = repo.join("later/deeper");
    settle(&mut harness, "the new folder watched", move |app| {
        app.state.repos[0].work_tree.watches(&deeper)
    });
    std::fs::write(repo.join("later/deeper/third.md"), "x\n").unwrap();
    shown(4)(&mut harness);

    // An ignored folder isn't watched: a build's output costs nothing.
    std::fs::create_dir_all(repo.join("build/out")).unwrap();
    std::fs::write(repo.join("debug.log"), "x\n").unwrap();
    std::fs::write(repo.join("README"), "x\n").unwrap();
    shown(5)(&mut harness);
    let work_tree = &harness.state().state.repos[0].work_tree;
    assert!(!work_tree.watches(&repo.join("build")));
    assert!(!work_tree.watches(&repo.join("build/out")));
    assert!(work_tree.watches(&repo.join("later")));

    // A tracked folder deleted and made again (as a checkout does) is watched again.
    std::fs::remove_dir_all(repo.join("src")).unwrap();
    shown(6)(&mut harness);
    std::fs::create_dir(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "one\n").unwrap();
    shown(5)(&mut harness);
    let src = repo.join("src");
    settle(&mut harness, "the folder watched again", move |app| {
        app.state.repos[0].work_tree.watches(&src)
    });
    std::fs::write(repo.join("src/lib.rs"), "two\n").unwrap();
    shown(6)(&mut harness);
}

/// Drags across the open file's diff from `from` to `to` (each a line and a cell).
fn drag(harness: &mut Harness<'_, EgitApp>, from: (u32, u32), to: (u32, u32)) {
    let spot = |harness: &mut Harness<'_, EgitApp>, (line, cell): (u32, u32)| {
        harness.state_mut().state.repos[0]
            .history_view
            .file_spot(line, cell)
            .unwrap()
    };
    let (from, to) = (spot(harness, from), spot(harness, to));
    harness.hover_at(from);
    harness.drag_at(from);
    harness.run_steps(1);
    harness.hover_at(to);
    harness.drop_at(to);
    harness.run_steps(2);
}

#[test]
fn the_work_tree_is_staged_by_file_by_line_and_by_hunk() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let notes = |changed: &[u32]| -> String {
        (1..=30)
            .map(|line| {
                if changed.contains(&line) {
                    format!("LINE {line}\n")
                } else {
                    format!("line {line}\n")
                }
            })
            .collect()
    };
    sandbox
        .commit_file(&repo, "notes.txt", &notes(&[]), "Write notes")
        .unwrap();
    std::fs::write(repo.join("notes.txt"), notes(&[5, 25])).unwrap();
    std::fs::write(repo.join("todo.md"), "- stage this\n").unwrap();
    let staged = |sandbox: &Sandbox| -> String {
        let out = sandbox
            .run(&repo, "diff", &["--cached", "--no-color"])
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for_label(&mut harness, &uncommitted(2));
    harness.get_by_label(&uncommitted(2)).click();
    wait_for_label(&mut harness, "Not staged");

    // A whole file, by its own button.
    harness.get_by_label("Stage todo.md").click();
    wait_for_label(&mut harness, "Unstage todo.md");
    assert!(staged(&sandbox).contains("+- stage this"));
    render(&mut harness, "work-tree");

    // A line: the first hunk's change, picked by dragging across it. The diff's lines: a hunk
    // header, three lines of context, `-line 5`, `+LINE 5`, three more, the next hunk.
    harness
        .get_by_role_and_label(Role::Button, "notes.txt")
        .click();
    settle(&mut harness, "the diff", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("notes.txt".to_owned(), true))
    });
    drag(&mut harness, (4, 0), (5, 3));
    harness.get_by_label("Stage lines").click();
    settle(&mut harness, "the line staged", |_| {
        staged(&sandbox).contains("+LINE 5")
    });
    assert!(!staged(&sandbox).contains("LINE 25"));
    // The diff follows: the second hunk is all that is left to stage.
    settle(&mut harness, "the diff read again", |app| {
        app.state.repos[0].history_view.diff_lines() == Some(9)
    });
    render(&mut harness, "work-tree-diff");

    // A hunk: a line of it picks all of it.
    drag(&mut harness, (4, 0), (4, 3));
    harness.get_by_label("Stage hunk").click();
    settle(&mut harness, "the hunk staged", |_| {
        staged(&sandbox).contains("+LINE 25")
    });

    // And everything back, once the lists show it all staged (the staged list moves up).
    wait_for_label(&mut harness, "Nothing to stage.");
    click_when_enabled(&mut harness, "Unstage all");
    settle(&mut harness, "everything unstaged", |_| {
        staged(&sandbox).is_empty()
    });
    wait_for_label(&mut harness, "Nothing staged yet.");
}

/// Types `text` into the field of `role` labelled `label`.
pub(super) fn type_into(harness: &mut Harness<'_, EgitApp>, role: Role, label: &str, text: &str) {
    let field = harness.get_by_role_and_label(role, label);
    field.focus();
    field.type_text(text);
    harness.run_steps(2);
}

#[test]
fn staged_changes_are_committed_and_amended_from_the_form() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "notes", "one\n", "Start")
        .unwrap();
    std::fs::write(repo.join("notes"), "two\n").unwrap();
    std::fs::write(repo.join("todo"), "x\n").unwrap();
    let last = |sandbox: &Sandbox, format: &str| -> String {
        let out = sandbox
            .run(&repo, "log", &["-1", &format!("--format={format}")])
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for_label(&mut harness, &uncommitted(2));
    harness.get_by_label(&uncommitted(2)).click();
    wait_for_label(&mut harness, "Stage all");
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 2 files");

    type_into(
        &mut harness,
        Role::TextInput,
        "Summary",
        "Write the notes down",
    );
    type_into(
        &mut harness,
        Role::MultilineTextInput,
        "Description",
        "# Why\nBecause.",
    );
    render(&mut harness, "work-tree-commit");
    harness.get_by_label("Commit 2 files").click();
    wait_for_label(&mut harness, "2 commits");
    assert_eq!(
        last(&sandbox, "%B"),
        "Write the notes down\n\n# Why\nBecause."
    );
    // The form starts over, and nothing is left to commit.
    wait_for_label(&mut harness, "Nothing staged yet.");

    // Amending starts from the last commit's message.
    std::fs::write(repo.join("notes"), "three\n").unwrap();
    // Listed (the button for all is there, disabled, before it is).
    wait_for_label(&mut harness, "Stage notes");
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 1 file");
    harness.get_by_label("Amend").click();
    wait_for_label(&mut harness, "Amend the last commit");
    for _ in 0..200 {
        harness.step();
        if harness
            .query_by_role_and_label(Role::TextInput, "Summary")
            .and_then(|field| field.value())
            .is_some_and(|value| value == "Write the notes down")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.get_by_label("Amend the last commit").click();
    settle(&mut harness, "the amended commit", |_| {
        last(&sandbox, "%s") == "Write the notes down"
            && sandbox
                .run(&repo, "show", &["HEAD:notes"])
                .is_ok_and(|out| out.stdout == b"three\n")
    });
    let count = sandbox
        .run(&repo, "rev-list", &["--count", "HEAD"])
        .unwrap();
    assert_eq!(count.stdout.trim_ascii(), b"2", "amended, not added");

    // A hook that refuses says why, in full.
    let hook = repo.join(".git/hooks/commit-msg");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho 'rejected: say why'\nexit 1\n").unwrap();
    egit_git::testing::set_mode(&hook, 0o755).unwrap();
    std::fs::write(repo.join("notes"), "four\n").unwrap();
    wait_for_label(&mut harness, "Stage notes");
    harness.get_by_label("Stage all").click();
    wait_for_label(&mut harness, "Commit 1 file");
    type_into(&mut harness, Role::TextInput, "Summary", "Refused");
    harness.get_by_label("Commit 1 file").click();
    wait_for_label(&mut harness, "rejected: say why");
    assert_eq!(last(&sandbox, "%s"), "Write the notes down");
}

#[test]
fn signed_commits_show_whether_their_signatures_hold() {
    let has_ssh_keygen = std::process::Command::new("ssh-keygen")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    assert!(
        has_ssh_keygen || std::env::var_os("EGIT_REQUIRE_SIGNERS").is_none(),
        "ssh-keygen is needed to test signatures"
    );
    if !has_ssh_keygen {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let key = sandbox.home().join("id_ed25519");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "tests", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(made.success());
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let allowed = sandbox.home().join("allowed_signers");
    std::fs::write(
        &allowed,
        format!("tests@egit.invalid namespaces=\"git\" {}", public.trim()),
    )
    .unwrap();
    let repo = sandbox.init("repo").unwrap();
    for setting in [
        ["gpg.format", "ssh"],
        ["user.signingKey", key.to_str().unwrap()],
        ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
        ["commit.gpgSign", "true"],
    ] {
        sandbox.run(&repo, "config", &setting).unwrap();
    }
    sandbox.commit_file(&repo, "a", "x\n", "Signed").unwrap();
    sandbox
        .run(&repo, "config", &["commit.gpgSign", "false"])
        .unwrap();
    sandbox.commit_file(&repo, "b", "x\n", "Plain").unwrap();

    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    // Checked once on screen: one seal, the signed commit's.
    wait_for_label(&mut harness, icon::SEAL_CHECK);
    assert_eq!(harness.query_all_by_label(icon::SEAL_CHECK).count(), 1);
    harness
        .get_by_role_and_label(Role::Button, "Signed")
        .click();
    wait_for_label(
        &mut harness,
        &format!("{}  Good signature", icon::SEAL_CHECK),
    );
    harness.get_by_label("by tests@egit.invalid");
    // The details settled (their lines counted) before they are looked at control by control.
    settle(&mut harness, "the lines counted", |app| {
        app.state.repos[0].history_view.counts_shown()
    });
    render(&mut harness, "history-signed");
}

/// A bare repository with a commit on `main`, a colleague's clone of it, and mine (the one
/// egit opens), both by plain git.
pub(super) fn shared_remote(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let bare = sandbox.path("origin.git");
    sandbox
        .run(
            sandbox.home(),
            "init",
            &["--quiet", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
    let url = format!("file://{}", bare.display());
    let seed = sandbox.init("seed").unwrap();
    sandbox
        .commit_file(&seed, "shared", "base\n", "Start")
        .unwrap();
    sandbox
        .run(&seed, "push", &["--quiet", &url, "HEAD:refs/heads/main"])
        .unwrap();
    let clone = |name: &str| {
        let path = sandbox.path(name);
        sandbox
            .run(
                sandbox.home(),
                "clone",
                &["--quiet", &url, path.to_str().unwrap()],
            )
            .unwrap();
        path
    };
    (clone("colleague"), clone("mine"))
}

/// The colleague commits `file` and pushes it.
pub(super) fn colleague_pushes(sandbox: &Sandbox, colleague: &Path, file: &str, contents: &str) {
    sandbox
        .run(colleague, "pull", &["--quiet", "--rebase"])
        .unwrap();
    sandbox
        .commit_file(colleague, file, contents, &format!("Colleague's {file}"))
        .unwrap();
    sandbox.run(colleague, "push", &["--quiet"]).unwrap();
}

#[test]
fn fetches_pulls_and_pushes_from_the_history() {
    let sandbox = Sandbox::new().unwrap();
    let (colleague, mine) = shared_remote(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &mine);
    settle(&mut harness, "the history", history_shown);
    // Only the fetches asked for: one in the background would move the branch's counts while
    // a screen is checked.
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();
    let branch = |compared: &str| {
        format!(
            "{}  main {} origin/main{compared}",
            icon::GIT_BRANCH,
            icon::ARROW_RIGHT
        )
    };
    wait_for_label(&mut harness, &branch(""));

    // Fetched, the branch is behind; pulled, it isn't.
    colleague_pushes(&sandbox, &colleague, "theirs", "x\n");
    click_when_enabled(&mut harness, &format!("{}  Fetch", icon::ARROWS_CLOCKWISE));
    wait_for_label(&mut harness, &branch(&format!(" {}1", icon::ARROW_DOWN)));
    click_when_enabled(&mut harness, &format!("{}  Pull", icon::ARROW_DOWN));
    wait_for_label(&mut harness, "Pulled.");
    wait_for_label(&mut harness, &branch(""));
    settle(&mut harness, "the pulled history", history_of(2));

    // Behind again when pushing: refused, the pull it suggests asks how to bring the
    // diverged branches together.
    colleague_pushes(&sandbox, &colleague, "theirs", "y\n");
    sandbox.commit_file(&mine, "mine", "m\n", "Mine").unwrap();
    click_when_enabled(&mut harness, &format!("{}  Push", icon::ARROW_UP));
    wait_for_label(&mut harness, "Force push…");
    // The commit made shown, and the branch ahead by it (the refused push moved nothing).
    wait_for_label(&mut harness, "Mine");
    wait_for_label(&mut harness, &branch(&format!(" {}1", icon::ARROW_UP)));
    settle(&mut harness, "the commit made", history_shown);
    render(&mut harness, "sync-refused");
    harness.get_by_label("Pull").click();
    wait_for_label(&mut harness, "The branches have diverged");
    settle(&mut harness, "the fetched history", history_of(4));
    render(&mut harness, "sync-diverged");
    harness.get_by_label("Rebase").click();
    wait_for_label(&mut harness, "Pulled.");
    click_when_enabled(&mut harness, &format!("{}  Push", icon::ARROW_UP));
    wait_for_label(&mut harness, "Pushed to main.");
    let pushed = sandbox
        .run(&colleague, "fetch", &["--quiet"])
        .and_then(|_| sandbox.run(&colleague, "log", &["-1", "--format=%s", "origin/main"]));
    assert_eq!(pushed.unwrap().stdout.trim_ascii(), b"Mine");

    // Amended after pushing: refused, then forced with a lease.
    sandbox
        .run(
            &mine,
            "commit",
            &["--quiet", "--amend", "--message", "Mine, amended"],
        )
        .unwrap();
    click_when_enabled(&mut harness, &format!("{}  Push", icon::ARROW_UP));
    wait_for_label(&mut harness, "Force push…");
    harness.get_by_label("Force push…").click();
    wait_for_label(&mut harness, "Force push to origin/main?");
    harness.get_by_label("Force push").click();
    wait_for_label(&mut harness, "Force pushed: main replaced.");

    // A new branch's first push says where it goes.
    sandbox
        .run(&mine, "switch", &["--quiet", "--create", "topic"])
        .unwrap();
    wait_for_label(
        &mut harness,
        &format!("{}  topic (not pushed yet)", icon::GIT_BRANCH),
    );
    click_when_enabled(&mut harness, &format!("{}  Push", icon::ARROW_UP));
    wait_for_label(&mut harness, "Push topic for the first time");
    // The history shows the new branch.
    wait_for_label(&mut harness, &format!("{} topic", icon::GIT_BRANCH));
    render(&mut harness, "sync-first-push");
    harness.get_by_label("Push").click();
    wait_for_label(&mut harness, "Pushed: topic is new on the remote.");
    wait_for_label(
        &mut harness,
        &format!(
            "{}  topic {} origin/topic",
            icon::GIT_BRANCH,
            icon::ARROW_RIGHT
        ),
    );
}

#[test]
fn a_pull_stopped_at_conflicts_is_shown_and_aborted() {
    let sandbox = Sandbox::new().unwrap();
    let (colleague, mine) = shared_remote(&sandbox);
    sandbox
        .run(&mine, "config", &["pull.rebase", "false"])
        .unwrap();
    colleague_pushes(&sandbox, &colleague, "shared", "theirs\n");
    sandbox
        .commit_file(&mine, "shared", "mine\n", "Mine")
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &mine);
    settle(&mut harness, "the history", history_shown);
    click_when_enabled(&mut harness, &format!("{}  Pull", icon::ARROW_DOWN));
    wait_for_label(
        &mut harness,
        &format!("{}  A merge is in progress.", icon::WARNING),
    );
    harness.get_by_label("Show the conflicts").click();
    wait_for_label(&mut harness, "Conflict");
    // The fetched commit shown too: the history is read again after the pull.
    settle(&mut harness, "the fetched history", history_of(3));
    render(&mut harness, "sync-conflicts");

    harness.get_by_label("Abort…").click();
    wait_for_label(&mut harness, "Abort a merge?");
    harness.get_by_label("Abort").click();
    wait_for_label(&mut harness, "Aborted: everything is as it was before.");
    for _ in 0..2_000 {
        harness.step();
        if harness
            .query_by_label(&format!("{}  A merge is in progress.", icon::WARNING))
            .is_none()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let head = sandbox.run(&mine, "log", &["-1", "--format=%s"]).unwrap();
    assert_eq!(head.stdout.trim_ascii(), b"Mine");
}

#[test]
fn a_repository_is_cloned_from_home_and_opened() {
    let sandbox = Sandbox::new().unwrap();
    shared_remote(&sandbox);
    let bare = sandbox.path("origin.git");
    // First from a remote that answers only after a long pause (git's `ext::` transport runs
    // a command as the remote): cancelled, nothing is left.
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.ext.allow", "always"],
        )
        .unwrap();
    let slow = format!(
        "ext::sh -c sleep% 60;% exec% {}% upload-pack% {}",
        sandbox.git().program().display(),
        bare.display()
    );
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    type_into(&mut harness, Role::TextInput, "Address", &slow);
    click_when_enabled(&mut harness, &format!("{}  Clone", icon::DOWNLOAD_SIMPLE));
    wait_for_label(&mut harness, "Cancel");
    render(&mut harness, "home-cloning");
    harness.get_by_label("Cancel").click();
    wait_for_label(&mut harness, "Cancelled: git removed what it had made.");
    assert!(!sandbox.home().join("origin").exists());

    let url = format!("file://{}", bare.display());
    harness
        .get_by_role_and_label(Role::TextInput, "Address")
        .focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(1);
    type_into(&mut harness, Role::TextInput, "Address", &url);
    // The folder made is named as git names it; it goes into the home folder at first.
    let named = harness.get_by_role_and_label(Role::TextInput, "Named");
    assert_eq!(named.accesskit_node().value().as_deref(), Some("origin"));
    wait_for_label(&mut harness, "Into ~/origin");
    render(&mut harness, "home-clone");

    click_when_enabled(&mut harness, &format!("{}  Clone", icon::DOWNLOAD_SIMPLE));
    settle(&mut harness, "the clone opened", history_of(1));
    let cloned = sandbox.home().join("origin");
    assert_eq!(
        std::fs::read_to_string(cloned.join("shared")).unwrap(),
        "base\n"
    );
    let app = harness.state_mut();
    assert_eq!(app.state.repos[0].repository.work_tree(), cloned.as_path());
    // The next clone goes into the same folder.
    assert_eq!(
        app.state
            .settings
            .clone_folder
            .as_ref()
            .map(StoredPath::to_path),
        Some(sandbox.home().to_owned())
    );
}

/// `notes.txt` as the merge editor's test has it, with `alpha` and `beta` on its two lines that
/// conflict.
fn notes(alpha: &str, beta: &str) -> String {
    format!("intro\n{alpha}\nmiddle 1\nmiddle 2\nmiddle 3\nmiddle 4\n{beta}\nend\n")
}

/// A repository stopped merging `side`: `notes.txt` with two conflicts, and `gone`, which `main`
/// deleted and `side` changed.
fn stopped_merging(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "notes.txt", &notes("alpha", "beta"), "Start")
        .unwrap();
    sandbox
        .commit_file(&repo, "gone", "kept\n", "Add gone")
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "notes.txt",
            &notes("alpha (theirs)", "beta (theirs)"),
            "Theirs",
        )
        .unwrap();
    sandbox
        .commit_file(&repo, "gone", "changed\n", "Change gone")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(
            &repo,
            "notes.txt",
            &notes("alpha (ours)", "beta (ours)"),
            "Ours",
        )
        .unwrap();
    sandbox.run(&repo, "rm", &["--quiet", "gone"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Remove gone"])
        .unwrap();
    let merged = sandbox
        .git()
        .write("merge")
        .args(["--quiet", "side"])
        .current_dir(&repo)
        .output_unchecked()
        .unwrap();
    assert!(!merged.status.success());

    repo
}

#[test]
fn conflicts_are_resolved_in_the_merge_editor() {
    let sandbox = Sandbox::new().unwrap();
    let repo = stopped_merging(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.get_by_label("Show the conflicts").click();
    wait_for_label(&mut harness, "notes.txt");
    harness
        .get_by_role_and_label(Role::Button, "notes.txt")
        .click();
    wait_for_label(&mut harness, "Conflict 1 of 2");
    wait_for_label(&mut harness, "· 2 left");
    render(&mut harness, "merge-editor");

    // The incoming side for the first; the pick moves on to the second, edited by hand.
    harness.get_by_label("Incoming").click();
    wait_for_label(&mut harness, "Conflict 2 of 2");
    harness.get_by_label("Edit…").click();
    let editor = "Conflict 2 resolves to (Escape cancels):";
    wait_for_label(&mut harness, editor);
    harness
        .get_by_role_and_label(Role::MultilineTextInput, editor)
        .focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(1);
    type_into(
        &mut harness,
        Role::MultilineTextInput,
        editor,
        "beta (both)\n",
    );
    harness.get_by_label("Apply").click();
    wait_for_label(&mut harness, "· all resolved");
    render(&mut harness, "merge-resolved");
    click_when_enabled(&mut harness, &format!("{}  Save resolution", icon::CHECK));
    let resolved = notes("alpha (theirs)", "beta (both)");
    let index = |file: &str| {
        sandbox
            .run(&repo, "cat-file", &["-p", &format!(":0:{file}")])
            .map(|shown| shown.stdout)
            .unwrap_or_default()
    };
    // Staged, then written to the work tree.
    settle(&mut harness, "the resolution staged and written", |_| {
        index("notes.txt") == resolved.as_bytes()
            && std::fs::read_to_string(repo.join("notes.txt")).is_ok_and(|file| file == resolved)
    });
    // The view goes on to what it stages.
    wait_for_label(&mut harness, "Staged · Modified");

    // A file one side deleted: kept as the other changed it.
    harness
        .get_by_label(&format!("{}  History", icon::ARROW_LEFT))
        .click();
    wait_for_label(&mut harness, "gone");
    harness.get_by_role_and_label(Role::Button, "gone").click();
    wait_for_label(
        &mut harness,
        "The current side deleted the file, and the incoming side changed it.",
    );
    render(&mut harness, "merge-whole");
    harness.get_by_label("Keep the incoming side's").click();
    settle(&mut harness, "the file kept", |_| {
        index("gone") == b"changed\n"
    });

    // Every conflict resolved: the merge carries on.
    click_when_enabled(&mut harness, "Continue");
    wait_for_label(&mut harness, "Carried on.");
    let parents = sandbox.run(&repo, "log", &["-1", "--format=%P"]).unwrap();
    assert_eq!(parents.stdout.split(|&byte| byte == b' ').count(), 2);
}

/// Double-clicks the control labelled `label`: both clicks in one frame (each of the harness's
/// frames is a quarter of a second: two apart are no double-click).
pub(super) fn double_click(harness: &mut Harness<'_, EgitApp>, label: &str) {
    harness.get_by_label(label).hover();
    harness.step();
    let at = harness.get_by_label(label).rect().center();
    for pressed in [true, false, true, false] {
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
}

/// Right-clicks the control labelled `label`, then clicks `item` in the menu it opens.
pub(super) fn menu_item(harness: &mut Harness<'_, EgitApp>, label: &str, item: &str) {
    harness.get_by_label(label).click_secondary();
    wait_for_label(harness, item);
    harness.get_by_label(item).click();
}

/// `main` with `f` changed twice, and `other` from the first commit adding `g`.
pub(super) fn two_branches(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "other"])
        .unwrap();
    sandbox.commit_file(&repo, "g", "other\n", "Other").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    repo
}

/// The branches in `repo`.
fn branch_names(sandbox: &Sandbox, repo: &Path) -> String {
    let listed = sandbox
        .run(repo, "branch", &["--format=%(refname:short)"])
        .unwrap();
    String::from_utf8(listed.stdout).unwrap()
}

#[test]
fn branches_are_switched_to_made_and_deleted_from_the_graph() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    let badge = |name: &str| format!("{} {name}", icon::GIT_BRANCH);
    let on = |name: &str| format!("{}  {name} (not pushed yet)", icon::GIT_BRANCH);
    wait_for_label(&mut harness, &on("main"));

    // A branch's menu switches to it; double-clicking another switches back.
    harness.get_by_label(&badge("other")).click_secondary();
    wait_for_label(&mut harness, "Switch to other");
    render(&mut harness, "branch-menu");
    harness.get_by_label("Switch to other").click();
    wait_for_label(&mut harness, "Switched to other.");
    wait_for_label(&mut harness, &on("other"));
    double_click(&mut harness, &badge("main"));
    wait_for_label(&mut harness, &on("main"));

    // A file in the way is stashed first, once asked.
    std::fs::write(repo.join("g"), "untracked\n").unwrap();
    menu_item(&mut harness, &badge("other"), "Switch to other");
    wait_for_label(&mut harness, "Your changes are in the way");
    render(&mut harness, "stash-and-switch");
    harness.get_by_label("Stash and switch").click();
    wait_for_label(&mut harness, "Stashed your changes, and switched to other.");
    wait_for_label(&mut harness, "Stashes · 1");

    // A branch made at a commit, and switched to.
    harness
        .get_by_role_and_label(Role::Button, "One")
        .click_secondary();
    wait_for_label(&mut harness, "Create a branch here…");
    harness.get_by_label("Create a branch here…").click();
    wait_for_label(&mut harness, "Switch to it");
    type_into(&mut harness, Role::TextInput, "Name", "feature");
    render(&mut harness, "create-branch");
    harness.get_by_label("Create").click();
    wait_for_label(&mut harness, "Made feature; switched to it.");
    wait_for_label(&mut harness, &on("feature"));

    // Deleting a branch whose commit is nowhere else asks again, saying so.
    menu_item(&mut harness, &badge("other"), "Delete other…");
    wait_for_label(&mut harness, "Delete other?");
    harness.get_by_label("Delete").click();
    wait_for_label(&mut harness, "Delete anyway");
    harness.get_by_label("Delete anyway").click();
    wait_for_label(&mut harness, "Deleted other.");
    assert_eq!(branch_names(&sandbox, &repo), "feature\nmain\n");
}

#[test]
fn commits_are_picked_reverted_tagged_reset_and_merged_from_the_graph() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    let row = |subject: &str| (Role::Button, subject.to_owned());
    let right_click = |harness: &mut Harness<'_, EgitApp>, (role, subject): (Role, String)| {
        harness
            .get_by_role_and_label(role, &subject)
            .click_secondary();
    };
    let subject = |name: &str| {
        let shown = sandbox
            .run(&repo, "log", &["-1", "--format=%s", name])
            .unwrap();
        String::from_utf8(shown.stdout).unwrap().trim().to_owned()
    };
    let abbreviated = |name: &str| {
        let shown = sandbox
            .run(&repo, "rev-parse", &["--short=8", name])
            .unwrap();
        String::from_utf8(shown.stdout).unwrap().trim().to_owned()
    };

    right_click(&mut harness, row("Other"));
    wait_for_label(&mut harness, "Cherry-pick onto main");
    render(&mut harness, "commit-menu");
    harness.get_by_label("Cherry-pick onto main").click();
    wait_for_label(&mut harness, &format!("Picked {}.", abbreviated("other")));
    assert_eq!(subject("HEAD"), "Other");
    // Rows move as the history is read again: a click is aimed once they are still.
    settle(&mut harness, "the picked commit shown", history_of(4));

    let two = abbreviated("HEAD~1");
    right_click(&mut harness, row("Two"));
    wait_for_label(&mut harness, "Revert in main");
    harness.get_by_label("Revert in main").click();
    wait_for_label(&mut harness, &format!("Reverted {two}."));
    assert_eq!(subject("HEAD"), "Revert \"Two\"");
    settle(&mut harness, "the revert shown", history_of(5));

    // An annotated tag.
    let one = abbreviated("main~3");
    right_click(&mut harness, row("One"));
    wait_for_label(&mut harness, "Create a tag here…");
    harness.get_by_label("Create a tag here…").click();
    wait_for_label(&mut harness, &format!("Tag {one}"));
    type_into(&mut harness, Role::TextInput, "Name", "v1");
    let message = "Message (makes it an annotated tag, with its author and date)";
    type_into(&mut harness, Role::MultilineTextInput, message, "The first");
    harness.get_by_label("Create tag").click();
    wait_for_label(&mut harness, &format!("Tagged {one} as v1."));
    wait_for_label(&mut harness, &format!("{} v1", icon::TAG));
    let kind = sandbox.run(&repo, "cat-file", &["-t", "v1"]).unwrap();
    assert_eq!(kind.stdout.trim_ascii(), b"tag");

    // Back to the first commit, the changes discarded once confirmed.
    std::fs::write(repo.join("f"), "uncommitted\n").unwrap();
    // Once the work in progress's row is in (it moves the rows below it down).
    wait_for_label(&mut harness, &uncommitted(1));
    right_click(&mut harness, row("One"));
    // egui names a submenu's button with its arrow.
    let reset = "Reset main to this commit";
    wait_for_label(&mut harness, reset);
    harness.get_by_label(reset).click();
    wait_for_label(&mut harness, "Hard: discard the changes…");
    harness.get_by_label("Hard: discard the changes…").click();
    wait_for_label(
        &mut harness,
        &format!("Reset main to {one}, discarding your changes?"),
    );
    render(&mut harness, "hard-reset");
    harness.get_by_label("Reset").click();
    wait_for_label(&mut harness, &format!("Reset to {one}."));
    assert_eq!(std::fs::read_to_string(repo.join("f")).unwrap(), "one\n");
    // `One` and `other`'s commit are all `main`, `other`, and `v1` reach now.
    settle(&mut harness, "the history after the reset", history_of(2));

    // `other` merged in: a fast-forward from here.
    menu_item(
        &mut harness,
        &format!("{} other", icon::GIT_BRANCH),
        "Merge other into main",
    );
    wait_for_label(&mut harness, "Merged other.");
    assert_eq!(subject("HEAD"), "Other");
}

#[test]
fn changes_are_stashed_popped_and_dropped() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    let stash = |count: usize| match count {
        0 => "Stash".to_owned(),
        count => format!("Stashes · {count}"),
    };

    std::fs::write(repo.join("f"), "changed\n").unwrap();
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&stash(0)).click();
    // Enabled once the work tree's status says there are changes.
    click_when_enabled(&mut harness, "Stash the changes…");
    wait_for_label(&mut harness, "Stash your changes");
    type_into(
        &mut harness,
        Role::TextInput,
        "Message (optional)",
        "Half done",
    );
    // The dialog's (the top bar's menu is named so too).
    harness.get_all_by_label("Stash").last().unwrap().click();
    wait_for_label(&mut harness, "Stashed your changes.");
    wait_for_label(&mut harness, &stash(1));
    assert_eq!(std::fs::read_to_string(repo.join("f")).unwrap(), "two\n");

    harness.get_by_label(&stash(1)).click();
    open_stash(&mut harness, "On main: Half done");
    render(&mut harness, "stash-menu");
    harness.get_by_label("Pop").click();
    wait_for_label(&mut harness, "Popped stash@{0}.");
    wait_for_label(&mut harness, &stash(0));
    assert_eq!(
        std::fs::read_to_string(repo.join("f")).unwrap(),
        "changed\n"
    );

    // Stashed again, then dropped once confirmed.
    harness.get_by_label(&stash(0)).click();
    // Enabled once the work tree's status says there are changes.
    click_when_enabled(&mut harness, "Stash the changes…");
    wait_for_label(&mut harness, "Stash your changes");
    harness.get_all_by_label("Stash").last().unwrap().click();
    wait_for_label(&mut harness, &stash(1));
    harness.get_by_label(&stash(1)).click();
    open_stash(&mut harness, "WIP on main");
    harness.get_by_label("Drop…").click();
    wait_for_label(&mut harness, "Drop stash@{0}?");
    harness.get_by_label("Drop").click();
    wait_for_label(&mut harness, "Dropped stash@{0}.");
    wait_for_label(&mut harness, &stash(0));
    let listed = sandbox.run(&repo, "stash", &["list"]).unwrap();
    assert_eq!(listed.stdout, b"");
}

#[test]
fn many_stashes_scroll_within_the_window() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    for at in 0..30 {
        std::fs::write(repo.join("f"), format!("change {at}\n")).unwrap();
        sandbox
            .run(
                &repo,
                "stash",
                &["push", "--message", &format!("Change {at}")],
            )
            .unwrap();
    }
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for_label(&mut harness, "Stashes · 30");
    harness.get_by_label("Stashes · 30").click();
    open_stash(&mut harness, "On main: Change 29");
    render(&mut harness, "stash-menu-many");
    // A screenful listed (the rest scroll), and all of it inside the window.
    let screen = harness.ctx.content_rect();
    let rows: Vec<egui::Rect> = harness
        .query_all_by_role(Role::Button)
        .filter(|node| {
            node.accesskit_node()
                .label()
                .is_some_and(|label| label.starts_with("On main: Change"))
        })
        .map(|node| node.rect())
        .collect();
    assert!((5..30).contains(&rows.len()), "{} rows", rows.len());
    assert!(
        rows.iter().all(|row| screen.contains_rect(*row)),
        "{rows:?}"
    );
    assert!(screen.contains_rect(harness.get_by_label("Drop…").rect()));
}

#[test]
fn history_is_rewritten_in_the_rebase_editor() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    for (index, subject) in ["One", "Two", "Three", "Four", "Five"].iter().enumerate() {
        let file = format!("f{}", index + 1);
        sandbox
            .commit_file(&repo, &file, &format!("{subject}\n"), subject)
            .unwrap();
    }
    let short = |name: &str| {
        let shown = sandbox
            .run(&repo, "rev-parse", &["--short=8", name])
            .unwrap();
        String::from_utf8(shown.stdout).unwrap().trim().to_owned()
    };
    let subjects = || {
        let shown = sandbox.run(&repo, "log", &["--format=%s"]).unwrap();
        String::from_utf8(shown.stdout).unwrap()
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_of(5));

    harness
        .get_by_role_and_label(Role::Button, "Two")
        .click_secondary();
    wait_for_label(&mut harness, "Rebase interactively from here…");
    harness
        .get_by_label("Rebase interactively from here…")
        .click();
    wait_for_label(&mut harness, "Rebase main interactively");
    wait_for_label(
        &mut harness,
        &format!("4 commits, onto {}", short("HEAD~4")),
    );
    render(&mut harness, "rebase-editor");

    // By keys: `Five` dropped, `Three` squashed into `Two`, `Four` moved to the bottom.
    let select = |harness: &mut Harness<'_, EgitApp>, subject: &str| {
        harness.get_by_role_and_label(Role::Button, subject).click();
        harness.run_steps(2);
    };
    // Each key lands before a row is looked for: moves move them.
    let press = |harness: &mut Harness<'_, EgitApp>, modifiers, key| {
        harness.key_press_modifiers(modifiers, key);
        harness.run_steps(2);
    };
    let none = egui::Modifiers::NONE;
    select(&mut harness, "Five");
    press(&mut harness, none, egui::Key::D);
    select(&mut harness, "Three");
    press(&mut harness, none, egui::Key::S);
    select(&mut harness, "Four");
    for _ in 0..2 {
        press(&mut harness, egui::Modifiers::ALT, egui::Key::ArrowDown);
    }
    // `Two` reworded: its new message is the squashed pair's.
    select(&mut harness, "Two");
    press(&mut harness, none, egui::Key::R);
    wait_for_label(&mut harness, "Its new message:");
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Its new message:")
        .focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(1);
    type_into(
        &mut harness,
        Role::MultilineTextInput,
        "Its new message:",
        "Two and three",
    );
    render(&mut harness, "rebase-plan");
    harness.get_by_label("Start rebase").click();
    wait_for_label(&mut harness, "Rebased: the branch is rewritten.");
    assert_eq!(subjects(), "Two and three\nFour\nOne\n");
    settle(&mut harness, "the rewritten history", history_of(3));

    // A commit dropped from its menu, once confirmed.
    let four = short("HEAD~1");
    harness
        .get_by_role_and_label(Role::Button, "Four")
        .click_secondary();
    wait_for_label(&mut harness, "Drop this commit…");
    harness.get_by_label("Drop this commit…").click();
    wait_for_label(&mut harness, &format!("Drop {four} from main?"));
    harness.get_by_label("Drop").click();
    wait_for_label(&mut harness, &format!("Dropped {four}."));
    assert_eq!(subjects(), "Two and three\nOne\n");
    settle(&mut harness, "the history without it", history_of(2));

    // The last squashed into its parent, the root: one commit, their messages joined.
    let last = short("HEAD");
    harness
        .get_by_role_and_label(Role::Button, "Two and three")
        .click_secondary();
    wait_for_label(&mut harness, "Squash into its parent");
    harness.get_by_label("Squash into its parent").click();
    wait_for_label(&mut harness, &format!("Squashed {last} into its parent."));
    let message = sandbox.run(&repo, "log", &["--format=%B"]).unwrap();
    assert_eq!(message.stdout.trim_ascii(), b"One\n\nTwo and three");
}

#[test]
fn a_file_is_blamed_and_its_history_followed() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let block = |name: &str| {
        (1..=3).fold(String::new(), |mut block, n| {
            let _ = writeln!(block, "{name} line {n}");
            block
        })
    };
    sandbox
        .commit_file(&repo, "notes", &block("alpha"), "Write")
        .unwrap();
    sandbox
        .commit_file(&repo, "notes", &format!("{}added\n", block("alpha")), "Add")
        .unwrap();
    sandbox.run(&repo, "mv", &["notes", "doc"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Rename"])
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "doc",
            &format!("{}added\nmoved on\n", block("alpha")),
            "Move on",
        )
        .unwrap();
    let short = |name: &str| {
        let shown = sandbox
            .run(&repo, "rev-parse", &["--short=8", name])
            .unwrap();
        String::from_utf8(shown.stdout).unwrap().trim().to_owned()
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_of(4));

    // The last commit's file, blamed there: each line's commit, across the rename.
    harness
        .get_by_role_and_label(Role::Button, "Move on")
        .click();
    settle(&mut harness, "the commit's files", |app| {
        let view = &mut app.state.repos[0].history_view;
        view.details_shown() && view.counts_shown()
    });
    harness.get_by_role_and_label(Role::Button, "doc").click();
    settle(&mut harness, "the file", |app| {
        app.state.repos[0].history_view.file_shown() == Some(("doc".to_owned(), true))
    });
    harness.get_by_label("Blame").click();
    wait_for_label(&mut harness, &format!("at {}", short("HEAD")));
    settle(&mut harness, "the blame", |app| {
        app.state.repos[0].history_view.blame_done() == Some(true)
    });
    harness.run_steps(2);
    render(&mut harness, "blame");
    // Following moved lines blames it again.
    harness.get_by_label("Follow moved lines").click();
    harness.run_steps(2);
    settle(&mut harness, "the blame again", |app| {
        app.state.repos[0].history_view.blame_done() == Some(true)
    });

    // The line `Add` added: its commit, and the way to it.
    drag(&mut harness, (3, 0), (3, 3));
    wait_for_label(&mut harness, "Blame before this commit");
    render(&mut harness, "blame-line");
    harness.get_by_label("Show the commit").click();
    wait_for_label(&mut harness, &short("HEAD~2"));

    // Its history: four commits, the rename's diff.
    harness
        .get_by_role_and_label(Role::Button, "Move on")
        .click();
    settle(&mut harness, "the commit's files", |app| {
        let view = &mut app.state.repos[0].history_view;
        view.details_shown() && view.counts_shown()
    });
    harness.get_by_role_and_label(Role::Button, "doc").click();
    wait_for_label(&mut harness, "File history");
    harness.get_by_label("File history").click();
    // Its commits as they are found (the history's bar says "4 commits" too).
    wait_for_label(&mut harness, "History of");
    for _ in 0..2_000 {
        harness.step();
        if harness
            .query_by_role_and_label(Role::Button, "Rename")
            .is_some()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    harness
        .get_by_role_and_label(Role::Button, "Rename")
        .click();
    wait_for_label(&mut harness, "egit tests · from notes");
    wait_for_label(&mut harness, "Renamed from notes; its text unchanged.");
    // ↓ to the commit before, which changed the file's text.
    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness, "the earlier commit's diff", |app| {
        app.state.repos[0].history_view.file_history_diff() == Some((1, 0))
    });
    harness.run_steps(4);
    render(&mut harness, "file-history");
}

#[test]
fn whole_file_shows_all_of_the_file_and_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let lines = |changed: &str| -> String {
        (1..=30)
            .map(|line| {
                if line == 15 {
                    format!("{changed}\n")
                } else {
                    format!("line {line}\n")
                }
            })
            .collect()
    };
    sandbox
        .commit_file(&repo, "notes.txt", &lines("before"), "Write notes")
        .unwrap();
    sandbox
        .commit_file(&repo, "notes.txt", &lines("after"), "Change a note")
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness
        .get_by_role_and_label(Role::Button, "Change a note")
        .click();
    settle(&mut harness, "the commit's details", |app| {
        app.state.repos[0].history_view.details_shown()
    });
    harness
        .get_by_role_and_label(Role::Button, "notes.txt")
        .click();
    // A hunk header, three lines either side, and the line removed and added.
    let shown = |lines| {
        move |app: &mut EgitApp| app.state.repos[0].history_view.diff_lines() == Some(lines)
    };
    settle(&mut harness, "the diff", shown(9));
    harness.get_by_label("Whole file").click();
    settle(&mut harness, "the whole file", shown(32));
    harness.get_by_label("Whole file").click();
    settle(&mut harness, "the diff again", shown(9));

    // Side by side, the new side has the hunk header and its own lines.
    let split = |lines| {
        move |app: &mut EgitApp| app.state.repos[0].history_view.split_lines() == Some(lines)
    };
    harness.get_by_label("Side by side").click();
    settle(&mut harness, "the sides", split(8));
    harness.get_by_label("Whole file").click();
    settle(&mut harness, "the whole file's sides", split(31));
    harness.get_by_label("Whole file").click();
    settle(&mut harness, "the sides again", split(8));
}

#[test]
fn a_changed_file_opens_in_place_of_the_history() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = parser_change(&sandbox);
    harness.get_by_role_and_label(Role::Button, &format!("{}  History", icon::ARROW_LEFT));
    // The diff's lines, beside the commit's and the file's in the details.
    assert_eq!(harness.query_all_by_label("+3").count(), 3);
    assert_eq!(harness.query_all_by_label("−1").count(), 3);
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Greet the world")
            .is_none(),
        "the diff takes the commit table's place"
    );
    render(&mut harness, "history-diff");

    harness.get_by_label("Side by side").click();
    settle(&mut harness, "the sides", parser_shown);
    render(&mut harness, "history-diff-split");
    harness.get_by_label("Unified").click();

    harness.get_by_label("Whole file").click();
    settle(&mut harness, "the whole file's diff", parser_shown);
    harness.get_by_role_and_label(Role::Button, "File").click();
    settle(&mut harness, "the file", parser_shown);
    harness.get_by_label("Wrap lines");

    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
    assert_eq!(
        harness.state_mut().state.repos[0].history_view.file_shown(),
        None
    );
    harness.get_by_role_and_label(Role::Button, "Greet the world");
}

#[test]
fn text_on_the_grid_is_selected_copied_and_found() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = parser_change(&sandbox);
    harness.get_by_role_and_label(Role::Button, "File").click();
    settle(&mut harness, "the file", parser_shown);

    // Dragging across the start of the first line selects it, and ⌘C copies it.
    let spot = |harness: &mut Harness<'_, EgitApp>, cell| {
        harness.state_mut().state.repos[0]
            .history_view
            .file_spot(0, cell)
            .unwrap()
    };
    let (from, to) = (spot(&mut harness, 0), spot(&mut harness, 9));
    harness.hover_at(from);
    harness.drag_at(from);
    harness.run_steps(1);
    harness.hover_at(egui::pos2(f32::midpoint(from.x, to.x), from.y));
    harness.hover_at(to);
    harness.drop_at(to);
    harness.run_steps(2);
    render(&mut harness, "history-file");
    harness.event(egui::Event::Copy);
    harness.run_steps(1);
    let copied = harness
        .output()
        .platform_output
        .commands
        .iter()
        .any(|command| *command == egui::OutputCommand::CopyText("fn parse(".to_owned()));
    assert!(copied, "{:?}", harness.output().platform_output.commands);
    // Escape clears the selection before it closes the file.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(
        harness.state_mut().state.repos[0]
            .history_view
            .file_shown()
            .is_some()
    );

    // ⌘F finds in the file; Enter moves to a match; Escape closes the bar first.
    harness.hover_at(from);
    harness.drag_at(from);
    harness.drop_at(from);
    harness.run_steps(1);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    find_field(&harness).type_text("TRUE");
    wait_for_label(&mut harness, "No matches");
    find_field(&harness).focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    find_field(&harness).type_text("true");
    wait_for_label(&mut harness, "1 found");
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
    harness.get_by_label("1 of 1");
    render(&mut harness, "history-file-find");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label("1 of 1").is_none());
    assert!(
        harness.state_mut().state.repos[0]
            .history_view
            .file_shown()
            .is_some()
    );
}

#[test]
fn a_long_history_without_a_commit_graph_is_offered_one() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("long").unwrap();
    let mut stream = String::new();
    for index in 1..=10_000 {
        let message = format!("Commit number {index}\n");
        let _ = write!(
            stream,
            "commit refs/heads/main\nmark :{index}\ncommitter Test <t@example.com> {} +0000\ndata {}\n{message}",
            1_700_000_000 + index,
            message.len()
        );
        if index > 1 {
            let _ = writeln!(stream, "from :{}", index - 1);
        }
        stream.push('\n');
    }
    sandbox
        .git()
        .write("fast-import")
        .arg("--quiet")
        .stdin(stream.into_bytes())
        .current_dir(&repo)
        .output()
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.get_by_label("10,000 commits");
    harness.get_by_role_and_label(Role::Button, "Commit number 10000");
    render(&mut harness, "history-commit-graph-offer");

    harness
        .get_by_role_and_label(Role::Button, "Write a commit-graph")
        .click();
    settle(&mut harness, "the commit-graph", |app| {
        app.state.repos[0].history.shown.commit_graph() == Some(true)
    });
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Write a commit-graph")
            .is_none()
    );
    assert!(harness.state().state.repos[0].repository.has_commit_graph());
}

#[test]
fn the_profile_follows_edits_to_the_users_configuration() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "user.name", "Edited Elsewhere"],
        )
        .unwrap();
    settle(&mut harness, "the edit", |app| {
        app.state
            .user_config
            .ok_ref()
            .and_then(|snapshot| {
                snapshot
                    .profile()
                    .name()
                    .map(|name| name.text() == "Edited Elsewhere")
            })
            .unwrap_or(false)
    });
    harness.get_by_label("Edited Elsewhere <tests@egit.invalid>");
}

/// The subject of the commit the first repository's branch is at.
fn shown_subject(app: &mut EgitApp) -> Option<String> {
    match &app.state.repos.first_mut()?.status.ok_ref()?.head {
        egit_git::Head::Branch(branch) => branch
            .commit
            .as_ref()
            .map(|commit| commit.subject.to_string()),
        egit_git::Head::Detached(commit) => Some(commit.subject.to_string()),
    }
}

/// The branch the first repository is on.
fn shown_branch(app: &mut EgitApp) -> Option<String> {
    match &app.state.repos.first_mut()?.status.ok_ref()?.head {
        egit_git::Head::Branch(branch) => Some(branch.name.to_string()),
        egit_git::Head::Detached(_) => None,
    }
}

#[test]
fn the_tabs_open_at_the_last_close_come_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("kept").unwrap();
    let settings = Settings {
        open: vec![
            sandbox.path("deleted since").as_path().into(),
            repo.as_path().into(),
        ],
        ..Settings::default()
    };
    let mut harness = harness_with_settings(
        sandbox.git().clone(),
        sandbox.home(),
        egui::Theme::Dark,
        settings,
    );
    settle(&mut harness, "the repository", repository_shown);
    let app = harness.state();
    assert_eq!(app.state.repos.len(), 1);
    assert_eq!(app.state.repos[0].repository.work_tree(), repo);
    assert_eq!(app.state.home.problem, None);
}

/// A commit's name in `repo`.
fn commit_of(sandbox: &Sandbox, repo: &Path, name: &str) -> String {
    let shown = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    String::from_utf8(shown.stdout).unwrap().trim().to_owned()
}

/// A clone whose `origin` is `path` on `fake` (fetched through a bare repository, by
/// `insteadOf`), signed in with `token` through a `store` helper: `main` (Start), `topic` (Add
/// topic), and a fork's pull request kept as `refs/pull/7/head`. Returns the clone, `topic`'s
/// commit, and the fork's.
fn forge_clone(
    sandbox: &Sandbox,
    fake: &FakeForge,
    path: &str,
    token: &str,
) -> (PathBuf, String, String) {
    let bare = sandbox.path("origin.git");
    let bare_path = bare.to_str().unwrap().to_owned();
    sandbox
        .run(sandbox.home(), "init", &["--quiet", "--bare", &bare_path])
        .unwrap();
    let seed = sandbox.init("seed").unwrap();
    sandbox.commit_file(&seed, "a", "a\n", "Start").unwrap();
    let push = |what: &str| {
        sandbox
            .run(&seed, "push", &["--quiet", &bare_path, what])
            .unwrap();
    };
    push("HEAD:refs/heads/main");
    sandbox
        .run(&seed, "switch", &["--quiet", "--create", "topic"])
        .unwrap();
    sandbox.commit_file(&seed, "t", "t\n", "Add topic").unwrap();
    let topic = commit_of(sandbox, &seed, "HEAD");
    push("HEAD:refs/heads/topic");
    sandbox.run(&seed, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&seed, "f", "f\n", "Fork change")
        .unwrap();
    let fork = commit_of(sandbox, &seed, "HEAD");
    // Where the forge keeps a request's head.
    push(match fake.kind() {
        ForgeKind::GitHub => "HEAD:refs/pull/7/head",
        ForgeKind::GitLab => "HEAD:refs/merge-requests/7/head",
    });

    let work = sandbox.path("work");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &[
                "--quiet",
                &format!("file://{bare_path}"),
                work.to_str().unwrap(),
            ],
        )
        .unwrap();
    let url = format!("http://{}/{path}.git", fake.address());
    sandbox
        .run(&work, "remote", &["set-url", "origin", &url])
        .unwrap();
    let rewrite = format!("url.file://{bare_path}.insteadOf");
    sandbox.run(&work, "config", &[&rewrite, &url]).unwrap();
    sandbox
        .run(
            &work,
            "config",
            &[
                match fake.kind() {
                    ForgeKind::GitHub => "egit.githubHost",
                    ForgeKind::GitLab => "egit.gitlabHost",
                },
                &fake.host_entry(),
            ],
        )
        .unwrap();
    let store = sandbox.path("credentials");
    std::fs::write(&store, format!("http://ada:{token}@{}\n", fake.address())).unwrap();
    // Through git's shell, which takes `/` between folders everywhere (and a backslash as an
    // escape).
    let helper = format!(
        "store --file={}",
        store.display().to_string().replace('\\', "/")
    );
    sandbox
        .run(&work, "config", &["credential.helper", &helper])
        .unwrap();
    serve_requests(fake, path, &topic, &fork);
    (work, topic, fork)
}

/// `fake`'s open requests on `path`: `!8`/`#8` (Add topic, from `topic`, approved, its build
/// passed) and `7` (Fork change, from a fork's `feature` at `fork`, its build failed).
fn serve_requests(fake: &FakeForge, path: &str, topic: &str, fork: &str) {
    // Each forge's own words for a passed build and a failed one.
    let (passed_run, failed) = match fake.kind() {
        ForgeKind::GitHub => (("COMPLETED", Some("SUCCESS".to_owned())), "FAILURE"),
        ForgeKind::GitLab => (("SUCCESS", None), "FAILED"),
    };
    fake.state().repositories.push(FakeRepository {
        path: path.to_owned(),
        id: 41,
        default_branch: "main".to_owned(),
        parent: None,
        pulls: vec![
            FakePull {
                number: 8,
                title: "Add topic".to_owned(),
                author: "bo".to_owned(),
                head_branch: "topic".to_owned(),
                base_branch: "main".to_owned(),
                head_commit: topic.to_owned(),
                review: Some("APPROVED".to_owned()),
                checks: Some("SUCCESS".to_owned()),
                check_runs: vec![("build".to_owned(), passed_run.0.to_owned(), passed_run.1)],
                body: "Adds the topic.".to_owned(),
                ..FakePull::default()
            },
            FakePull {
                number: 7,
                title: "Fork change".to_owned(),
                author: "cy".to_owned(),
                head_branch: "feature".to_owned(),
                fork: Some("cy/tool".to_owned()),
                base_branch: "main".to_owned(),
                head_commit: fork.to_owned(),
                checks: Some(failed.to_owned()),
                ..FakePull::default()
            },
        ],
    });
}

/// Whether the first repository's pull requests are read, `count` of them.
fn pull_requests_read(count: usize) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        app.state
            .repos
            .first()
            .is_some_and(|repo| !repo.forge.is_reading() && repo.forge.found().pulls.len() == count)
    }
}

#[test]
fn pull_requests_are_listed_badged_checked_out_and_opened() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitHub, "t0ken", "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "octo/tool", "t0ken");
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the pull requests", pull_requests_read(2));
    // The same repository's pull request is on its head commit; the fork's head isn't here.
    wait_for_label(&mut harness, &format!("{} #8", icon::GIT_PULL_REQUEST));
    assert!(
        harness
            .query_by_label(&format!("{} #7", icon::GIT_PULL_REQUEST))
            .is_none()
    );
    render(&mut harness, "pull-request-badges");

    harness.get_by_label_contains("2 pull requests").click();
    wait_for_label(&mut harness, "Adds the topic.");
    harness.get_by_label("build");
    harness.get_by_label("bo wants to merge topic into main");
    render(&mut harness, "pull-requests");

    // Checked out: the same repository's through its branch, tracking the remote's.
    click_when_enabled(&mut harness, "Check out");
    wait_for_label(&mut harness, "Checked out pull request #8 as topic.");
    let branch = sandbox.run(&work, "branch", &["--show-current"]).unwrap();
    assert_eq!(branch.stdout.trim_ascii(), b"topic");
    let upstream = sandbox
        .run(
            &work,
            "for-each-ref",
            &["--format=%(upstream:short)", "refs/heads/topic"],
        )
        .unwrap();
    assert_eq!(upstream.stdout.trim_ascii(), b"origin/topic");

    // The fork's: fetched from its pull request's head into pr/7.
    harness
        .get_by_role_and_label(Role::Button, "Fork change")
        .click();
    wait_for_label(
        &mut harness,
        "cy wants to merge feature into main, from cy/tool",
    );
    assert!(
        harness
            .get_by_label("Show the commit")
            .accesskit_node()
            .is_disabled()
    );
    click_when_enabled(&mut harness, "Check out");
    wait_for_label(&mut harness, "Checked out pull request #7 as pr/7.");
    let branch = sandbox.run(&work, "branch", &["--show-current"]).unwrap();
    assert_eq!(branch.stdout.trim_ascii(), b"pr/7");

    harness.get_by_label_contains("Open on GitHub").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        [format!("http://{}/octo/tool/pull/7", fake.address())]
    );
}

#[test]
fn a_pull_request_is_opened_from_a_branch_s_menu() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitHub, "t0ken", "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "octo/tool", "t0ken");
    sandbox
        .run(&work, "switch", &["--quiet", "--create", "fresh"])
        .unwrap();
    sandbox
        .run(
            &work,
            "commit",
            &[
                "--quiet",
                "--allow-empty",
                "--message",
                "Make it fresh\n\nBecause it was stale.",
            ],
        )
        .unwrap();
    sandbox
        .run(
            &work,
            "push",
            &["--quiet", "--set-upstream", "origin", "fresh"],
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(3));
    settle(&mut harness, "the pull requests", pull_requests_read(2));

    let fresh = format!("{} fresh", icon::GIT_BRANCH);
    menu_item(&mut harness, &fresh, "Create pull request…");
    wait_for_label(&mut harness, "Create a pull request");
    settle(&mut harness, "the suggestions", |app| {
        app.state.repos[0].history_view.pull_form_title() == Some("Make it fresh".to_owned())
    });
    render(&mut harness, "pull-request-create");
    click_when_enabled(&mut harness, "Create pull request");
    wait_for_label(&mut harness, "Opened pull request #9.");
    // The pull requests are read again at once: find the request that opened it.
    let sent = fake
        .state()
        .requests
        .iter()
        .find(|request| request.path.ends_with("/pulls"))
        .unwrap()
        .body
        .clone();
    assert_eq!(sent["head"], "fresh");
    assert_eq!(sent["base"], "main");
    assert_eq!(sent["title"], "Make it fresh");
    assert_eq!(sent["body"], "Because it was stale.");
    assert_eq!(sent["draft"], false);

    // Read again: the branch's menu shows it now.
    settle(
        &mut harness,
        "the pull requests again",
        pull_requests_read(3),
    );
    menu_item(&mut harness, &fresh, "Show pull request #9");
    wait_for_label(&mut harness, "ada wants to merge fresh into main");
}

#[test]
fn pull_requests_without_credentials_offer_to_sign_in() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitHub, "t0ken", "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "octo/tool", "wrong");
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    // The remotes worked out and read: refused.
    settle(&mut harness, "the refusal", |app| {
        app.state.repos.first().is_some_and(|repo| {
            !repo.forge.is_reading()
                && matches!(
                    repo.forge.found().problem,
                    Some(ForgeProblem::SignIn { refused: true, .. })
                )
        })
    });
    harness.get_by_label("Pull requests").click();
    let host = fake.address();
    wait_for_label(
        &mut harness,
        &format!("{}  Sign in to {host}", icon::SIGN_IN),
    );
    harness.get_by_label(&format!(
        "{host} refused the credentials git gave it. Sign in with a token that can read this \
         repository."
    ));
    // The refused credential is forgotten.
    let kept = std::fs::read_to_string(sandbox.path("credentials")).unwrap();
    assert!(!kept.contains("wrong"), "{kept}");
    render(&mut harness, "pull-requests-sign-in");
    harness.get_by_label("Create a token…").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        [format!(
            "http://{host}/settings/tokens/new?scopes=repo&description=egit"
        )]
    );
    // Nobody can answer git's question here: signing in says so.
    harness.get_by_label("Sign in").click();
    wait_for_label(&mut harness, "No credentials were given.");
}

#[test]
fn merge_requests_on_gitlab_are_listed_badged_checked_out_and_opened() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, "glpat-t0ken", "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", "glpat-t0ken");
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    wait_for_label(&mut harness, &format!("{} !8", icon::GIT_PULL_REQUEST));

    harness.get_by_label_contains("2 merge requests").click();
    wait_for_label(&mut harness, "Adds the topic.");
    harness.get_by_label("build");
    harness.get_by_label("Merge requests");
    render(&mut harness, "merge-requests");

    click_when_enabled(&mut harness, "Check out");
    wait_for_label(&mut harness, "Checked out merge request !8 as topic.");
    let branch = sandbox.run(&work, "branch", &["--show-current"]).unwrap();
    assert_eq!(branch.stdout.trim_ascii(), b"topic");

    // A fork's: fetched from GitLab's ref for it into mr/7.
    harness
        .get_by_role_and_label(Role::Button, "Fork change")
        .click();
    wait_for_label(
        &mut harness,
        "cy wants to merge feature into main, from cy/tool",
    );
    click_when_enabled(&mut harness, "Check out");
    wait_for_label(&mut harness, "Checked out merge request !7 as mr/7.");
    let branch = sandbox.run(&work, "branch", &["--show-current"]).unwrap();
    assert_eq!(branch.stdout.trim_ascii(), b"mr/7");

    harness.get_by_label_contains("Open on GitLab").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        [format!(
            "http://{}/team/sub/tool/-/merge_requests/7",
            fake.address()
        )]
    );
}

#[test]
fn a_merge_request_is_opened_from_a_branch_s_menu_on_gitlab() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, "glpat-t0ken", "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", "glpat-t0ken");
    sandbox
        .run(&work, "switch", &["--quiet", "--create", "fresh"])
        .unwrap();
    sandbox
        .run(
            &work,
            "commit",
            &[
                "--quiet",
                "--allow-empty",
                "--message",
                "Make it fresh\n\nBecause it was stale.",
            ],
        )
        .unwrap();
    sandbox
        .run(
            &work,
            "push",
            &["--quiet", "--set-upstream", "origin", "fresh"],
        )
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(3));
    settle(&mut harness, "the merge requests", pull_requests_read(2));

    let fresh = format!("{} fresh", icon::GIT_BRANCH);
    menu_item(&mut harness, &fresh, "Create merge request…");
    wait_for_label(&mut harness, "Create a merge request");
    settle(&mut harness, "the suggestions", |app| {
        app.state.repos[0].history_view.pull_form_title() == Some("Make it fresh".to_owned())
    });
    harness.get_by_label("Draft").click();
    click_when_enabled(&mut harness, "Create merge request");
    wait_for_label(&mut harness, "Opened merge request !9.");
    let sent = fake
        .state()
        .requests
        .iter()
        .find(|request| request.path.ends_with("/merge_requests"))
        .unwrap()
        .clone();
    assert_eq!(
        sent.path,
        "/api/v4/projects/team%2Fsub%2Ftool/merge_requests"
    );
    assert_eq!(sent.body["source_branch"], "fresh");
    assert_eq!(sent.body["target_branch"], "main");
    assert_eq!(sent.body["title"], "Draft: Make it fresh");
    assert_eq!(sent.body["description"], "Because it was stale.");

    settle(
        &mut harness,
        "the merge requests again",
        pull_requests_read(3),
    );
    menu_item(&mut harness, &fresh, "Show merge request !9");
    wait_for_label(&mut harness, "ada wants to merge fresh into main");
}

/// Double-clicks the label `label` in the details panel (the rightmost one so named: the
/// commit table's row has the same name).
fn double_click_in_details(harness: &mut Harness<'_, EgitApp>, label: &str) {
    let at = harness
        .query_all_by_role_and_label(Role::Label, label)
        .map(|node| node.rect().center())
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .unwrap();
    harness.hover_at(at);
    harness.step();
    for pressed in [true, false, true, false] {
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
}

#[test]
fn a_commits_message_is_edited_in_place() {
    const LONG: &str = "Make the parser answer the question it was asked, with a subject long \
                        enough to wrap";
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "1\n", "Start").unwrap();
    sandbox
        .commit_file(
            &repo,
            "f",
            "2\n",
            &format!("{LONG}\n\nIts description.\nA second line."),
        )
        .unwrap();
    sandbox.commit_file(&repo, "g", "3\n", "Add more").unwrap();
    // A change staged, which rewording the last commit leaves staged.
    std::fs::write(repo.join("f"), "staged\n").unwrap();
    sandbox.run(&repo, "add", &["f"]).unwrap();
    let message = |sandbox: &Sandbox, at: &str| {
        let out = sandbox
            .run(&repo, "log", &["-1", "--format=%B", at])
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    };
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // The last commit's, from the pencil: only its message changes.
    harness
        .get_by_role_and_label(Role::Button, "Add more")
        .click();
    settle(&mut harness, "the commit's details", |app| {
        app.state.repos[0].history_view.details_shown()
    });
    click_when_enabled(&mut harness, "Edit the message");
    wait_for_label(&mut harness, "Save message");
    type_into(&mut harness, Role::TextInput, "Summary", " things");
    type_into(
        &mut harness,
        Role::MultilineTextInput,
        "Description",
        "Why it matters.",
    );
    render(&mut harness, "message-edit");
    let short = |sandbox: &Sandbox, at: &str| {
        let out = sandbox.run(&repo, "rev-parse", &["--short=8", at]).unwrap();
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    };
    let head = short(&sandbox, "HEAD");
    harness.get_by_label("Save message").click();
    wait_for_label(&mut harness, &format!("Reworded {head}."));
    assert_eq!(
        message(&sandbox, "HEAD"),
        "Add more things\n\nWhy it matters."
    );
    // Still selected: the commit it became.
    wait_for_label(&mut harness, "Add more things");
    let staged = sandbox
        .run(&repo, "diff", &["--cached", "--name-only"])
        .unwrap()
        .stdout;
    assert_eq!(staged, b"f\n");

    // An earlier commit's, by a double click on its subject: a rebase rewording it.
    sandbox.run(&repo, "reset", &["--hard", "--quiet"]).unwrap();
    wait_for_label(&mut harness, LONG);
    harness.run_steps(4);
    harness.get_by_role_and_label(Role::Button, LONG).click();
    settle(&mut harness, "the commit's details", |app| {
        app.state.repos[0].history_view.details_shown()
    });
    render(&mut harness, "history-long-message");
    double_click_in_details(&mut harness, LONG);
    wait_for_label(&mut harness, "Save message");
    // The description, a line longer.
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Description")
        .focus();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Description")
        .type_text("Its description.\nA second line.\nA third line.");
    harness.run_steps(2);
    let earlier = short(&sandbox, "HEAD~1");
    harness.get_by_label("Save message").click();
    wait_for_label(&mut harness, &format!("Reworded {earlier}."));
    assert_eq!(
        message(&sandbox, "HEAD~1"),
        format!("{LONG}\n\nIts description.\nA second line.\nA third line.")
    );
    assert_eq!(
        message(&sandbox, "HEAD"),
        "Add more things\n\nWhy it matters."
    );
}
