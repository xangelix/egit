//! The Home tab: open a repository, clone one, reopen a recent one or one suggested (beside two
//! you've opened: `crate::suggested`), and see who you are to git.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Key, RichText, Ui};
use egui_async::Bind;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::layout::Column;
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use super::clone::{self, CloneView};
use crate::app::{Action, AppState};
use crate::format::now;
use crate::problem::{OpenProblem, Problem, Remedy};
use crate::settings::{Location, StoredPath};
use crate::suggested::{Suggestion, suggest};
use crate::tables::{RecentRows, SuggestedRows};
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// How tall a table row is.
const ROW_HEIGHT: f32 = 24.0;

/// The Home tab's own state.
pub(crate) struct HomeView {
    /// The path typed into the open field.
    pub(crate) path: String,
    /// Another computer to open a repository on (as ssh reaches it), and the folder there.
    pub(crate) there_host: String,
    pub(crate) there_path: String,
    /// Why the last repository couldn't be opened.
    pub(crate) problem: Option<OpenProblem>,
    /// The folder dialog, while it is open.
    pub(crate) picking: Bind<Option<PathBuf>, Problem>,
    /// Trusting a folder another user owns, before opening it.
    /// A folder trusted or made a repository, to open once done.
    pub(crate) preparing: Bind<crate::settings::Location, Problem>,
    /// Cloning a repository.
    pub(crate) clone: CloneView,
    recent: RecentTable,
    suggested: Suggested,
}

/// How long the repositories suggested stand before they are worked out again (while Home
/// shows): which was last active changes as one works.
const SUGGESTED_LASTS: Duration = Duration::from_secs(600);

/// The repositories suggested: worked out (in a job) from the repositories known then, when,
/// and their table's state (starting over with each list).
struct Suggested {
    known: Vec<PathBuf>,
    found: Bind<Arc<[Suggestion]>, Problem>,
    at: Option<Instant>,
    shown: usize,
    state: TableState,
}

