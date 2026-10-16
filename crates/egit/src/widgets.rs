//! The pieces views are built from: cards, buttons, labels for where a setting comes from, and
//! problem boxes.

use std::path::Path;

use egit_git::{ConfigEntry, ConfigScope};
use egui::{Button, CornerRadius, Frame, Margin, Response, RichText, Stroke, Ui};

use crate::format::display_path;
use crate::problem::Problem;
use crate::theme::Palette;

/// A raised panel grouping related content.
pub(crate) fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A card's or a section's title.
pub(crate) fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong());
}

/// Secondary text.
pub(crate) fn muted(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(Palette::of(ui.ctx()).muted)
}

/// The button for the main thing to do.
pub(crate) fn primary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(text).color(palette.on_accent).strong()).fill(palette.accent),
    )
}

/// Any other button.
pub(crate) fn secondary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    ui.add_enabled(enabled, Button::new(text))
}

/// Where a setting comes from: its scope and file, or git's default when nothing sets it.
pub(crate) fn origin(ui: &mut Ui, entry: Option<ConfigEntry<'_>>, home: Option<&Path>) {
    let text = origin_text(entry, home);
    let response = ui.label(muted(ui, text).small());
    if let Some(file) = entry.and_then(ConfigEntry::file) {
        response.on_hover_text(file.display().to_string());
    }
}

/// The words [`origin`] shows.
pub(crate) fn origin_text(entry: Option<ConfigEntry<'_>>, home: Option<&Path>) -> String {
    let Some(entry) = entry else {
        return "git's default".to_owned();
    };
    let scope = scope_name(entry.scope);
    entry.file().map_or_else(
        || format!("{scope} · {}", entry.origin),
        |file| format!("{scope} · {}", display_path(&file, home)),
    )
}

/// A scope as the user reads it.
pub(crate) const fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::System => "system",
        ConfigScope::Global => "global",
        ConfigScope::Local => "repository",
        ConfigScope::Worktree => "work tree",
        ConfigScope::Command => "command",
        ConfigScope::Unknown => "elsewhere",
    }
}

/// A problem, with git's words below the sentence; `add_actions` adds buttons beside them.
pub(crate) fn problem_box(ui: &mut Ui, problem: &Problem, add_actions: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.danger.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, palette.danger.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    crate::theme::icon::WARNING_CIRCLE,
                    problem.title
                ))
                .color(palette.danger)
                .strong(),
            );
            if !problem.detail.is_empty() {
                ui.add(egui::Label::new(problem.detail.as_str()).selectable(true));
            }
            ui.horizontal(add_actions);
        });
}

/// A dialog's row of buttons, laid out from the right (the first added is the rightmost), as
/// tall as its buttons.
pub(crate) fn buttons_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 0.0),
        egui::Layout::right_to_left(egui::Align::Center),
        add,
    );
}

/// A note that something is happening in the background.
pub(crate) fn working(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(muted(ui, text));
    });
}
