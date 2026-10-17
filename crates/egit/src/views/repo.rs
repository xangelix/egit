//! A repository's tab: its overview (branch, commit, upstream, what is in progress, who you are
//! here, where it lives) and every setting in effect in it.

use std::path::Path;
use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{Committed, ConfigSnapshot, Git, Head, Operation, Repository, Stash, Tracking};
use egui::{Color32, Grid, RichText, Ui, WidgetText};
use egui_async::Bind;
use egui_tiles::{TileId, Tiles, UiResponse};

use crate::app::{Action, AppState};
use crate::forge::RepoForge;
use crate::format::display_path;
use crate::history::HistoryState;
use crate::problem::Problem;
use crate::repo::{RepoId, RepoPanel, Status};
use crate::tables::SnapshotTable;
use crate::tasks::Tasks;
use crate::theme::{Palette, icon};
use crate::views::history::{HistoryPane, HistoryView};
use crate::widgets::{self, muted};
use crate::worktree::WorkTree;

/// Draws repository `id`'s tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState, id: RepoId) {
    let AppState {
        git,
        repos,
        actions,
        home_dir,
        ..
    } = state;
    let git = git.ok_ref();
    let Some(repo) = repos.iter_mut().find(|repo| repo.id == id) else {
        ui.label(muted(ui, "This repository was closed."));
        return;
    };
    let mut behavior = Panels {
        id,
        git,
        title: &repo.title,
        repository: &repo.repository,
        status: &mut repo.status,
        config: &mut repo.config,
        config_table: &mut repo.config_table,
        history: &mut repo.history,
        history_view: &mut repo.history_view,
        work_tree: &mut repo.work_tree,
        staging: &mut repo.staging,
        committing: &mut repo.committing,
        commits_made: repo.commits_made,
        tasks: &mut repo.tasks,
        stashes: &mut repo.stashes,
        writing_commit_graph: &mut repo.writing_commit_graph,
        forge: &repo.forge,
        home: home_dir.as_deref(),
        actions,
    };
    repo.panels.ui(&mut behavior, ui);
}

/// What a repository's panels share while they are drawn.
struct Panels<'a> {
    id: RepoId,
    git: Option<&'a Git>,
    title: &'a str,
    repository: &'a Arc<Repository>,
    status: &'a mut Bind<Status, Problem>,
    config: &'a mut Bind<Arc<ConfigSnapshot>, Problem>,
    config_table: &'a mut SnapshotTable,
    history: &'a mut HistoryState,
    history_view: &'a mut HistoryView,
    work_tree: &'a mut WorkTree,
    staging: &'a mut Bind<(), Problem>,
    committing: &'a mut Bind<Committed, Problem>,
    commits_made: u64,
    tasks: &'a mut Tasks,
    stashes: &'a mut Bind<Arc<[Stash]>, Problem>,
    writing_commit_graph: &'a mut Bind<(), Problem>,
    forge: &'a RepoForge,
    home: Option<&'a Path>,
    actions: &'a mut Vec<Action>,
}

impl egui_tiles::Behavior<RepoPanel> for Panels<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, panel: &mut RepoPanel) -> UiResponse {
        match panel {
            RepoPanel::History => {
                let status = self.status.ok_ref();
                let config = self.config.ok_ref().map(|config| &**config);
                super::history::show(
                    ui,
                    HistoryPane {
                        id: self.id,
                        git: self.git,
                        repository: self.repository,
                        history: self.history,
                        view: self.history_view,
                        head: status.map(|status| &status.head),
                        writing_commit_graph: self.writing_commit_graph,
                        partial_clone: config.map(ConfigSnapshot::is_partial_clone),
                        work_tree: self.work_tree,
                        staging: self.staging,
                        committing: self.committing,
                        commits_made: self.commits_made,
                        sign_by_default: config
                            .and_then(|config| config.get("commit.gpgSign"))
                            .and_then(egit_git::ConfigEntry::bool)
                            .unwrap_or(false),
                        operations: status.map_or(&[], |status| status.operations.as_slice()),
                        applying: status.and_then(|status| status.applying.as_ref()),
                        config,
                        tasks: self.tasks,
                        stashes: self.stashes.ok_ref().map_or(&[], |stashes| &**stashes),
                        forge: self.forge,
                        home: self.home,
                        actions: self.actions,
                    },
                );
            }
            RepoPanel::Overview => overview(ui, self),
            RepoPanel::Configuration => configuration(ui, self),
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, panel: &RepoPanel) -> WidgetText {
        match panel {
            RepoPanel::History => format!("{}  History", icon::GIT_COMMIT).into(),
            RepoPanel::Overview => format!("{}  Overview", icon::INFO).into(),
            RepoPanel::Configuration => format!("{}  Settings", icon::SLIDERS_HORIZONTAL).into(),
        }
    }

