//! Signing in to GitHub or GitLab, as someone who never has: a repository on a fake GitLab with
//! no credentials kept offers it in a banner, the top bar, and the sidebar; the dialog opens the
//! token's page (or copies its link), refuses a mistyped token (the helper forgetting nothing), takes the right one
//! (kept by the helper under its login), and the merge requests are read with it at once;
//! Profile says who is signed in, and signs out (the helper forgets it, and the repository
//! offers to sign in again). Not now hides the banner, the top bar's button staying. Where no
//! helper keeps it, the sign-in holds for a repository opened after it too, and the helpers
//! installed are offered: one chosen (from Profile's credentials) is set up and keeps it.

use egit_forge::ForgeKind;
use egit_forge::testing::FakeForge;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    forge_clone, harness, history_of, into_view, open_path, profile_loaded, pull_requests_read,
    render, settle, type_into, wait_for_label,
};
use crate::app::{Action, EgitApp};
use crate::settings::Location;
use crate::theme::icon;

const TOKEN: &str = "glpat-t0ken";

/// Whether a node named `label` is in the window.
fn shows(harness: &egui_kittest::Harness<'_, EgitApp>, label: &str) -> bool {
    harness.query_by_label(label).is_some()
}

/// The lowest of the buttons named `label` (the dialog's, over the page's).
fn click_lowest(harness: &egui_kittest::Harness<'_, EgitApp>, label: &str) {
    harness
        .query_all_by_label(label)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
}

/// Waits until the repository's requests want no sign-in (or want one again, `wanted`).
fn wanting_sign_in(wanted: bool) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        app.state.repos.first().is_some_and(|repo| {
            !repo.forge.is_reading() && repo.forge.needs_sign_in().is_some() == wanted
        })
    }
}

/// Copy the link copies `link`, and says so.
fn copies_the_link(harness: &mut egui_kittest::Harness<'_, EgitApp>, link: &str) {
    let before = harness.get_by_label("Copy the link").rect();
    harness.get_by_label("Copy the link").click();
    harness.run_steps(1);
    assert!(
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .any(|command| *command == egui::OutputCommand::CopyText(link.to_owned()))
    );
    harness.run_steps(1);
    // Saying "Copied", it keeps its place: nothing beside it moves.
    assert_eq!(harness.get_by_label("Copy the link").rect(), before);
}

#[test]
fn signing_in_is_offered_wherever_it_counts_and_works_with_a_token() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    // Nothing kept: nobody ever signed in. The user's own settings list the host and keep
    // credentials where the repository does.
    let store = sandbox.path("credentials");
    std::fs::write(&store, "").unwrap();
    let helper = format!(
        "store --file={}",
        store.display().to_string().replace('\\', "/")
    );
    for (key, value) in [
        ("credential.helper", helper.as_str()),
        ("egit.gitlabHost", &fake.host_entry()),
    ] {
        sandbox
            .run(sandbox.home(), "config", &["--global", key, value])
            .unwrap();
    }
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "nobody signed in", wanting_sign_in(true));

    // Offered in the banner, the top bar, and the sidebar.
    wait_for_label(
        &mut harness,
        &format!(
            "{}  Sign in to GitLab to see this repository's merge requests and CI/CD pipelines.",
            icon::SIGN_IN
        ),
    );
    assert!(shows(&harness, "Sign in to GitLab"));
    assert!(shows(&harness, "Sign in to see merge requests…"));
    render(&mut harness, "sign-in-offered");

    // The dialog: the token's page, then a mistyped token refused.
    harness.get_by_label("Sign in…").click();
    wait_for_label(&mut harness, "Paste it here");
    harness
        .get_by_label(&format!("{}  Create a token…", icon::ARROW_SQUARE_OUT))
        .click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().state.opener.recorded(),
        [format!(
            "http://{}/-/user_settings/personal_access_tokens?name=egit&scopes=api",
            fake.address()
        )]
    );
    // Or its link copied, for whichever browser one is signed in with.
    copies_the_link(
        &mut harness,
        &format!(
            "http://{}/-/user_settings/personal_access_tokens?name=egit&scopes=api",
            fake.address()
        ),
    );
    render(&mut harness, "sign-in");
    type_into(&mut harness, Role::PasswordInput, "Token", "glpat-mistyped");
    click_lowest(&harness, "Sign in");
    wait_for_label(
        &mut harness,
        "Check it is whole (copied as GitLab showed it) and hasn't expired, and that it has \
         the scope egit asks for when it opens the token's page.",
    );
    assert_eq!(std::fs::read_to_string(&store).unwrap(), "");

    // The right one: signed in as the host says, kept by the helper, the requests read.
    type_into(&mut harness, Role::PasswordInput, "Token", TOKEN);
    click_lowest(&harness, "Sign in");
    wait_for_label(
        &mut harness,
        &format!(
            "{}  Signed in to {} as ada.",
            icon::CHECK_CIRCLE,
            fake.address()
        ),
    );
    assert!(
        std::fs::read_to_string(&store)
            .unwrap()
            .contains(&format!("ada:{TOKEN}@"))
    );
    harness.get_by_label("Done").click();
    settle(&mut harness, "the merge requests", pull_requests_read(2));
    wait_for_label(&mut harness, "2 merge requests");
    assert!(!shows(&harness, "Sign in to GitLab"));

    // Profile says who, and signs out: the helper forgets it, and the repository offers again.
    harness
        .state_mut()
        .state
        .actions
        .push(crate::app::Action::ShowProfile);
    let who = format!("{}  Signed in as ada", icon::CHECK_CIRCLE);
    wait_for_label(&mut harness, &who);
    into_view(&mut harness, "Sign out");
    render(&mut harness, "profile-accounts");
    harness
        .get_by_role_and_label(Role::Button, "Sign out")
        .click();
    settle(&mut harness, "signed out", wanting_sign_in(true));
    assert!(!std::fs::read_to_string(&store).unwrap().contains(TOKEN));
}

