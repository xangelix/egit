//! The Preferences tab: how egit looks (the theme, the text's size), how repositories are kept
//! up to date (the background fetch) and laid out (the sidebar), how diffs and files show at
//! first (side by side, lines wrapped, a tab's width; changed files as a list or a tree), and
//! commit dates; then the keyboard shortcuts. Each change is in effect at once, and kept.

use egui::{ComboBox, Grid, RichText, Ui};

use crate::app::{Action, AppState};
use crate::commands::{Command, keys_text};
use crate::preferences::{Dates, FetchEvery, FileLists, Preferences, TAB_WIDTHS, ZOOM};
use crate::theme::ThemeChoice;
use crate::widgets::{self, muted};

/// Draws the Preferences tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    super::page(ui, "preferences", |ui| {
        widgets::page_title(ui, "Preferences");
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

/// The width of the preferences' labels.
const LABEL: f32 = 150.0;

/// The theme, the text's size, and how commit dates show.
fn appearance(ui: &mut Ui, state: &mut AppState, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Appearance");
        ui.add_space(8.0);
        let mut theme = state.settings.theme;
        widgets::fact(ui, LABEL, "Theme", |ui| {
            let options = [
                (ThemeChoice::System, "As the desktop's"),
                (ThemeChoice::Light, "Light"),
                (ThemeChoice::Dark, "Dark"),
            ];
            if widgets::segmented(ui, &mut theme, &options) {
                state.actions.push(Action::SetTheme(theme));
            }
        });
        widgets::form_row(ui, LABEL, "Text size", |ui| {
            let mut zoom = preferences.zoom;
            let slider = ui.add(egui::Slider::new(&mut zoom, ZOOM).step_by(10.0).suffix("%"));
            preferences.zoom = zoom;
            if widgets::secondary(ui, preferences.zoom != 100, "Reset").clicked() {
                preferences.zoom = 100;
            }
            ui.label(muted(
                ui,
                format!(
                    "{}, {}, {}",
                    keys_text("⌘+"),
                    keys_text("⌘−"),
                    keys_text("⌘0")
                ),
            ));
            slider
        });
        widgets::fact(ui, LABEL, "Commit dates", |ui| {
            widgets::segmented(
                ui,
                &mut preferences.dates,
                &[
                    (Dates::Written, "2026-10-17 14:05"),
                    (Dates::Ago, "5 minutes ago"),
                ],
            );
        });
    });
}

/// How repositories are kept up to date, and laid out.
fn repositories(ui: &mut Ui, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Repositories");
        ui.add_space(8.0);
        widgets::form_row(ui, LABEL, "Fetch in the background", |ui| {
            let combo = ComboBox::from_id_salt("preferences-fetch")
                .selected_text(preferences.fetch.label())
                .show_ui(ui, |ui| {
                    for every in FetchEvery::ALL {
                        ui.selectable_value(&mut preferences.fetch, every, every.label());
                    }
                })
                .response;
            ui.label(muted(
                ui,
                "Never asking: a remote that needs a password waits for your own fetch.",
            ));
            combo
        });
        widgets::fact(ui, LABEL, "Sidebar", |ui| {
            widgets::checkbox(
                ui,
                &mut preferences.sidebar,
                "Shown when a repository opens",
            );
        });
    });
}

/// How diffs and files show at first.
fn files(ui: &mut Ui, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Diffs and files");
        ui.label(muted(
            ui,
            "How a file shows when it opens, and lists of them.",
        ));
        ui.add_space(8.0);
        widgets::fact(ui, LABEL, "Diffs", |ui| {
            widgets::segmented(
                ui,
                &mut preferences.files.split_diffs,
                &[(false, "Unified"), (true, "Side by side")],
            );
        });
        widgets::fact(ui, LABEL, "Long lines", |ui| {
            widgets::checkbox(ui, &mut preferences.files.wrap, "Wrapped");
        });
        widgets::fact(ui, LABEL, "Changed files", |ui| {
            widgets::segmented(
                ui,
                &mut preferences.files.lists,
                &[
                    (FileLists::List, "A list"),
                    (FileLists::Tree, "A tree of folders"),
                ],
            );
        });
        widgets::fact(ui, LABEL, "Conflicts", |ui| {
            widgets::checkbox(
                ui,
                &mut preferences.files.merge_panes,
                "Both sides whole above the merged file",
            );
        });
        widgets::form_row(ui, LABEL, "Tab width", |ui| {
            ComboBox::from_id_salt("preferences-tab-width")
                .selected_text(format!("{} cells", preferences.files.tab_width))
                .show_ui(ui, |ui| {
                    for width in TAB_WIDTHS {
                        ui.selectable_value(
                            &mut preferences.files.tab_width,
                            width,
                            format!("{width} cells"),
                        );
                    }
                })
                .response
        });
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