    fn is_tab_closable(&self, _tiles: &Tiles<RepoPanel>, _tile_id: TileId) -> bool {
        false
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..egui_tiles::SimplificationOptions::default()
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        30.0
    }

    fn tab_bar_color(&self, visuals: &egui::Visuals) -> Color32 {
        if visuals.dark_mode {
            Palette::DARK.background
        } else {
            Palette::LIGHT.background
        }
    }
}

fn overview(ui: &mut Ui, panels: &mut Panels<'_>) {
    let palette = Palette::of(ui.ctx());
    super::page(ui, ("repo-overview", panels.id), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(panels.title).size(26.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let busy = panels.status.is_pending() || panels.config.is_pending();
                if widgets::secondary(ui, !busy, &format!("{}  Refresh", icon::ARROWS_CLOCKWISE))
                    .clicked()
                {
                    panels.actions.push(Action::Refresh(panels.id));
                }
                if busy {
                    ui.spinner();
                }
            });
        });
        ui.label(muted(
            ui,
            display_path(panels.repository.work_tree(), panels.home),
        ));
        ui.add_space(16.0);

        match panels.status.read_as_ref() {
            None => widgets::working(ui, "Reading the repository…"),
            Some(Err(problem)) => {
                let problem = problem.clone();
                widgets::problem_box(ui, &problem, |_| {});
            }
            Some(Ok(status)) => {
                operations_banner(ui, &status.operations, palette);
                branch_card(ui, &status.head, palette);
            }
        }
        ui.add_space(12.0);
        identity_card(ui, panels);
        ui.add_space(12.0);
        location_card(ui, panels.repository, panels.home);
    });
}

/// A warning for each operation git stopped in the middle of.
fn operations_banner(ui: &mut Ui, operations: &[Operation], palette: Palette) {
    for operation in operations {
        let text = match operation {
            Operation::Merge => {
                "A merge is in progress: resolve its conflicts, then commit, or abort it."
            }
            Operation::Rebase => {
                "A rebase is in progress: resolve its conflicts and continue, or abort it."
            }
            Operation::ApplyMailbox => "Applying patches (git am) is in progress.",
            Operation::CherryPick => {
                "A cherry-pick is in progress: resolve its conflicts, then continue, or abort it."
            }
            Operation::Revert => {
                "A revert is in progress: resolve its conflicts, then continue, or abort it."
            }
            Operation::Bisect => "A bisect is in progress.",
        };
        egui::Frame::new()
            .fill(palette.warn.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, palette.warn.gamma_multiply(0.6)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("{}  {text}", icon::WARNING)).color(palette.warn));
            });
        ui.add_space(8.0);
    }
}

fn branch_card(ui: &mut Ui, head: &Head, palette: Palette) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Branch");
        ui.add_space(4.0);
        Grid::new("repo-branch")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| match head {
                Head::Branch(branch) => {
                    ui.label(muted(ui, "On"));
                    ui.label(
                        RichText::new(format!("{}  {}", icon::GIT_BRANCH, branch.name)).strong(),
                    );
                    ui.end_row();

                    ui.label(muted(ui, "Commit"));
                    match &branch.commit {
                        Some(commit) => commit_label(
                            ui,
                            &commit.id.abbreviated(10).to_string(),
                            &commit.subject.to_str_lossy(),
                        ),
                        None => {
                            ui.label(muted(ui, "No commits yet"));
                        }
                    }
                    ui.end_row();

                    ui.label(muted(ui, "Upstream"));
                    match &branch.upstream {
                        None => {
                            ui.label(muted(ui, "None: this branch doesn't track another"));
                        }
                        Some(upstream) => {
                            ui.horizontal(|ui| {
                                ui.label(upstream.name.to_str_lossy());
                                tracking_label(ui, upstream.tracking, palette);
                            });
                        }
                    }
                    ui.end_row();
                }
                Head::Detached(commit) => {
                    ui.label(muted(ui, "Detached at"));
                    commit_label(
                        ui,
                        &commit.id.abbreviated(10).to_string(),
                        &commit.subject.to_str_lossy(),
                    );
                    ui.end_row();
                }
            });
    });
}

