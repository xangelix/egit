//! The Home tab: open a repository, reopen a recent one, and see who you are to git.

use std::path::{Path, PathBuf};

use egui::{Key, RichText, TextEdit, Ui};
use egui_async::Bind;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::layout::Column;
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use crate::app::{Action, AppState};
use crate::format::now;
use crate::problem::{OpenProblem, Problem};
use crate::settings::StoredPath;
use crate::tables::RecentRows;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// How tall a table row is.
const ROW_HEIGHT: f32 = 24.0;

/// The Home tab's own state.
pub(crate) struct HomeView {
    /// The path typed into the open field.
    pub(crate) path: String,
    /// Why the last repository couldn't be opened.
    pub(crate) problem: Option<OpenProblem>,
    /// The folder dialog, while it is open.
    pub(crate) picking: Bind<Option<PathBuf>, Problem>,
    /// Trusting a folder another user owns, before opening it.
    pub(crate) trusting: Bind<PathBuf, Problem>,
    recent: RecentTable,
}

impl Default for HomeView {
    fn default() -> Self {
        Self {
            path: String::new(),
            problem: None,
            picking: Bind::new(true),
            trusting: Bind::new(true),
            recent: RecentTable::default(),
        }
    }
}

/// The recent repositories' table; its state starts over when the list changes.
struct RecentTable {
    shown: Vec<StoredPath>,
    state: TableState,
}

impl Default for RecentTable {
    fn default() -> Self {
        Self {
            shown: Vec::new(),
            state: TableState::new("home-recent", 0),
        }
    }
}

impl HomeView {
    /// Opens the desktop's folder dialog.
    pub(crate) fn pick_folder(&mut self) {
        if self.picking.is_pending() {
            return;
        }
        self.picking.request(async {
            let folder = rfd::AsyncFileDialog::new()
                .set_title("Open a repository")
                .pick_folder()
                .await;
            Ok(folder.map(|folder| folder.path().to_owned()))
        });
    }
}

/// Draws the Home tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    super::page(ui, "home", |ui| {
        ui.label(RichText::new("egit").size(30.0).strong());
        ui.label(muted(ui, "A fast, native git client."));
        ui.add_space(16.0);
        git_problem(ui, state);
        open_card(ui, state);
        ui.add_space(12.0);
        recent_card(ui, state);
        ui.add_space(12.0);
        profile_card(ui, state);
    });
}

/// Why git can't be used, when it can't.
fn git_problem(ui: &mut Ui, state: &mut AppState) {
    if state.git.is_pending() {
        widgets::working(ui, "Looking for git…");
        ui.add_space(12.0);
        return;
    }
    let Some(problem) = state.git.err_ref().cloned() else {
        return;
    };
    widgets::problem_box(ui, &problem, |ui| {
        if widgets::primary(ui, true, "Try again").clicked() {
            state.actions.push(Action::DetectGit);
        }
        ui.label(muted(
            ui,
            "egit needs git 2.43.0 or newer (on Ubuntu: sudo apt install git).",
        ));
    });
    ui.add_space(12.0);
}

fn open_card(ui: &mut Ui, state: &mut AppState) {
    let ready = state.git.is_ok();
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Open a repository");
        ui.label(muted(
            ui,
            "Any folder inside a repository opens the whole repository.",
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let picking = state.home.picking.is_pending();
            if widgets::primary(
                ui,
                ready && !picking,
                &format!("{}  Open folder…", icon::FOLDER_OPEN),
            )
            .clicked()
            {
                state.actions.push(Action::PickFolder);
            }
            let field = ui.add(
                TextEdit::singleline(&mut state.home.path)
                    .hint_text("…or type a path")
                    .desired_width((ui.available_width() - 72.0).max(120.0)),
            );
            let typed = state.home.path.trim();
            let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            let clicked = widgets::secondary(ui, ready && !typed.is_empty(), "Open").clicked();
            if (clicked || submitted) && ready && !typed.is_empty() {
                let path = expand_home(typed, state.home_dir.as_deref());
                state.actions.push(Action::Open(path));
            }
        });
        if state.home.picking.is_pending() {
            widgets::working(ui, "Waiting for the folder dialog…");
        }
        if state.is_opening() || state.home.trusting.is_pending() {
            widgets::working(ui, "Opening…");
        }
        if let Some(problem) = state.home.problem.clone() {
            ui.add_space(8.0);
            widgets::problem_box(ui, &problem.problem, |ui| {
                if let Some(path) = &problem.trust
                    && widgets::primary(ui, ready, "Trust this folder")
                        .on_hover_text(format!(
                            "Adds {} to git's safe.directory setting",
                            path.display()
                        ))
                        .clicked()
                {
                    state.actions.push(Action::Trust(path.clone()));
                }
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    state.home.problem = None;
                }
            });
        }
    });
}

