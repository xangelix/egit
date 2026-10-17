//! The Preferences tab: how egit looks (the theme, the text's size), how repositories are kept
//! up to date (the background fetch) and laid out (the sidebar), how diffs and files show at
//! first (side by side, lines wrapped, a tab's width), and commit dates; then the keyboard
//! shortcuts. Each change is in effect at once, and kept.

use egui::{ComboBox, Grid, RichText, Ui};

use crate::app::{Action, AppState};
use crate::commands::{Command, keys_text};
use crate::preferences::{Dates, FetchEvery, Preferences, TAB_WIDTHS, ZOOM};
use crate::theme::{ThemeChoice, icon};
use crate::widgets::{self, muted};

/// Draws the Preferences tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    super::page(ui, "preferences", |ui| {
        ui.label(
            RichText::new(format!("{}  Preferences", icon::GEAR_SIX))
                .size(22.0)
                .strong(),
        );
        ui.label(muted(ui, "In effect at once, and kept for next time."));
        ui.add_space(16.0);
        let mut preferences = state.settings.preferences;
        appearance(ui, state, &mut preferences);
        ui.add_space(12.0);
        repositories(ui, &mut preferences);
        ui.add_space(12.0);
        files(ui, &mut preferences);
        ui.add_space(12.0);
        shortcuts(ui);
        if preferences != state.settings.preferences {
            state.actions.push(Action::SetPreferences(preferences));
        }
        ui.add_space(24.0);
    });
}

/// The theme, the text's size, and how commit dates show.
fn appearance(ui: &mut Ui, state: &mut AppState, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Appearance");
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Theme");
            let current = state.settings.theme;
            for (choice, text) in [
                (ThemeChoice::System, "As the desktop's"),
                (ThemeChoice::Light, "Light"),
                (ThemeChoice::Dark, "Dark"),
            ] {
                if ui.radio(current == choice, text).clicked() && current != choice {
                    state.actions.push(Action::SetTheme(choice));
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let mut zoom = preferences.zoom;
            ui.add(
                egui::Slider::new(&mut zoom, ZOOM)
                    .step_by(10.0)
                    .suffix("%")
                    .text("Text size"),
            );
            preferences.zoom = zoom;
            if widgets::secondary(ui, preferences.zoom != 100, "Reset").clicked() {
                preferences.zoom = 100;
            }
            ui.label(muted(
                ui,
                format!(
                    "({} and {} too; {} resets it)",
                    keys_text("⌘+"),
                    keys_text("⌘−"),
                    keys_text("⌘0")
                ),
            ));
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Commit dates");
            ui.radio_value(
                &mut preferences.dates,
                Dates::Written,
                "As dates (2026-10-17 14:05)",
            );
            ui.radio_value(
                &mut preferences.dates,
                Dates::Ago,
                "As how long ago (5 minutes ago)",
            );
        });
    });
}

/// How repositories are kept up to date, and laid out.
fn repositories(ui: &mut Ui, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Repositories");
        ui.add_space(8.0);
        ComboBox::from_label("Fetch from the remotes in the background")
            .selected_text(preferences.fetch.label())
            .show_ui(ui, |ui| {
                for every in FetchEvery::ALL {
                    ui.selectable_value(&mut preferences.fetch, every, every.label());
                }
            });
        ui.label(muted(
            ui,
            "Without asking for anything: a remote that needs a password is fetched from when \
             you fetch.",
        ));
        ui.add_space(4.0);
        ui.checkbox(
            &mut preferences.sidebar,
            "Show the sidebar when a repository opens",
        );
    });
}

/// How diffs and files show at first.
fn files(ui: &mut Ui, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Diffs and files");
        ui.add_space(8.0);
        ui.checkbox(
            &mut preferences.files.split_diffs,
            "Diffs side by side at first",
        );
        ui.checkbox(&mut preferences.files.wrap, "Long lines wrapped at first");
        ComboBox::from_label("Tab width")
            .selected_text(format!("{} cells", preferences.files.tab_width))
            .show_ui(ui, |ui| {
                for width in TAB_WIDTHS {
                    ui.selectable_value(
                        &mut preferences.files.tab_width,
                        width,
                        format!("{width} cells"),
                    );
                }
            });
        ui.label(muted(ui, "For the files and diffs opened from now on."));
    });
}

/// Every keyboard shortcut.
fn shortcuts(ui: &mut Ui) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Keyboard shortcuts");
        ui.add_space(8.0);
        Grid::new("preferences-shortcuts")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                for (keys, what) in Command::shortcuts() {
                    ui.label(RichText::new(keys_text(keys)).monospace());
                    ui.label(what);
                    ui.end_row();
                }
            });
    });
}
