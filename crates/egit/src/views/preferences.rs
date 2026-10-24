//! The Preferences tab: how egit looks (the theme, the text's size), how repositories are kept
//! up to date (the background fetch) and laid out (the sidebar), how diffs and files show at
//! first (side by side, lines wrapped, a tab's width; changed files as a list or a tree), and
//! commit dates; then the keyboard shortcuts. Each change is in effect at once, and kept.

use egui::{ComboBox, Grid, Label, RichText, Ui};

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
        agents(ui, state);
        ui.add_space(12.0);
        shortcuts(ui);
        if preferences != state.settings.preferences {
            state.actions.push(Action::SetPreferences(preferences));
        }
        ui.add_space(24.0);
    });
}

/// The width of the preferences' labels: every page's.
const LABEL: f32 = widgets::FACT_LABEL;

/// The accounts agents run as, besides Claude Code's own: those there are, forgotten from here,
/// and one added (a Claude subscription in a Claude Code folder, or an API key kept by git's
/// credential helper).
fn agents(ui: &mut Ui, state: &mut AppState) {
    if let Some(added) = state.agents.adding.take_ok() {
        state.settings.agent_accounts.push(added);
        state.agents.form = crate::agents::AccountForm::default();
    }
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Agents");
        ui.add_space(4.0);
        ui.add(
            Label::new(muted(
                ui,
                "Conversations run as Claude Code is signed in here, unless one chooses another \
                 account under its prompt: a Claude subscription in a Claude Code folder of its \
                 own (signed into with Claude Code itself; on another computer, the same folder \
                 under the home folder there), or an Anthropic API key, which git's credential \
                 helper keeps (never egit's settings).",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        let mut forgotten = None;
        let home = state.home_dir.clone();
        widgets::list_box(ui, |ui| {
            let own = std::path::Path::new("~/.claude");
            account_row(
                ui,
                &mut state.agents,
                ("Claude Code's own", SignsIn::Folder(None, own)),
                None,
            );
            for (index, account) in state.settings.agent_accounts.iter().enumerate() {
                let folder = match &account.kind {
                    crate::agents::AccountKind::Folder(folder) => Some(folder.to_path()),
                    crate::agents::AccountKind::ApiKey => None,
                };
                let shown = folder
                    .as_deref()
                    .map(|folder| crate::format::display_path(folder, home.as_deref()))
                    .unwrap_or_default();
                let signs_in = folder.as_deref().map_or(SignsIn::ApiKey, |folder| {
                    SignsIn::Folder(Some(folder), std::path::Path::new(&shown))
                });
                if account_row(
                    ui,
                    &mut state.agents,
                    (&account.name, signs_in),
                    Some(index),
                ) {
                    forgotten = Some(index);
                }
            }
        });
        if let Some(index) = forgotten {
            let account = state.settings.agent_accounts.remove(index);
            if account.kind == crate::agents::AccountKind::ApiKey
                && let Some(git) = state.git.ok_ref().cloned()
            {
                crate::jobs::detached(move || {
                    if let Err(error) = egit_git::erase_credential(&git, None, account.place()) {
                        tracing::warn!(%error, "an API key couldn't be erased");
                    }
                });
            }
        }
        ui.add_space(8.0);
        add_account(ui, state);
    });
}

/// How an account signs in: with a Claude Code folder (`None`: Claude Code's own), shown as
/// written; or with an API key.
#[derive(Clone, Copy)]
enum SignsIn<'a> {
    Folder(Option<&'a std::path::Path>, &'a std::path::Path),
    ApiKey,
}

/// An account's row: its icon and name, what it signs in with and who is signed in there, and
/// for one added (`index`), Forget the account; returns whether that was clicked.
fn account_row(
    ui: &mut Ui,
    setup: &mut crate::agents::AgentsSetup,
    (name, signs_in): (&str, SignsIn<'_>),
    index: Option<usize>,
) -> bool {
    let palette = crate::theme::Palette::of(ui.ctx());
    let mut forget = false;
    ui.push_id(("agent-account", index), |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let glyph = match signs_in {
                        SignsIn::Folder(..) => crate::theme::icon::FOLDER_SIMPLE,
                        SignsIn::ApiKey => crate::theme::icon::KEY,
                    };
                    ui.label(RichText::new(glyph).color(palette.muted));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(name).strong());
                        let (said, color) = signed_in(setup, signs_in, &palette);
                        let said = RichText::new(said).size(crate::theme::SMALL).color(color);
                        ui.add(Label::new(said).wrap());
                    });
                    if index.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            forget = widgets::secondary(ui, true, "Forget the account")
                                .on_hover_text(
                                    "A folder stays where it is; an API key is erased from \
                                     git's credential helper",
                                )
                                .clicked();
                        });
                    }
                });
            });
    });
    forget
}