fn recent_card(ui: &mut Ui, state: &mut AppState) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Recent repositories");
        if state.settings.recent.is_empty() {
            ui.label(muted(ui, "Repositories you open appear here."));
            return;
        }
        let table = &mut state.home.recent;
        let listed: Vec<StoredPath> = state
            .settings
            .recent
            .iter()
            .map(|recent| recent.path.clone())
            .collect();
        if table.shown != listed {
            table.state = TableState::new("home-recent", listed.len());
            table.shown = listed;
        }
        let provider = RecentRows::new(&state.settings.recent, state.home_dir.as_deref(), now());
        let rows = state.settings.recent.len().min(10);
        let height = ROW_HEIGHT * (f32::from(u8::try_from(rows).unwrap_or(10)) + 1.0) + 4.0;
        let name = 200.0;
        let opened = 140.0;
        let location = (ui.available_width() - name - opened).max(160.0);
        let output = TableKit::new("home-recent", &provider, &mut table.state)
            .with_columns(vec![
                Column::new(name).range(120.0..=400.0),
                Column::new(location)
                    .range(120.0..=f32::INFINITY)
                    .remainder(true),
                Column::new(opened).range(80.0..=240.0),
            ])
            .with_row_height(ROW_HEIGHT)
            .with_max_height(Some(height))
            .show_with_output(ui, |_, _, _, _| None);
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                ui.label(
                    RichText::new(format!("The list couldn't be drawn: {error}"))
                        .color(Palette::of(ui.ctx()).danger),
                );
                return;
            }
        };
        for event in &output.events {
            if event.kind == RowEventKind::Activated
                && let Some(recent) = state.settings.recent.get(event.row_index)
            {
                state.actions.push(Action::Open(recent.path.to_path()));
            }
        }
        let selected: Option<PathBuf> = table
            .state
            .selected_rows
            .min()
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| state.settings.recent.get(index))
            .map(|recent| recent.path.to_path());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = state.git.is_ok();
            if widgets::secondary(
                ui,
                ready && selected.is_some(),
                &format!("{}  Open", icon::FOLDER_OPEN),
            )
            .clicked()
                && let Some(path) = &selected
            {
                state.actions.push(Action::Open(path.clone()));
            }
            if widgets::secondary(
                ui,
                selected.is_some(),
                &format!("{}  Remove from list", icon::X),
            )
            .clicked()
                && let Some(path) = selected
            {
                state.actions.push(Action::Forget(path));
            }
            ui.label(muted(ui, "Double-click a repository to open it.").small());
        });
    });
}

fn profile_card(ui: &mut Ui, state: &mut AppState) {
    let palette = Palette::of(ui.ctx());
    widgets::card(ui, |ui| {
        widgets::heading(ui, "You");
        match state.user_config.read_as_ref() {
            None => widgets::working(ui, "Reading your git settings…"),
            Some(Err(problem)) => widgets::problem_box(ui, problem, |_| {}),
            Some(Ok(snapshot)) => {
                let profile = snapshot.profile();
                let name = profile.name().map(|entry| entry.text().to_string());
                let email = profile.email().map(|entry| entry.text().to_string());
                match (name, email) {
                    (Some(name), Some(email)) => {
                        ui.label(RichText::new(format!("{name} <{email}>")).strong());
                    }
                    _ => {
                        ui.label(
                            RichText::new(format!(
                                "{}  Your name or email isn't set: git won't let you commit.",
                                icon::WARNING
                            ))
                            .color(palette.warn),
                        );
                    }
                }
                let signing = profile.signing();
                let signs = if signing.commits.value {
                    format!("Commits are signed ({}).", signing.format.value.name())
                } else {
                    "Commits aren't signed by default.".to_owned()
                };
                ui.label(muted(ui, signs));
            }
        }
        ui.add_space(6.0);
        if widgets::secondary(
            ui,
            true,
            &format!("{}  Edit your profile", icon::USER_CIRCLE),
        )
        .clicked()
        {
            state.actions.push(Action::ShowProfile);
        }
    });
}

/// A typed path with a leading `~` meaning the home folder.
fn expand_home(typed: &str, home: Option<&Path>) -> PathBuf {
    match (typed.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_owned(),
        (Some(rest), Some(home)) if rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(typed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_tilde_is_the_home_folder() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(expand_home("~", home), PathBuf::from("/home/me"));
        assert_eq!(
            expand_home("~/src/egit", home),
            PathBuf::from("/home/me/src/egit")
        );
        assert_eq!(expand_home("~other/x", home), PathBuf::from("~other/x"));
        assert_eq!(expand_home("/srv/x", home), PathBuf::from("/srv/x"));
        assert_eq!(expand_home("~/x", None), PathBuf::from("~/x"));
    }
}
