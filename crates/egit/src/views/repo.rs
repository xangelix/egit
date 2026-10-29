//! A repository's tab: its overview (branch, commit, upstream, what is in progress, who you are
//! here, where it lives) and every setting in effect in it.

use std::path::Path;
use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{
    Committed, ConfigSnapshot, Git, Head, Operation, Repository, Stash, Submodule, Tracking,
    Worktree,
};
use egui::{Grid, RichText, Ui};
use egui_async::Bind;

use crate::app::{Action, AppState};
use crate::forge::RepoForge;
use crate::format::display_path;
use crate::history::HistoryState;
use crate::problem::Problem;
use crate::repo::{RepoId, RepoPanel, Status};
use crate::tables::SnapshotTable;
use crate::tasks::Tasks;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::undo::Journal;
use crate::views::history::{HistoryPane, HistoryView};
use crate::views::sidebar::Lists;
use crate::widgets::{self, muted};
use crate::worktree::WorkTree;

/// Draws repository `id`'s tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState, id: RepoId) {
    let AppState {
        git,
        repos,
        actions,
        home_dir,
        agents,
        agents_there,
        settings,
        ..
    } = state;
    let git = git.ok_ref();
    let Some(repo) = repos.iter_mut().find(|repo| repo.id == id) else {
        ui.label(muted(ui, "This repository was closed."));
        return;
    };
    // Its agents, at the right, beside whatever it shows; then what its tasks and commits
    // printed, at the bottom, under the rest.
    let around = super::agents::Around {
        repo: id,
        work_tree: repo.repository.work_tree(),
        elsewhere: repo.git.host().ssh().map(|ssh| ssh.destination.as_str()),
        git,
        accounts: &settings.agent_accounts,
    };
    // Claude's setup where the repository is: here, or on the computer it is on.
    let setup = repo.git.host().ssh().map_or(agents, |ssh| {
        agents_there
            .entry(ssh.destination.clone())
            .or_insert_with(|| {
                crate::agents::AgentsSetup::there(repo.git.clone(), home_dir.as_deref())
            })
    });
    agents_panel(ui, (setup, &mut repo.agents), (id, around), actions);
    super::output::show(ui, &mut repo.output, id);
    let following_there = repo
        .following_there()
        .map(crate::repo::FollowingThere::words);
    let there = repo
        .git
        .host()
        .ssh()
        .map(|ssh| (ssh.destination.as_str(), following_there));
    let mut behavior = Panels {
        id,
        // The repository's own: on another computer, the git there.
        git: Some(&repo.git),
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
        commit_run: repo.commit_run.and_then(|run| repo.output.run(run)),
        hooks: repo.hooks.ok_ref(),
        tasks: &mut repo.tasks,
        stashes: &mut repo.stashes,
        worktrees: &mut repo.worktrees,
        submodules: &mut repo.submodules,
        lfs: &mut repo.lfs,
        submodule_states: &mut repo.submodule_states,
        writing_commit_graph: &mut repo.writing_commit_graph,
        forge: &mut repo.forge,
        // `~` is the home folder of the computer the repository is on.
        home: if repo.git.host().is_local() {
            home_dir.as_deref()
        } else {
            repo.git.home_there()
        },
        there,
        journal: &repo.journal,
        agents_shown: repo.agents.shown,
        agents_attention: repo.agents.attention(),
        actions,
    };
    match repo.panel {
        RepoPanel::History => history(ui, &mut behavior),
        panel => {
            let (back, switched) = page_bar(ui, panel);
            if let Some(switched) = switched {
                behavior.actions.push(Action::ShowPanel(id, switched));
            }
            // Escape goes back, as from the history's tools; a field keeps it for itself.
            let escaped = !ui.ctx().text_edit_focused()
                && ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            if back || escaped {
                behavior
                    .actions
                    .push(Action::ShowPanel(id, RepoPanel::History));
            }
            if panel == RepoPanel::Overview {
                overview(ui, &mut behavior);
            } else {
                configuration(ui, &mut behavior);
            }
        }
    }
}