fn commit_label(ui: &mut Ui, id: &str, subject: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(id).monospace());
        ui.label(subject);
    });
}

fn tracking_label(ui: &mut Ui, tracking: Tracking, palette: Palette) {
    let (text, color) = match tracking {
        Tracking::Gone => (
            format!("{}  gone from the remote", icon::WARNING),
            palette.warn,
        ),
        Tracking::Diverged {
            ahead: 0,
            behind: 0,
        } => (format!("{}  up to date", icon::CHECK), palette.ok),
        Tracking::Diverged { ahead, behind } => {
            let mut parts = Vec::new();
            if ahead > 0 {
                parts.push(format!("{}{ahead} to push", icon::ARROW_UP));
            }
            if behind > 0 {
                parts.push(format!("{}{behind} to pull", icon::ARROW_DOWN));
            }
            (parts.join("  "), palette.text)
        }
    };
    ui.label(RichText::new(text).color(color));
}

fn identity_card(ui: &mut Ui, panels: &mut Panels<'_>) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "You, here");
        match panels.config.read_as_ref() {
            None => widgets::working(ui, "Reading the repository's settings…"),
            Some(Err(problem)) => {
                let problem = problem.clone();
                widgets::problem_box(ui, &problem, |_| {});
            }
            Some(Ok(snapshot)) => {
                let profile = snapshot.profile();
                ui.label(muted(
                    ui,
                    "What commits made in this repository record, and where it is set.",
                ));
                ui.add_space(4.0);
                Grid::new(("repo-identity", panels.id))
                    .num_columns(3)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        for (label, entry) in [("Name", profile.name()), ("Email", profile.email())]
                        {
                            ui.label(muted(ui, label));
                            match entry {
                                Some(entry) => {
                                    ui.label(entry.text().to_string());
                                }
                                None => {
                                    ui.label(
                                        RichText::new("Not set").color(Palette::of(ui.ctx()).warn),
                                    );
                                }
                            }
                            widgets::origin(ui, entry, panels.home);
                            ui.end_row();
                        }
                        let signing = profile.signing();
                        ui.label(muted(ui, "Signing"));
                        ui.label(if signing.commits.value {
                            format!("Every commit ({})", signing.format.value.name())
                        } else {
                            "Off".to_owned()
                        });
                        widgets::origin(ui, signing.commits.entry, panels.home);
                        ui.end_row();
                    });
            }
        }
    });
}

fn location_card(ui: &mut Ui, repository: &Repository, home: Option<&Path>) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Where it lives");
        Grid::new(("repo-location", repository.work_tree()))
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label(muted(ui, "Work tree"));
                ui.label(display_path(repository.work_tree(), home));
                ui.end_row();
                ui.label(muted(ui, "Git directory"));
                ui.label(display_path(repository.git_dir(), home));
                ui.end_row();
                if repository.is_linked_worktree() {
                    ui.label(muted(ui, "Shared with"));
                    ui.label(display_path(repository.common_dir(), home));
                    ui.end_row();
                }
                ui.label(muted(ui, "Object names"));
                ui.label(
                    repository
                        .object_format()
                        .name()
                        .to_uppercase()
                        .replace("SHA", "SHA-"),
                );
                ui.end_row();
            });
    });
}

fn configuration(ui: &mut Ui, panels: &mut Panels<'_>) {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.label(muted(
                ui,
                "Every setting in effect in this repository, in the order git reads them: for a \
                 key set more than once, the last value applies.",
            ));
            ui.add_space(8.0);
            match panels.config.read_as_ref() {
                None => widgets::working(ui, "Reading the repository's settings…"),
                Some(Err(problem)) => {
                    let problem = problem.clone();
                    widgets::problem_box(ui, &problem, |_| {});
                }
                Some(Ok(snapshot)) => {
                    let snapshot = Arc::clone(snapshot);
                    let height = ui.available_height();
                    super::profile::config_table(
                        ui,
                        &format!("repo-config-{}", panels.id),
                        panels.config_table,
                        &snapshot,
                        panels.home,
                        height,
                    );
                }
            }
        });
}