/// What an account signs in with, and who is signed in there (in the warning colour when
/// nobody is).
fn signed_in(
    setup: &mut crate::agents::AgentsSetup,
    signs_in: SignsIn<'_>,
    palette: &crate::theme::Palette,
) -> (String, egui::Color32) {
    let SignsIn::Folder(folder, shown) = signs_in else {
        let said = "An API key, kept by git's credential helper".to_owned();
        return (said, palette.muted);
    };
    let at = shown.display();
    match setup.who(folder) {
        Some(Ok(status)) if status.logged_in => {
            let who = status.email.as_deref().unwrap_or(&status.auth_method);
            (format!("{at} · signed in as {who}"), palette.muted)
        }
        Some(Ok(_)) => (
            format!("{at} · nobody signed in: sign in with Claude Code, from the agents' panel"),
            palette.warn,
        ),
        Some(Err(problem)) => (format!("{at} · {}", problem.title), palette.muted),
        None => (at.to_string(), palette.muted),
    }
}

/// The form adding an account.
fn add_account(ui: &mut Ui, state: &mut AppState) {
    let form = &mut state.agents.form;
    widgets::form_row(ui, LABEL, "Account's name", |ui| {
        ui.add(widgets::line_field(&mut form.name).desired_width(240.0))
    });
    widgets::fact(ui, LABEL, "Signs in with", |ui| {
        widgets::segmented(
            ui,
            &mut form.api_key,
            &[(false, "A Claude subscription"), (true, "An API key")],
        );
    });
    let name = form.name.trim().to_owned();
    if form.api_key {
        widgets::form_row(ui, LABEL, "API key", |ui| {
            ui.add(
                widgets::line_field(&mut *form.key)
                    .password(true)
                    .desired_width(240.0),
            )
        });
    } else {
        let hint = format!("~/.claude-{}", name.replace(char::is_whitespace, "-"));
        widgets::form_row(ui, LABEL, "Folder", |ui| {
            ui.add(
                widgets::line_field(&mut form.folder)
                    .hint_text(hint.as_str())
                    .desired_width(240.0),
            )
        });
    }
    let taken = name == "Claude Code's own"
        || state
            .settings
            .agent_accounts
            .iter()
            .any(|account| account.name == name);
    let fits = !name.is_empty() && !taken && !name.contains(char::is_control);
    let complete = fits && (!form.api_key || !form.key.trim().is_empty());
    let adding = state.agents.adding.is_pending();
    ui.horizontal(|ui| {
        let add = widgets::primary(ui, complete && !adding, "Add the account");
        let add = if taken {
            add.on_disabled_hover_text("Another account has that name")
        } else {
            add
        };
        if add.clicked() {
            let form = &mut state.agents.form;
            let kind = if form.api_key {
                crate::agents::AccountKind::ApiKey
            } else {
                let typed = form.folder.trim();
                let typed = if typed.is_empty() {
                    format!("~/.claude-{}", name.replace(char::is_whitespace, "-"))
                } else {
                    typed.to_owned()
                };
                let folder = super::home::expand_home(&typed, state.home_dir.as_deref());
                crate::agents::AccountKind::Folder(folder.as_path().into())
            };
            let key = std::mem::take(&mut form.key);
            let account = crate::agents::AgentAccount { name, kind };
            let git = state.git.ok_ref().cloned();
            state.agents.add_account(git.as_ref(), account, key);
        }
        if adding {
            ui.spinner();
        }
    });
    if let Some(problem) = state.agents.adding.err_ref().cloned() {
        widgets::problem_box(ui, &problem, |_| {});
    }
}

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
            let reset = widgets::secondary(ui, preferences.zoom != 100, "Reset");
            crate::help::note_topic(ui, reset.rect, "pref-text-size");
            if reset.clicked() {
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
        widgets::fact(ui, LABEL, "People", |ui| {
            widgets::segmented(
                ui,
                &mut preferences.pictures,
                &[(true, "Their pictures"), (false, "Their initials")],
            );
        });
        ui.add(
            Label::new(muted(
                ui,
                "Pictures come from GitHub (the account each author's email belongs to), else \
                 Gravatar: GitHub is sent the email, Gravatar its hash. They are kept on this \
                 computer.",
            ))
            .wrap(),
        );
    });
}

/// How repositories are kept up to date, and laid out.
fn repositories(ui: &mut Ui, preferences: &mut Preferences) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Repositories");
        ui.add_space(8.0);
        widgets::form_row(ui, LABEL, "Fetch in the background", |ui| {
            let combo = ComboBox::from_id_salt("preferences-fetch")
                .icon(crate::widgets::combo_caret)
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
        widgets::fact(ui, LABEL, "Output", |ui| {
            widgets::checkbox(
                ui,
                &mut preferences.output,
                "Opens while hooks run, and when a task fails",
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
                .icon(crate::widgets::combo_caret)
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