/// The bar over a page of the repository's: back to the history, and which page (the other a
/// click away). Returns whether "History" was clicked, and the page switched to.
fn page_bar(ui: &mut Ui, panel: RepoPanel) -> (bool, Option<RepoPanel>) {
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            let back = widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT))
                .on_hover_text("Back to the history (Escape)")
                .clicked();
            let mut shown = panel;
            let switched = widgets::segmented(
                ui,
                &mut shown,
                &[
                    (RepoPanel::Overview, "Overview"),
                    (RepoPanel::Configuration, "Settings"),
                ],
            )
            .then_some(shown);
            (back, switched)
        })
        .inner
    })
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
    commit_run: Option<&'a crate::output::Run>,
    hooks: Option<&'a egit_git::Hooks>,
    tasks: &'a mut Tasks,
    stashes: &'a mut Bind<Arc<[Stash]>, Problem>,
    worktrees: &'a mut Bind<Arc<[Worktree]>, Problem>,
    submodules: &'a mut Bind<Arc<[Submodule]>, Problem>,
    lfs: &'a mut Bind<Arc<crate::lfs::LfsState>, Problem>,
    submodule_states: &'a mut Bind<Arc<crate::submodules::Standing>, Problem>,
    writing_commit_graph: &'a mut Bind<(), Problem>,
    forge: &'a mut RepoForge,
    home: Option<&'a Path>,
    /// The other computer it is on, as ssh reaches it, and how changes made there are
    /// followed, in words (none here; the second while they aren't followed at all).
    there: Option<(&'a str, Option<&'static str>)>,
    journal: &'a Journal,
    agents_shown: bool,
    agents_attention: crate::agents::Attention,
    actions: &'a mut Vec<Action>,
}

/// The history: the commit graph, its tools, and the details beside it.
fn history(ui: &mut Ui, panels: &mut Panels<'_>) {
    let status = panels.status.ok_ref();
    let config_read = panels.config.ok_ref();
    let config = config_read.map(|config| &**config);
    let stashes = panels.stashes.read_as_ref();
    super::history::show(
        ui,
        HistoryPane {
            id: panels.id,
            git: panels.git,
            repository: panels.repository,
            history: panels.history,
            view: panels.history_view,
            head: status.map(|status| &status.head),
            writing_commit_graph: panels.writing_commit_graph,
            partial_clone: config.map(ConfigSnapshot::is_partial_clone),
            work_tree: panels.work_tree,
            work: super::history::WorkSide {
                staging: panels.staging,
                committing: panels.committing,
                commits_made: panels.commits_made,
                commit_run: panels.commit_run,
                hooks: panels.hooks,
                sign_by_default: config
                    .and_then(|config| config.get("commit.gpgSign"))
                    .and_then(egit_git::ConfigEntry::bool)
                    .unwrap_or(false),
            },
            operations: status.map_or(&[], |status| status.operations.as_slice()),
            applying: status.and_then(|status| status.applying.as_ref()),
            bisect: status.and_then(|status| status.bisect.as_ref()),
            config,
            tasks: panels.tasks,
            stashes: stashes
                .and_then(Result::ok)
                .map_or(&[], |stashes| &**stashes),
            forge: &*panels.forge,
            home: panels.home,
            journal: panels.journal,
            agents: (panels.agents_shown, panels.agents_attention),
            lists: Lists {
                stashes,
                worktrees: panels.worktrees.read_as_ref(),
                submodules: panels.submodules.read_as_ref(),
                lfs: panels.lfs.read_as_ref(),
                submodule_states: panels.submodule_states.read_as_ref(),
                config: config_read,
            },
            actions: panels.actions,
        },
    );
}

