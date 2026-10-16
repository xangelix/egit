//! The screens, driven as a user drives them (`egui_kittest`, through the accessibility tree),
//! against a sandboxed git. Where a wgpu adapter exists (a software one will do: lavapipe) each
//! screen is also rendered to `target/gui-screens/<name>.png` for review; `EGIT_GUI_RENDER=1`
//! requires rendering and `0` skips it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use egit_git::testing::Sandbox;
use egit_git::{ConfigPlace, ConfigSnapshot, Git};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use crate::app::EgitApp;
use crate::settings::Settings;

/// Where renderings are written.
fn screens() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gui-screens");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Whether screens are rendered (see the module's documentation).
fn rendering() -> bool {
    static RENDERING: OnceLock<bool> = OnceLock::new();
    *RENDERING.get_or_init(|| match std::env::var("EGIT_GUI_RENDER").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new).is_ok(),
    })
}

/// The whole application around `git`, at a typical window size.
fn harness_with(git: Git, home: &Path, theme: egui::Theme) -> Harness<'static, EgitApp> {
    let home = home.to_owned();
    let builder = Harness::builder()
        .with_size([1180.0, 800.0])
        .with_theme(theme);
    let builder = if rendering() { builder.wgpu() } else { builder };
    builder.build_eframe(move |creation| {
        EgitApp::with_git(&creation.egui_ctx, git, Settings::default(), Some(home))
    })
}

fn harness(sandbox: &Sandbox) -> Harness<'static, EgitApp> {
    harness_with(sandbox.git().clone(), sandbox.home(), egui::Theme::Dark)
}

/// Renders the window into `name.png` (where screens are rendered).
fn render(harness: &mut Harness<'_, EgitApp>, name: &str) {
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
