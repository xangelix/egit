//! Quick diff, driven as a user drives it, with no repository open: reached from the status bar,
//! Home, the palette, and its keys; two texts pasted one after the other, the keys moving on; what
//! differs between them shown unified and side by side, whole, and ignoring whitespace; back to
//! them, swapped, compared again; the same texts said to be; a text too long for a field shown on
//! the grid and compared; and all of it gone with the tab.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::patches::wait_for_text;
use super::screens::{
    click_when_enabled, harness, phone, profile_loaded, render, settle, type_into, wait_for_label,
};
use crate::app::{Action, EgitApp};
use crate::tabs::{Closing, Pane};
use crate::theme::icon;
use crate::views::quick_diff::QuickDiffView;

/// The tab's state.
fn view<'a>(harness: &'a mut Harness<'_, EgitApp>) -> &'a mut QuickDiffView {
    harness.state_mut().state.quick_diff.as_mut().unwrap()
}

/// Pastes `text`, and lets it land.
fn paste(harness: &mut Harness<'_, EgitApp>, text: &str) {
    harness.event(Event::Paste(text.to_owned()));
    harness.run_steps(2);
}

/// Opens the tab from Home's button, once git is found.
fn opened(sandbox: &Sandbox) -> Harness<'static, EgitApp> {
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    click_when_enabled(&mut harness, "Compare two texts");
    wait_for_label(&mut harness, "Before");
    harness
}

/// Waits for the diff to have `lines` lines.
fn diff_of(harness: &mut Harness<'_, EgitApp>, lines: u32) {
    settle(harness, "the diff", move |app| {
        app.state.quick_diff.as_mut().unwrap().diff_lines() == Some(lines)
    });
    harness.run_steps(2);
}

#[test]
fn two_texts_pasted_show_what_differs_between_them() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = opened(&sandbox);
    let compare = |harness: &Harness<'_, EgitApp>| {
        !harness
            .get_by_role_and_label(Role::Button, "Compare")
            .accesskit_node()
            .is_disabled()
    };
    assert!(!compare(&harness), "nothing to compare yet");
    render(&mut harness, "quick-diff-empty");

    // Two pastes, no clicks: the keys move on.
    paste(&mut harness, "one\ntwo\nthree\nfour\n");
    paste(&mut harness, "one\nTWO\nthree\nfour\nfive\n");
    assert_eq!(
        view(&mut harness).fields(),
        ("one\ntwo\nthree\nfour\n", "one\nTWO\nthree\nfour\nfive\n")
    );
    wait_for_label(&mut harness, "4 lines");
    assert!(compare(&harness));
    type_into(&mut harness, Role::TextInput, "File name", "notes.rs");
    render(&mut harness, "quick-diff-texts");

    // Its keys, from a field: its head, what is kept, one line each way, and one more.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    diff_of(&mut harness, 7);
    wait_for_label(&mut harness, "+2");
    wait_for_label(&mut harness, "−1");
    render(&mut harness, "quick-diff");

    harness.get_by_label("Side by side").click();
    settle(&mut harness, "the sides", |app| {
        app.state.quick_diff.as_mut().unwrap().sides_shown()
    });
    harness.run_steps(2);
    render(&mut harness, "quick-diff-side-by-side");
    harness.get_by_label("Unified").click();
    harness.run_steps(2);

    // Back by Escape: the texts as they were.
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(!view(&mut harness).is_comparing());
    assert_eq!(view(&mut harness).fields().0, "one\ntwo\nthree\nfour\n");

    // They change places, and so do the differences.
    harness
        .get_by_label(&format!("{}  Swap", icon::ARROWS_LEFT_RIGHT))
        .click();
    harness.run_steps(2);
    assert_eq!(
        view(&mut harness).fields().0,
        "one\nTWO\nthree\nfour\nfive\n"
    );
    click_when_enabled(&mut harness, "Compare");
    diff_of(&mut harness, 7);
    wait_for_label(&mut harness, "+1");
    wait_for_label(&mut harness, "−2");
    harness
        .get_by_label(&format!("{}  Edit the texts", icon::ARROW_LEFT))
        .click();
    harness.run_steps(2);

    // The tab closed, nothing is left: open again, it is empty.
    harness
        .state_mut()
        .state
        .actions
        .push(Action::CloseTabs(Closing::This(Pane::QuickDiff)));
    harness.run_steps(2);
    assert!(harness.state().state.quick_diff.is_none());
    harness
        .state_mut()
        .state
        .actions
        .push(Action::Show(Pane::QuickDiff));
    wait_for_label(&mut harness, "Before");
    assert_eq!(view(&mut harness).fields(), ("", ""));
}

#[test]
fn the_same_texts_are_said_to_be() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = opened(&sandbox);
    // One stopping short of its last newline is no difference.
    paste(&mut harness, "same\ntext");
    paste(&mut harness, "same\ntext\n");
    click_when_enabled(&mut harness, "Compare");
    wait_for_label(&mut harness, "The two texts are the same.");
    render(&mut harness, "quick-diff-same");
    harness.key_press(Key::Escape);
    harness.run_steps(2);

    // Spaces alone differ: said, while they are ignored.
    harness.get_by_label("Clear the text after").click();
    harness.run_steps(2);
    paste(&mut harness, "same\n  text\n");
    click_when_enabled(&mut harness, "Compare");
    diff_of(&mut harness, 4);
    harness
        .get_by_role_and_label(Role::CheckBox, "Ignore whitespace")
        .click();
    wait_for_label(
        &mut harness,
        "Nothing but whitespace differs (Ignore whitespace is on).",
    );

    // Not ignoring them, the whole text shows as well as what differs.
    harness
        .get_by_role_and_label(Role::CheckBox, "Ignore whitespace")
        .click();
    diff_of(&mut harness, 4);
    harness
        .get_by_role_and_label(Role::CheckBox, "Whole file")
        .click();
    diff_of(&mut harness, 4);
}