fn overview(ui: &mut Ui, panels: &mut Panels<'_>) {
    let palette = Palette::of(ui.ctx());
    super::page(ui, ("repo-overview", panels.id), |ui| {
        ui.horizontal(|ui| {
            widgets::page_title(ui, panels.title);
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
        location_card(ui, panels.repository, panels.home, panels.there);
        ui.add_space(12.0);
        super::forge_overview::card(
            ui,
            (&mut *panels.forge, &panels.history.shown),
            panels.id,
            panels.actions,
        );
        ui.add_space(12.0);
        hooks_card(ui, panels.hooks, panels.home);
    });
}

/// The hooks git runs here: where it looks for them, which are there, and where their output
/// shows.
fn hooks_card(ui: &mut Ui, hooks: Option<&egit_git::Hooks>, home: Option<&Path>) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Hooks");
        ui.add_space(4.0);
        let Some(hooks) = hooks else {
            widgets::working(ui, "Reading the hooks…");
            return;
        };
        widgets::fact(ui, widgets::FACT_LABEL, "Folder", |ui| {
            let mut folder = display_path(&hooks.folder, home);
            if hooks.configured {
                folder.push_str(" (core.hooksPath)");
            }
            ui.add(egui::Label::new(folder).truncate());
        });
        widgets::fact(ui, widgets::FACT_LABEL, "Installed", |ui| {
            let installed = if hooks.names.is_empty() {
                "None".to_owned()
            } else {
                hooks.names.join(", ")
            };
            ui.add(egui::Label::new(installed).wrap());
        });
        ui.add(
            egui::Label::new(muted(
                ui,
                format!(
                    "git runs them as it commits, checks out, merges, rebases, and pushes; what \
                     they print shows in the output at the tab's bottom ({}).",
                    crate::commands::keys_text("⌘J")
                ),
            ))
            .wrap(),
        );
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
            .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_BOX))
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
            .min_col_width(widgets::FACT_LABEL)
            .spacing([16.0, 8.0])
            .show(ui, |ui| match head {
                Head::Branch(branch) => {
                    ui.label(muted(ui, "On"));
                    ui.label(
                        RichText::new(format!("{}  {}", icon::GIT_BRANCH, branch.name)).semibold(),
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
                    .min_col_width(widgets::FACT_LABEL)
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

fn location_card(
    ui: &mut Ui,
    repository: &Repository,
    home: Option<&Path>,
    there: Option<(&str, Option<&str>)>,
) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Where it lives");
        // A path's cell is as wide as the card leaves it, wrapped within: a grid sizes cells to
        // what they hold, and a long path would widen the page past the window.
        let room = (ui.available_width() - widgets::FACT_LABEL - 16.0).max(120.0);
        let path = |ui: &mut Ui, at: &Path| {
            ui.allocate_ui(egui::vec2(room, 0.0), |ui| {
                ui.set_max_width(room);
                ui.add(egui::Label::new(display_path(at, home)).wrap());
            });
        };
        Grid::new(("repo-location", repository.work_tree()))
            .num_columns(2)
            .min_col_width(widgets::FACT_LABEL)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                if let Some((computer, _)) = there {
                    ui.label(muted(ui, "Computer"));
                    ui.label(format!("{computer}, over SSH"));
                    ui.end_row();
                }
                ui.label(muted(ui, "Work tree"));
                path(ui, repository.work_tree());
                ui.end_row();
                ui.label(muted(ui, "Git directory"));
                path(ui, repository.git_dir());
                ui.end_row();
                if repository.is_linked_worktree() {
                    ui.label(muted(ui, "Shared with"));
                    path(ui, repository.common_dir());
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
                if let Some((_, Some(following))) = there {
                    ui.label(muted(ui, "Changes there"));
                    ui.add(egui::Label::new(following).wrap());
                    ui.end_row();
                }
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

/// The agents' panel at the tab's right, while shown (the top bar's Agents shows it).
fn agents_panel(
    ui: &mut Ui,
    (setup, agents): (
        &mut crate::agents::AgentsSetup,
        &mut crate::agents::RepoAgents,
    ),
    (id, around): (RepoId, super::agents::Around<'_>),
    actions: &mut Vec<Action>,
) {
    if !agents.shown {
        return;
    }
    let palette = crate::theme::Palette::of(ui.ctx());
    egui::Panel::right(egui::Id::new(("agents", id)))
        .resizable(true)
        // At first a third of the window at most: the history keeps its room.
        .default_size(
            (ui.available_width() * 0.35)
                .clamp(*super::agents::WIDTHS.start(), super::agents::WIDTH),
        )
        .size_range(super::agents::WIDTHS)
        .frame(
            // Its parts keep their own margins: the transcript scrolls to the panel's edges.
            egui::Frame::new().fill(palette.panel),
        )
        .show(ui, |ui| {
            super::agents::panel(ui, (setup, agents), around, actions);
        });
    // Drawn, the conversation shown is seen: a turn it ended no longer calls for attention.
    if let Some(conversation) = agents.current() {
        conversation.seen();
    }
}