#[test]
fn not_now_hides_the_banner_and_leaves_the_top_bar_s_way() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    std::fs::write(sandbox.path("credentials"), "").unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "nobody signed in", wanting_sign_in(true));
    wait_for_label(&mut harness, "Not now");
    harness.get_by_label("Not now").click();
    harness.run_steps(2);
    assert!(!shows(&harness, "Not now"));
    // The top bar's way stays, and opens the dialog.
    harness.get_by_label("Sign in to GitLab").click();
    wait_for_label(&mut harness, "Paste it here");
}

#[test]
fn a_sign_in_nothing_keeps_holds_everywhere_until_a_helper_is_set_up() {
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(ForgeKind::GitLab, TOKEN, "ada").unwrap();
    let (work, _, _) = forge_clone(&sandbox, &fake, "team/sub/tool", TOKEN);
    // No credential helper anywhere: nothing keeps a sign-in.
    sandbox
        .run(&work, "config", &["--unset", "credential.helper"])
        .unwrap();
    // A second clone of the same project, opened later.
    let second = sandbox.path("second");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", work.to_str().unwrap(), second.to_str().unwrap()],
        )
        .unwrap();
    let url = format!("http://{}/team/sub/tool.git", fake.address());
    let bare = sandbox.path("origin.git");
    let rewrite = format!("url.file://{}.insteadOf", bare.to_str().unwrap());
    sandbox
        .run(&second, "remote", &["set-url", "origin", &url])
        .unwrap();
    sandbox
        .run(&second, "config", &[rewrite.as_str(), url.as_str()])
        .unwrap();
    sandbox
        .run(&second, "config", &["egit.gitlabHost", &fake.host_entry()])
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &work);
    settle(&mut harness, "the history", history_of(2));
    settle(&mut harness, "nobody signed in", wanting_sign_in(true));

    // Signed in, but nothing keeps it: the helpers installed are offered.
    harness.get_by_label("Sign in…").click();
    wait_for_label(&mut harness, "Paste it here");
    type_into(&mut harness, Role::PasswordInput, "Token", TOKEN);
    click_lowest(&harness, "Sign in");
    wait_for_label(&mut harness, "In a file, unencrypted (store)");
    assert!(shows(&harness, "In memory, for a day (cache)"));
    render(&mut harness, "sign-in-unkept");
    harness.get_by_label("Done").click();
    settle(&mut harness, "the merge requests", pull_requests_read(2));

    // A repository opened after it reads with it too.
    harness
        .state_mut()
        .state
        .actions
        .push(Action::Open(Location::here(second)));
    settle(&mut harness, "the second's merge requests", |app| {
        app.state.repos.get(1).is_some_and(|repo| {
            !repo.forge.is_reading()
                && repo.forge.needs_sign_in().is_none()
                && repo.forge.found().pulls.len() == 2
        })
    });

    // Profile's credentials offer the same: store set up, it keeps the sign-in.
    harness.state_mut().state.actions.push(Action::ShowProfile);
    wait_for_label(&mut harness, "In a file, unencrypted (store)");
    into_view(&mut harness, "In a file, unencrypted (store)");
    harness
        .get_by_role_and_label(Role::Button, "In a file, unencrypted (store)")
        .click();
    wait_for_label(
        &mut harness,
        &format!(
            "{}  In a file, unencrypted (store) keeps it now: egit stays signed in after it \
             quits.",
            icon::CHECK_CIRCLE
        ),
    );
    let kept = std::fs::read_to_string(sandbox.home().join(".git-credentials")).unwrap();
    assert!(kept.contains(&format!("ada:{TOKEN}@")), "{kept}");
    let helpers = sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "--get-all", "credential.helper"],
        )
        .unwrap();
    assert_eq!(String::from_utf8(helpers.stdout).unwrap().trim(), "store");
    // Profile then lists it.
    wait_for_label(&mut harness, "store");
}