#[test]
fn a_text_too_long_for_a_field_is_shown_on_the_grid_and_compared() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = opened(&sandbox);
    let line = |word: &str, n: usize| format!("{word} {n}\n");
    let before: String = (0..20_000).map(|n| line("line", n)).collect();
    let after = before.replace("line 19999\n", "changed\n");
    assert!(before.len() > 64 * 1024);

    // A paste too long never reaches the field; the keys move on to the other.
    paste(&mut harness, &before);
    assert_eq!(view(&mut harness).long(), (true, false));
    assert_eq!(view(&mut harness).fields(), ("", ""));
    wait_for_text(&mut harness, ": too long to edit here");
    paste(&mut harness, &after);
    assert_eq!(view(&mut harness).long(), (true, true));
    wait_for_text(&mut harness, ": too long to edit here");
    render(&mut harness, "quick-diff-long");

    click_when_enabled(&mut harness, "Compare");
    // A hunk's head, three lines before, and one line each way.
    diff_of(&mut harness, 6);
    render(&mut harness, "quick-diff-long-compared");
    harness.key_press(Key::Escape);
    harness.run_steps(2);

    // Cleared, its field is back, and takes the keys.
    harness.get_by_label("Clear the text before").click();
    harness.run_steps(3);
    assert_eq!(view(&mut harness).long(), (false, true));
    harness.event(Event::Text("typed".to_owned()));
    harness.run_steps(2);
    assert_eq!(view(&mut harness).fields().0, "typed");
}

#[test]
fn it_is_reached_from_everywhere_without_a_repository() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    let shown = |harness: &Harness<'_, EgitApp>| {
        harness
            .state()
            .tabs
            .tiles
            .find_pane(&Pane::QuickDiff)
            .is_some()
    };
    assert!(!shown(&harness));
    let close = |harness: &mut Harness<'_, EgitApp>| {
        harness
            .state_mut()
            .state
            .actions
            .push(Action::CloseTabs(Closing::This(Pane::QuickDiff)));
        harness.run_steps(2);
        assert!(!shown(harness));
    };

    // The status bar.
    harness
        .get_by_label(&format!("{}  Quick diff", icon::GIT_DIFF))
        .click();
    wait_for_label(&mut harness, "Before");
    assert!(shown(&harness));
    close(&mut harness);

    // Its keys.
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::D);
    wait_for_label(&mut harness, "Before");
    assert!(shown(&harness));
    close(&mut harness);

    // The palette, by a few of its letters.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    let field = "Command, branch, or repository";
    wait_for_label(&mut harness, field);
    type_into(&mut harness, Role::TextInput, field, "quick");
    harness.key_press(Key::Enter);
    wait_for_label(&mut harness, "Before");
    assert!(shown(&harness));
}

#[test]
fn a_phones_screen_stacks_the_texts_and_shows_their_differences_unified() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = phone(&sandbox, egui::Theme::Dark);
    settle(&mut harness, "the profile", profile_loaded);
    harness
        .state_mut()
        .state
        .actions
        .push(Action::Show(Pane::QuickDiff));
    wait_for_label(&mut harness, "Before");
    render(&mut harness, "narrow-quick-diff");
    paste(&mut harness, "one\ntwo\nthree\n");
    paste(&mut harness, "one\nTWO\nthree\n");
    wait_for_label(&mut harness, "3 lines");
    render(&mut harness, "narrow-quick-diff-texts");
    click_when_enabled(&mut harness, "Compare");
    diff_of(&mut harness, 5);
    assert!(harness.query_by_label("Side by side").is_none());
    render(&mut harness, "narrow-quick-diff-compared");
}

#[test]
fn nothing_that_isnt_drawn_keeps_the_keys() {
    let sandbox = Sandbox::new().unwrap();
    let mut harness = opened(&sandbox);
    let long = "line\n".repeat(20_000);
    // One frame after the other, the keys having only just moved on.
    harness.event(Event::Paste(long.clone()));
    harness.step();
    harness.event(Event::Paste(long));
    harness.step();
    harness.run_steps(2);
    assert_eq!(view(&mut harness).long(), (true, true));

    // Swapped with a field that has the keys, and typed past its room: alike.
    harness.get_by_label("Clear the text after").click();
    harness.run_steps(3);
    harness.event(Event::Text("typed".to_owned()));
    harness.run_steps(2);
    assert_eq!(view(&mut harness).fields().1, "typed");
    harness
        .get_by_label(&format!("{}  Swap", icon::ARROWS_LEFT_RIGHT))
        .click();
    harness.run_steps(2);
    assert_eq!(view(&mut harness).long(), (false, true));
    assert_eq!(view(&mut harness).fields().0, "typed");
    harness.get_by_label("Clear the text after").click();
    harness.run_steps(3);
    harness.event(Event::Text("x".repeat(70 * 1024)));
    harness.run_steps(2);
    assert_eq!(view(&mut harness).long(), (false, true));
}
