//! Preferences and the command palette, driven as a user drives them: preferences changed in
//! their tab (from the status bar) and in effect for a repository opened after (dates as how
//! long ago, a file's diff side by side, no background fetch), kept in the settings; and the
//! palette (⌘K) narrowing commands as letters are typed, saying why one can't run, running one
//! by Enter (the reflog, a switch to a branch), and the application's own shortcuts (⌘B, ⌘,,
//! ⌘W, ⌘1); then the tabs' menu, their +, and a middle click closing one.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::patches::wait_for_text;
use super::screens::{
    branchy, harness, history_shown, open_path, profile_loaded, render, settle, type_into,
    wait_for_label,
};
use crate::app::EgitApp;
use crate::preferences::{Dates, FetchEvery};
use crate::tabs::Pane;
use crate::theme::icon;

fn opened(sandbox: &Sandbox, repo: &std::path::Path) -> Harness<'static, EgitApp> {
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, repo);
    settle(&mut harness, "the history", history_shown);
    harness
}

#[test]
fn preferences_change_how_egit_behaves_and_are_kept() {
    let sandbox = Sandbox::new().unwrap();
    let repo = branchy(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);

    harness
        .get_by_label(&format!("{}  Preferences", icon::GEAR_SIX))
        .click();
    wait_for_label(&mut harness, "Side by side");
    render(&mut harness, "preferences");
    harness.get_by_label("Side by side").click();
    harness.get_by_label("5 minutes ago").click();
    harness.get_by_label("Fetch in the background").click();
    wait_for_label(&mut harness, "Never");
    harness.get_by_label("Never").click();
    harness.run_steps(2);
    let preferences = harness.state().state.settings.preferences;
    assert!(preferences.files.split_diffs);
    assert_eq!(preferences.dates, Dates::Ago);
    assert_eq!(preferences.fetch, FetchEvery::Never);

    // A repository opened now: no background fetch, dates as how long ago, a diff side by
    // side.
    harness
        .get_by_label(&format!("{}  Home", icon::HOUSE))
        .click();
    harness.run_steps(2);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    assert_eq!(harness.state().state.repos[0].tasks.fetches_every(), None);
    wait_for_text(&mut harness, "just now");
    harness
        .get_by_role_and_label(Role::Button, "Make the parser answer")
        .click();
    wait_for_label(&mut harness, "parser.rs");
    harness
        .get_by_role_and_label(Role::Button, "parser.rs")
        .click();
    settle(&mut harness, "the file open", |app| {
        app.state.repos[0].history_view.file_split().is_some()
    });
    assert_eq!(
        harness.state().state.repos[0].history_view.file_split(),
        Some(true)
    );
}

#[test]
fn the_palette_runs_commands_and_the_shortcuts_reach_them() {
    let sandbox = Sandbox::new().unwrap();
    let repo = branchy(&sandbox);
    let mut harness = opened(&sandbox, &repo);
    let field = "Command, branch, or repository";
    let palette = |harness: &mut Harness<'_, EgitApp>, typed: &str| {
        harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
        wait_for_label(harness, field);
        type_into(harness, Role::TextInput, field, typed);
    };

    // A command found by a few of its letters, run by Enter.
    palette(&mut harness, "reflg");
    render(&mut harness, "palette");
    harness.key_press(Key::Enter);
    wait_for_label(&mut harness, "Reflog of HEAD");
    assert!(harness.query_by_label(field).is_none(), "closed");

    // One that can't run says why, and Enter leaves it be.
    palette(&mut harness, "pull");
    wait_for_label(&mut harness, "Pull: The branch has no upstream");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label(field).is_none());

    // A branch switched to.
    palette(&mut harness, "switch parser");
    wait_for_label(&mut harness, "Switch to parser");
    harness.key_press(Key::Enter);
    wait_for_label(&mut harness, "Switched to parser.");

    // The application's own shortcuts.
    let hide = harness.get_by_label("Hide the sidebar").rect();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    wait_for_label(&mut harness, "Show the sidebar");
    // Its strip's button: where the hiding one was on the head band (level with the column
    // titles), centred across the strip (less its edge's line).
    let show = harness.get_by_label("Show the sidebar").rect();
    assert!(
        (show.center().y - hide.center().y).abs() < 0.5
            && (show.center().x - (crate::widgets::HEAD - 1.0) / 2.0).abs() < 0.5,
        "{show:?}, hidden from {hide:?}"
    );
    render(&mut harness, "sidebar-strip");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    wait_for_label(&mut harness, "Hide the sidebar");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    wait_for_label(&mut harness, "Keyboard shortcuts");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::W);
    harness.run_steps(2);
    assert!(
        harness.query_by_label("Keyboard shortcuts").is_none(),
        "closed"
    );
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    wait_for_label(&mut harness, "Open a repository");

    // The tabs' own ways: the others closed from a tab's menu, + going Home, and a middle
    // click closing a repository's.
    let tab = format!("{}  project", icon::GIT_BRANCH);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    wait_for_label(&mut harness, "Keyboard shortcuts");
    harness.get_by_label(&tab).click_secondary();
    wait_for_label(&mut harness, "Close the other tabs");
    render(&mut harness, "tab-menu");
    harness.get_by_label("Close the other tabs").click();
    harness.run_steps(2);
    let tabs = &harness.state().tabs;
    assert!(tabs.tiles.find_pane(&Pane::Preferences).is_none(), "closed");
    assert!(tabs.tiles.find_pane(&Pane::Home).is_some(), "Home stays");
    harness.get_by_label(&tab).click();
    wait_for_label(&mut harness, "Hide the sidebar");
    harness.get_by_label("Open or clone a repository").click();
    wait_for_label(&mut harness, "Open a repository");
    let at = harness.get_by_label(&tab).rect().center();
    for pressed in [true, false] {
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_steps(2);
    assert!(harness.query_by_label(&tab).is_none(), "closed");
    assert!(harness.state().state.repos.is_empty());
}