impl Default for Suggested {
    fn default() -> Self {
        Self {
            known: Vec::new(),
            found: Bind::new(true),
            at: None,
            shown: 0,
            state: TableState::new("home-suggested", 0),
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
    /// The Home tab, its clones going into `clone_folder` (or the home folder); `host`, the
    /// other computer a repository was last opened on, offered for the next.
    pub(crate) fn new(
        clone_folder: Option<PathBuf>,
        home: Option<&Path>,
        host: Option<&str>,
    ) -> Self {
        Self {
            path: String::new(),
            there_host: host.unwrap_or_default().to_owned(),
            there_path: String::new(),
            problem: None,
            picking: Bind::new(true),
            preparing: Bind::new(true),
            clone: CloneView::new(clone_folder, home, host),
            recent: RecentTable::default(),
            suggested: Suggested::default(),
        }
    }

    /// The repositories suggested, by name, once worked out.
    #[cfg(test)]
    pub(crate) fn suggested_names(&mut self) -> Option<Vec<String>> {
        let found = self.suggested.found.ok_ref()?;
        Some(
            found
                .iter()
                .filter_map(|suggestion| suggestion.path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect(),
        )
    }

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
        widgets::page_title(ui, "egit");
        ui.label(muted(ui, "A fast, native git client."));
        ui.add_space(16.0);
        git_problem(ui, state);
        // What is used every day first: who commits, what to open; cloning, now and then.
        profile_card(ui, state);
        ui.add_space(12.0);
        open_card(ui, state);
        ui.add_space(12.0);
        recent_card(ui, state);
        suggested_card(ui, state);
        ui.add_space(12.0);
        let ready = state.git.is_ok();
        clone::card(
            ui,
            &mut state.home.clone,
            ready,
            state.home_dir.as_deref(),
            &mut state.actions,
        );
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
        let again = widgets::primary(ui, true, "Try again");
        crate::help::note_topic(ui, again.rect, "home-git");
        if again.clicked() {
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
        let heading = widgets::heading(ui, "Open a repository");
        ui.label(muted(
            ui,
            "Any folder inside a repository opens the whole repository; one outside any can \
             be made a repository.",
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
            let field = ui
                .add(
                    crate::widgets::line_field(&mut state.home.path)
                        .hint_text("…or type a path")
                        .desired_width((ui.available_width() - 72.0).max(120.0)),
                )
                .labelled_by(heading.id);
            let typed = state.home.path.trim();
            let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            let clicked = widgets::secondary(ui, ready && !typed.is_empty(), "Open").clicked();
            if (clicked || submitted) && ready && !typed.is_empty() {
                let path = expand_home(typed, state.home_dir.as_deref());
                state.actions.push(Action::Open(Location::here(path)));
            }
        });
        // Not from Windows: its OpenSSH can't share one connection (each command would log in
        // again), and its paths join with `\`, which the other computer's don't.
        if !cfg!(windows) {
            ui.add_space(6.0);
            open_there(ui, state, ready);
        }
        if state.home.picking.is_pending() {
            widgets::working(ui, "Waiting for the folder dialog…");
        }
        if state.is_opening() || state.home.preparing.is_pending() {
            widgets::working(ui, "Opening…");
        }
        if let Some(problem) = state.home.problem.clone() {
            ui.add_space(8.0);
            widgets::problem_box(ui, &problem.problem, |ui| {
                let busy = state.home.preparing.is_pending();
                let remedy = problem.remedy.as_ref().map(|remedy| match remedy {
                    Remedy::Trust(location) => (
                        "Trust this folder",
                        format!(
                            "Adds {} to git's safe.directory setting",
                            location.path.display()
                        ),
                        Action::Trust(location.clone()),
                    ),
                    Remedy::Create(location) => (
                        "Create a repository here",
                        format!(
                            "Runs git init in {}: nothing in it changes, and nothing is \
                             committed yet",
                            location.path.display()
                        ),
                        Action::Init(location.clone()),
                    ),
                });
                if let Some((words, hover, action)) = remedy
                    && widgets::primary(ui, ready && !busy, words)
                        .on_hover_text(hover)
                        .clicked()
                {
                    state.actions.push(action);
                }
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    state.home.problem = None;
                }
            });
        }
    });
}

/// Opening a repository on another computer, on one row: which (as ssh reaches it), the
/// folder there, and Open there.
fn open_there(ui: &mut Ui, state: &mut AppState, ready: bool) {
    let mut submitted = false;
    let home = &mut state.home;
    ui.horizontal(|ui| {
        let label = ui.label(muted(ui, "On another computer"));
        crate::help::note(ui, label.rect, "On another computer");
        let room = (ui.available_width() - 260.0).max(240.0);
        ui.add(
            widgets::line_field(&mut home.there_host)
                .hint_text("host, user@host, or an ssh alias")
                .desired_width(room * 0.45),
        )
        .labelled_by(label.id);
        let folder = ui.label(muted(ui, "folder"));
        crate::help::note(ui, folder.rect, "folder");
        let field = ui
            .add(
                widgets::line_field(&mut home.there_path)
                    .hint_text("~/project")
                    .desired_width(room * 0.55),
            )
            .labelled_by(folder.id);
        submitted = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        let (host, path) = (home.there_host.trim(), home.there_path.trim());
        let complete = ready && !host.is_empty() && !path.is_empty();
        let clicked = widgets::secondary(ui, complete, &format!("{}  Open there", icon::GLOBE))
            .on_hover_text("egit runs git there over one shared SSH connection")
            .clicked();
        if (clicked || submitted) && complete {
            state.actions.push(Action::Open(Location {
                host: Some(host.to_owned()),
                path: PathBuf::from(path),
            }));
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
        let recent = &state.settings.recent;
        let unchanged = table.shown.len() == recent.len()
            && table
                .shown
                .iter()
                .zip(recent)
                .all(|(shown, recent)| *shown == recent.path);
        if !unchanged {
            table.shown = recent.iter().map(|recent| recent.path.clone()).collect();
            table.state = TableState::new("home-recent", recent.len());
        }
        let provider = RecentRows::new(&state.settings.recent, state.home_dir.as_deref(), now());
        let rows = state.settings.recent.len().min(10);
        let height = ROW_HEIGHT * (f32::from(u8::try_from(rows).unwrap_or(10)) + 1.0) + 4.0;
        let name = 200.0;
        let opened = 140.0;
        let location = (ui.available_width() - name - opened).max(160.0);
        let table_kit = TableKit::new("home-recent", &provider, &mut table.state);
        let output = crate::tables::styled(table_kit, ui)
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
                state.actions.push(Action::Open(recent.location()));
            }
        }
        let selected: Option<Location> = table
            .state
            .selected_rows
            .min()
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| state.settings.recent.get(index))
            .map(crate::settings::Recent::location);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = state.git.is_ok();
            if widgets::secondary(
                ui,
                ready && selected.is_some(),
                &format!("{}  Open", icon::FOLDER_OPEN),
            )
            .clicked()
                && let Some(location) = &selected
            {
                state.actions.push(Action::Open(location.clone()));
            }
            if widgets::secondary(
                ui,
                selected.is_some(),
                &format!("{}  Remove from list", icon::X),
            )
            .clicked()
                && let Some(location) = selected
            {
                state.actions.push(Action::Forget(location));
            }
            ui.label(muted(ui, "Double-click a repository to open it.").small());
        });
    });
}

