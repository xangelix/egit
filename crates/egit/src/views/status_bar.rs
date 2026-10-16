//! The bar along the window's bottom: which git egit runs, and the theme.

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
