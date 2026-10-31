//! The bar along the window's bottom: which git egit runs, what isn't working, the theme, the
//! preferences, and the command palette. On a narrow window (a phone's) its buttons are icons
//! (named in words) and what isn't working an icon each, its words on hover; which git, only
//! when it isn't usable.

use egui::Ui;

use crate::app::{Action, AppState};
use crate::commands::keys_text;
use crate::format::display_path;
use crate::theme::{ThemeChoice, icon};
use crate::widgets::muted;

/// Draws the status bar.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    let narrow = crate::narrow::is_narrow(ui.ctx());
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Quiet, as a toolbar's: the bar stays out of the way until wanted.
            let help = bar_button(ui, (icon::QUESTION, "Help"), Some("F1"), narrow);
            if help
                .on_hover_text("What everything does, at git's level (or hold Alt over anything)")
                .clicked()
            {
                state.actions.push(Action::ShowHelp);
            }
            let commands = bar_button(
                ui,
                (icon::COMMAND, "Commands"),
                Some(&keys_text("⌘K")),
                narrow,
            );
            if commands.on_hover_text("Every command, by name").clicked() {
                state.actions.push(Action::ShowPalette);
            }
            let preferences = bar_button(ui, (icon::GEAR_SIX, "Preferences"), None, narrow);
            if preferences
                .on_hover_text(format!("How egit looks and behaves ({})", keys_text("⌘,")))
                .clicked()
            {
                state.actions.push(Action::ShowPreferences);
            }
            let current = state.settings.theme;
            let (glyph, words) = match current {
                ThemeChoice::System => (icon::MONITOR, "System theme"),
                ThemeChoice::Dark => (icon::MOON, "Dark"),
                ThemeChoice::Light => (icon::SUN, "Light"),
            };
            let label = if narrow {
                glyph.to_owned()
            } else {
                format!("{glyph}  {words}")
            };
            let button = egui::Button::new(muted(ui, label).small())
                .frame_when_inactive(false)
                .min_size(egui::vec2(crate::widgets::TARGET, crate::widgets::TARGET));
            let (theme, _) = crate::widgets::quiet(ui, |ui| {
                egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                    for (choice, text) in [
                        (
                            ThemeChoice::System,
                            format!("{}  System theme", icon::MONITOR),
                        ),
                        (ThemeChoice::Light, format!("{}  Light", icon::SUN)),
                        (ThemeChoice::Dark, format!("{}  Dark", icon::MOON)),
                    ] {
                        if crate::widgets::radio(ui, current == choice, text).clicked() {
                            state.actions.push(Action::SetTheme(choice));
                            ui.close();
                        }
                    }
                })
            });
            if narrow {
                theme.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, words)
                });
                crate::help::note(ui, theme.rect, words);
            }
            // In what the buttons leave: cut short rather than under them.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                said(ui, state, narrow);
            });
        });
    });
}

/// Which git egit runs, and what isn't working.
fn said(ui: &mut Ui, state: &mut AppState, narrow: bool) {
    let home = state.home_dir.clone();
    let git_text = match state.git.read_as_ref() {
        Some(Ok(_)) if narrow => None,
        Some(Ok(git)) => Some(format!(
            "{}  git {} · {}",
            icon::TERMINAL,
            git.version(),
            display_path(git.program(), home.as_deref())
        )),
        Some(Err(_)) => Some(format!("{}  git isn't usable", icon::WARNING)),
        None => Some(format!("{}  Looking for git…", icon::HOURGLASS)),
    };
    if let Some(git_text) = git_text {
        ui.add(egui::Label::new(muted(ui, git_text).small()).truncate());
    }
    if let Some(reason) = state.watch_problem() {
        warning(
            ui,
            "Changes made outside egit aren't followed",
            &format!(
                "egit can't watch for them: {reason}. It keeps trying, and catches up once it \
                 can."
            ),
            narrow,
        );
    }
    if let Some(reason) = &state.prompts.unavailable {
        warning(ui, "git can't ask you for passwords", reason, narrow);
    }
}

/// One of the bar's buttons: its icon and words, and the keys it is on (`keys`); on a narrow
/// window its icon alone, named in words.
fn bar_button(
    ui: &mut Ui,
    (glyph, words): (&str, &str),
    keys: Option<&str>,
    narrow: bool,
) -> egui::Response {
    let text = match keys {
        _ if narrow => glyph.to_owned(),
        Some(keys) => format!("{glyph}  {words}  {keys}"),
        None => format!("{glyph}  {words}"),
    };
    let button = quiet_button(ui, muted(ui, text).small());
    if narrow {
        button.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, words));
    }
    crate::help::note(ui, button.rect, words);
    button
}

/// What isn't working, in the warning's colour: its words and, on hover, `why`; on a narrow
/// window its icon alone, the words on hover too.
fn warning(ui: &mut Ui, words: &str, why: &str, narrow: bool) {
    let text = if narrow {
        icon::WARNING.to_owned()
    } else {
        format!("{}  {words}", icon::WARNING)
    };
    let shown = ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .small()
                .color(crate::theme::Palette::of(ui.ctx()).warn),
        )
        .truncate(),
    );
    let hover = if narrow {
        format!("{words}: {why}")
    } else {
        why.to_owned()
    };
    shown.on_hover_text(hover);
}

/// A button without a frame until the pointer is on it.
fn quiet_button(ui: &mut Ui, text: egui::RichText) -> egui::Response {
    crate::widgets::quiet(ui, |ui| {
        ui.add(
            egui::Button::new(text)
                .frame_when_inactive(false)
                .min_size(egui::vec2(crate::widgets::TARGET, crate::widgets::TARGET)),
        )
    })
}
