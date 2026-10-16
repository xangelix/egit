//! The screens, driven as a user drives them (`egui_kittest`, through the accessibility tree),
//! against a sandboxed git. Where a wgpu adapter exists (a software one will do: lavapipe) each
//! screen is also rendered to `target/gui-screens/<name>.png` for review; `EGIT_GUI_RENDER=1`
//! requires rendering and `0` skips it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use egit_git::askpass::{PromptKind, ask_as_helper};
use egit_git::testing::Sandbox;
use egit_git::{ConfigPlace, ConfigSnapshot, Git};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use crate::app::EgitApp;
use crate::settings::Settings;
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
fn harness_with(git: Git, home: &Path, theme: egui::Theme) -> Harness<'static, EgitApp> {
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

fn harness(sandbox: &Sandbox) -> Harness<'static, EgitApp> {
    harness_with(sandbox.git().clone(), sandbox.home(), egui::Theme::Dark)
}

/// Checks the screen (`audit`) and renders it into `name.png` (where screens are rendered).
fn render(harness: &mut Harness<'_, EgitApp>, name: &str) {
    super::audit::assert_clean(harness, name);
    if !rendering() {
        return;
    }
    let image = harness.render().unwrap();
    image.save(screens().join(format!("{name}.png"))).unwrap();
}

/// Steps the application until `done` holds (jobs finish on their own threads).
fn settle(harness: &mut Harness<'_, EgitApp>, what: &str, done: impl Fn(&mut EgitApp) -> bool) {
    for _ in 0..2_000 {
        harness.step();
        if done(harness.state_mut()) {
            // Let the frame that saw the result lay itself out.
            harness.run_steps(4);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

fn profile_loaded(app: &mut EgitApp) -> bool {
    app.state.user_config.is_ok()
}

fn repository_shown(app: &mut EgitApp) -> bool {
    app.state
        .repos
        .first_mut()
        .is_some_and(|repo| repo.status.is_ok() && repo.config.is_ok())
}

/// Whether the first repository's history is read, with the summaries of the rows on screen.
fn history_shown(app: &mut EgitApp) -> bool {
    app.state.repos.first_mut().is_some_and(|repo| {
        !repo.history.is_loading()
            && !repo.history.summaries.is_reading()
            && (repo.history.shown.rows() == 0 || repo.history.summaries.len() > 0)
    })
}

/// Clicks a repository's panel tab.
fn show_panel(harness: &mut Harness<'_, EgitApp>, panel: &str) {
    harness.get_by_label_contains(panel).click();
    harness.run_steps(4);
}

/// Types `path` into Home's path field and opens it.
fn open_path(harness: &mut Harness<'_, EgitApp>, path: &Path) {
    let field = harness.get_by_role(Role::TextInput);
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
    std::fs::set_permissions(&base, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
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
fn branchy(sandbox: &Sandbox) -> PathBuf {
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
    harness.get_by_role_and_label(Role::Label, "Added");
    // The commit's lines, and the file's.
    assert_eq!(harness.query_all_by_label("+1").count(), 2);
    assert_eq!(harness.query_all_by_label("−0").count(), 2);
    harness.get_by_label("egit tests <tests@egit.invalid>");
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
fn uncommitted(files: usize) -> String {
    let files = if files == 1 {
        "1 file".to_owned()
    } else {
        format!("{files} files")
    };
    format!("{}  {files} with uncommitted changes", icon::PENCIL_SIMPLE)
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
        move |harness: &mut Harness<'_, EgitApp>| {
            for _ in 0..2_000 {
                harness.step();
                if harness.query_by_label(&label).is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("timed out waiting for {label:?}");
        }
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
    harness.get_by_label("Side by side").click();

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
    harness.run_steps(4);
    harness.get_by_label("No matches");
    find_field(&harness).focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    find_field(&harness).type_text("true");
    harness.run_steps(4);
    harness.get_by_label("1 found");
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