/// The repositories suggested, under the recent ones: those beside two egit knows (open, or
/// recent), the most recently active first; nothing while there are none.
fn suggested_card(ui: &mut Ui, state: &mut AppState) {
    let Some(found) = follow_suggested(state).filter(|found| !found.is_empty()) else {
        return;
    };
    let identity = Arc::as_ptr(&found).cast::<u8>().addr();
    if state.home.suggested.shown != identity {
        state.home.suggested.shown = identity;
        state.home.suggested.state = TableState::new("home-suggested", found.len());
    }
    let home = state.home_dir.clone();
    let ready = state.git.is_ok();
    let suggested = &mut state.home.suggested;
    let actions = &mut state.actions;
    suggested_table(ui, (&found, home.as_deref(), ready), suggested, actions);
}

/// The repositories suggested as last worked out, working them out again (in a job) when the
/// repositories known (open, or recent, on this computer) changed, or a while has passed.
fn follow_suggested(state: &mut AppState) -> Option<Arc<[Suggestion]>> {
    let mut known: Vec<PathBuf> = state
        .settings
        .recent
        .iter()
        .filter(|recent| recent.host.is_none())
        .map(|recent| recent.path.to_path())
        .chain(
            state
                .repos
                .iter()
                .filter(|repo| repo.git.host().is_local())
                .map(|repo| repo.repository.work_tree().to_owned()),
        )
        .collect();
    known.sort();
    known.dedup();
    let git = state.git();
    let suggested = &mut state.home.suggested;
    let due = suggested.at.is_none_or(|at| at.elapsed() > SUGGESTED_LASTS);
    if (suggested.known != known || due)
        && !suggested.found.is_pending()
        && let Some(git) = git
    {
        let asked = known.clone();
        suggested.found.request(crate::jobs::blocking(async move {
            Ok(Arc::from(suggest(&git, &asked)))
        }));
        suggested.known = known;
        suggested.at = Some(Instant::now());
    }
    suggested
        .found
        .read_as_ref()
        .and_then(|read| read.ok().map(Arc::clone))
}

