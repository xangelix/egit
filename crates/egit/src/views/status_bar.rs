//! The bar along the window's bottom: which git egit runs, what isn't working, and the theme.

use egui::Ui;

use crate::app::{Action, AppState};
use crate::format::display_path;
use crate::theme::{ThemeChoice, icon};
use crate::widgets::muted;

/// Draws the status bar.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        let home = state.home_dir.clone();
        let git_text = match state.git.read_as_ref() {
            Some(Ok(git)) => format!(
                "{}  git {} · {}",
                icon::TERMINAL,
                git.version(),
                display_path(git.program(), home.as_deref())
            ),
            Some(Err(_)) => format!("{}  git isn't usable", icon::WARNING),
            None => format!("{}  Looking for git…", icon::HOURGLASS),
        };
        ui.label(muted(ui, git_text).small());
        if let Some(reason) = state.watch_problem() {
            ui.label(
                egui::RichText::new(format!(
                    "{}  Changes made outside egit aren't followed",
                    icon::WARNING
                ))
                .small()
                .color(crate::theme::Palette::of(ui.ctx()).warn),
            )
            .on_hover_text(format!(
                "egit can't watch for them: {reason}. It keeps trying, and catches up once it can."
            ));
        }
        if let Some(reason) = &state.prompts.unavailable {
            ui.label(
                egui::RichText::new(format!(
                    "{}  git can't ask you for passwords",
                    icon::WARNING
                ))
                .small()
                .color(crate::theme::Palette::of(ui.ctx()).warn),
            )
            .on_hover_text(reason.as_str());
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let current = state.settings.theme;
            let label = match current {
                ThemeChoice::System => format!("{}  System theme", icon::MONITOR),
                ThemeChoice::Dark => format!("{}  Dark", icon::MOON),
                ThemeChoice::Light => format!("{}  Light", icon::SUN),
            };
            ui.menu_button(muted(ui, label).small(), |ui| {
                for (choice, text) in [
                    (
                        ThemeChoice::System,
                        format!("{}  System theme", icon::MONITOR),
                    ),
                    (ThemeChoice::Light, format!("{}  Light", icon::SUN)),
                    (ThemeChoice::Dark, format!("{}  Dark", icon::MOON)),
                ] {
                    if ui.radio(current == choice, text).clicked() {
                        state.actions.push(Action::SetTheme(choice));
                        ui.close();
                    }
                }
            });
        });
    });
}
