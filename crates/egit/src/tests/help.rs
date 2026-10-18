//! Help, both ways in: Alt held over a control shows its topic beside it (gone when Alt is let
//! go); F1 opens every topic, searchable, and Escape closes it.

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, rendering, screens, settle,
    shared_remote, type_into, wait_for_label,
};
use crate::theme::icon;

#[test]
fn alt_explains_a_control_and_f1_shows_every_topic() {
    let sandbox = Sandbox::new().unwrap();
    let (_colleague, mine) = shared_remote(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &mine);
    settle(&mut harness, "the history", history_shown);
    harness.state_mut().state.repos[0]
        .tasks
        .hold_background_fetch();

    // Alt held over Push: what pushing does, and its surprises.
    let push = format!("{}  Push", icon::ARROW_UP);
    wait_for_label(&mut harness, &push);
    harness.event(Event::ModifiersChanged(Modifiers::ALT));
    harness.get_by_label(&push).hover();
    harness.run_steps(2);
    wait_for_label(&mut harness, "Pushing");
    wait_for_label(&mut harness, "Rejected as behind?");
    render(&mut harness, "help-alt");
    // The overlay as it shows (the audit's pointer elsewhere, now back on Push).
    harness.get_by_label(&push).hover();
    harness.run_steps(2);
    if rendering()
        && let Ok(image) = harness.render()
    {
        let _saved = image.save(screens().join("help-alt-overlay.png"));
    }
    // Over a commit's row: the graph's topic.
    harness.get_by_role_and_label(Role::Button, "Start").hover();
    harness.run_steps(2);
    wait_for_label(&mut harness, "The commit graph");
    // Let go: gone.
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.run_steps(2);
    assert!(harness.query_by_label("The commit graph").is_none());

    // F1: every topic, found by what is typed.
    harness.key_press(Key::F1);
    wait_for_label(&mut harness, "How egit works");
    type_into(&mut harness, Role::TextInput, "Find a topic", "submodule");
    harness.run_steps(2);
    assert!(harness.query_by_label("Pushing").is_none(), "narrowed");
    harness.get_by_label("Mending submodules").click();
    wait_for_label(&mut harness, "A submodule you moved on yourself?");
    render(&mut harness, "help-window");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(!harness.state().state.help.is_open());
}
