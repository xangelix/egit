//! The commit table's columns: each line between them dragged follows the pointer, trading
//! width between its two columns (the message's by filling what is left) while nothing else
//! moves, holds still once let go, and a double click puts its columns back.

use egit_git::testing::Sandbox;
use egui::{Id, Pos2};
use egui_kittest::Harness;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, two_branches,
};
use crate::app::EgitApp;

/// Where the lines between the columns were drawn last frame.
fn lines(harness: &Harness<'_, EgitApp>) -> [f32; 4] {
    harness
        .ctx
        .data(|data| data.get_temp::<[f32; 4]>(Id::new("commit-column-xs")))
        .unwrap()
}

/// Drags line `at` by `by` points, a few frames of movement between, at `y`.
fn drag(harness: &mut Harness<'_, EgitApp>, at: usize, by: f32, y: f32) {
    let start = Pos2::new(lines(harness)[at], y);
    harness.hover_at(start);
    harness.step();
    harness.drag_at(start);
    harness.step();
    for step in 1..=5_u8 {
        harness.hover_at(Pos2::new(start.x + by * f32::from(step) / 5.0, y));
        harness.step();
    }
    harness.drop_at(Pos2::new(start.x + by, y));
    harness.run_steps(3);
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.5
}

#[test]
fn column_lines_follow_the_pointer_and_hold_still() {
    let sandbox = Sandbox::new().unwrap();
    let repo = two_branches(&sandbox);
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    harness.run_steps(2);
    let first = lines(&harness);
    // In the column titles' band.
    let y = 100.0;

    // Between the author and the date: the author wider, the date narrower, nothing else.
    drag(&mut harness, 2, 30.0, y);
    let moved = lines(&harness);
    assert!(close(moved[2], first[2] + 30.0), "{moved:?} from {first:?}");
    for at in [0, 1, 3] {
        assert!(
            close(moved[at], first[at]),
            "line {at}: {moved:?} from {first:?}"
        );
    }
    // Let go, it holds still.
    harness.run_steps(6);
    assert_eq!(lines(&harness), moved);

    // Between the message and the author: the author wider, the message narrower; the lines
    // right of it stay.
    drag(&mut harness, 1, -50.0, y);
    let widened = lines(&harness);
    assert!(
        close(widened[1], moved[1] - 50.0),
        "{widened:?} from {moved:?}"
    );
    assert!(
        close(widened[2], moved[2]) && close(widened[3], moved[3]),
        "{widened:?}"
    );
    harness.run_steps(6);
    assert_eq!(lines(&harness), widened);
    render(&mut harness, "commit-columns");

    // A double click puts the author back as it was at first.
    let at = Pos2::new(widened[1], y);
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
    harness.run_steps(3);
    let back = lines(&harness);
    assert!(close(back[2] - back[1], 150.0), "{back:?}");
}