/// The table of `found` (`home`: the home folder, for `~`; `ready`: git is), Open, and how to.
fn suggested_table(
    ui: &mut Ui,
    (found, home, ready): (&Arc<[Suggestion]>, Option<&Path>, bool),
    suggested: &mut Suggested,
    actions: &mut Vec<Action>,
) {
    ui.add_space(12.0);
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Suggested repositories");
        ui.label(muted(
            ui,
            "Beside repositories you've opened, in the same folders: the most recently active \
             first.",
        ));
        ui.add_space(4.0);
        let provider = SuggestedRows::new(found, home, now());
        let rows = found.len().min(10);
        let height = ROW_HEIGHT * (f32::from(u8::try_from(rows).unwrap_or(10)) + 1.0) + 4.0;
        let (name, active) = (200.0, 140.0);
        let location = (ui.available_width() - name - active).max(160.0);
        // Its own ids: the recent table's are taken.
        let output = ui
            .push_id("home-suggested", |ui| {
                let table_kit = TableKit::new("home-suggested", &provider, &mut suggested.state);
                crate::tables::styled(table_kit, ui)
                    .with_columns(vec![
                        Column::new(name).range(120.0..=400.0),
                        Column::new(location)
                            .range(120.0..=f32::INFINITY)
                            .remainder(true),
                        Column::new(active).range(80.0..=240.0),
                    ])
                    .with_row_height(ROW_HEIGHT)
                    .with_max_height(Some(height))
                    .show_with_output(ui, |_, _, _, _| None)
            })
            .inner;
        let Ok(output) = output else {
            return;
        };
        for event in &output.events {
            if event.kind == RowEventKind::Activated
                && let Some(chosen) = found.get(event.row_index)
            {
                actions.push(Action::Open(Location::here(chosen.path.clone())));
            }
        }
        let selected = suggested
            .state
            .selected_rows
            .min()
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| found.get(index));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if widgets::secondary(
                ui,
                ready && selected.is_some(),
                &format!("{}  Open", icon::FOLDER_OPEN),
            )
            .clicked()
                && let Some(chosen) = selected
            {
                actions.push(Action::Open(Location::here(chosen.path.clone())));
            }
            ui.label(muted(ui, "Double-click a repository to open it.").small());
        });
    });
}

/// Who commits, on one line: the name and email, whether commits are signed, and the profile's
/// button at the right.
fn profile_card(ui: &mut Ui, state: &mut AppState) {
    let palette = Palette::of(ui.ctx());
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            // Two lines tall however it stands (being read, read, failed), so the cards below
            // don't move when the settings land.
            let line = ui.text_style_height(&egui::TextStyle::Body);
            ui.set_min_height(2.0f32.mul_add(line, 2.0));
            ui.label(
                RichText::new(icon::USER_CIRCLE)
                    .size(20.0)
                    .color(palette.muted),
            );
            match state.user_config.read_as_ref() {
                None => widgets::working(ui, "Reading your git settings…"),
                Some(Err(problem)) => {
                    ui.label(RichText::new(problem.title.as_str()).color(palette.danger));
                }
                Some(Ok(snapshot)) => {
                    let profile = snapshot.profile();
                    let name = profile.name().map(|entry| entry.text().to_string());
                    let email = profile.email().map(|entry| entry.text().to_string());
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        match (name, email) {
                            (Some(name), Some(email)) => {
                                ui.label(RichText::new(format!("{name} <{email}>")).medium());
                            }
                            _ => {
                                ui.label(
                                    RichText::new(format!(
                                        "{}  Your name or email isn't set: git won't let you \
                                         commit.",
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
                    });
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        });
    });
}

/// A folder typed for another computer, as a shell there reads it: `~` its `home` folder,
/// and a path that doesn't start at the root inside it (where ssh starts).
pub(crate) fn folder_there(typed: &Path, home: &Path) -> PathBuf {
    let expanded = typed
        .to_str()
        .map_or_else(|| typed.to_owned(), |text| expand_home(text, Some(home)));
    if expanded.has_root() {
        expanded
    } else {
        home.join(expanded)
    }
}

/// A typed path with a leading `~` meaning the home folder.
pub(crate) fn expand_home(typed: &str, home: Option<&Path>) -> PathBuf {
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

    #[test]
    fn a_folder_typed_for_another_computer_reads_as_a_shell_there_reads_it() {
        let there = |typed: &str| folder_there(Path::new(typed), Path::new("/home/me"));
        assert_eq!(there("~"), PathBuf::from("/home/me"));
        assert_eq!(there("~/src/egit"), PathBuf::from("/home/me/src/egit"));
        assert_eq!(there("src/egit"), PathBuf::from("/home/me/src/egit"));
        assert_eq!(there("/srv/egit"), PathBuf::from("/srv/egit"));
        // Someone else's home isn't looked up: a folder of that name in this one.
        assert_eq!(there("~ada/x"), PathBuf::from("/home/me/~ada/x"));
    }
}
