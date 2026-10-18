//! Submodules in the commit table's place: what they are, where they stand (nested ones too)
//! and what mends them, each one's details (its addresses, branch, the commit recorded and the
//! one checked out, what lies between) with what can be done to it, and adding one. Escape or
//! History goes back.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{Checkout, ObjectId, Submodule, SubmoduleDetails, SubmoduleStatus, UpdateTo};
use egui::{Frame, Id, Margin, RichText, ScrollArea, Sense, Ui};
use egui_async::Bind;

use super::history::{ID_DIGITS, Shared};
use crate::problem::Problem;
use crate::submodules::{OutOfStep, Standing, SubmoduleTask, read_details, url_is_stale};
use crate::tasks::Task;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// What the page asks of the history view.
#[derive(Debug)]
pub(super) enum SubmodulesEvent {
    Close,
    Task(Task),
    /// Open the repository at this folder in a tab.
    Open(PathBuf),
    /// Record a submodule's commit here: stage its path.
    Record(BString),
}

/// What a selected submodule's details are, as read.
type Details = Arc<(SubmoduleDetails, Option<Vec<(ObjectId, BString)>>)>;

/// The page's state.
#[derive(Debug, Default)]
pub(super) struct SubmodulesView {
    /// The submodule selected, by path.
    selected: Option<BString>,
    details: Bind<Details, Problem>,
    /// What the details were read for: the path, and the states read (read again when they are).
    details_for: Option<(BString, usize)>,
    /// A submodule being added: its address, path, and branch as typed.
    adding: Option<[String; 3]>,
    /// A new address or branch being typed for the selected one.
    url: Option<String>,
    branch: Option<String>,
    /// Removing the selected one, asked to confirm.
    removing: bool,
}

/// A row's height.
const ROW: f32 = 26.0;

/// Draws the page; returns what it asked.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut SubmodulesView,
) -> Option<SubmodulesEvent> {
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut event = escape.then_some(SubmodulesEvent::Close);
    let modules = shared.submodules.map_or(&[][..], |modules| &**modules);
    let standing = shared.submodule_states.map(|standing| &**standing);
    let states = standing.map_or(&[][..], |standing| standing.states.as_slice());
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT))
                .on_hover_text("Back to the history (Escape)")
                .clicked()
            {
                event = Some(SubmodulesEvent::Close);
            }
            ui.label(RichText::new("Submodules").semibold());
            ui.label(muted(ui, count_words(states.len())));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::secondary(ui, view.adding.is_none(), "Add a submodule…").clicked() {
                    view.adding = Some(Default::default());
                }
            });
        });
    });
    ScrollArea::vertical()
        .id_salt(("submodules-page", shared.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(900.0));
                let mut ask = |asked| event = Some(asked);
                if adding(ui, shared, view, &mut ask) {
                    ui.add_space(12.0);
                }
                standing_card(ui, shared, (modules, standing), &mut ask);
                if states.is_empty() {
                    return;
                }
                ui.add_space(12.0);
                list_card(ui, shared, states, view);
                if let Some(path) = view.selected.clone() {
                    ui.add_space(12.0);
                    details_card(
                        ui,
                        shared,
                        (modules, states),
                        (view, path.as_ref()),
                        &mut ask,
                    );
                }
            });
        });
    event
}

/// "3 submodules", or none.
fn count_words(count: usize) -> String {
    match count {
        0 => "none".to_owned(),
        1 => "1 submodule".to_owned(),
        count => format!("{count} submodules"),
    }
}

/// What submodules are, how they stand altogether, and what mends them all.
fn standing_card(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (modules, standing): (&[Submodule], Option<&Standing>),
    ask: &mut impl FnMut(SubmodulesEvent),
) {
    let palette = shared.palette;
    let empty = Standing::default();
    let standing = standing.unwrap_or(&empty);
    let states = &standing.states;
    let out = OutOfStep::of(modules, standing, shared.config);
    let task = |task| SubmodulesEvent::Task(Task::Submodules(task));
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Repositories inside this one");
        ui.add(
            egui::Label::new(muted(
                ui,
                "A submodule is another repository, checked out here at a commit this one \
                 records, not on a branch. Pulling or switching here records new commits for \
                 them without moving them, unless they are updated: until then they are at \
                 other commits than recorded.",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        if states.is_empty() {
            ui.label(muted(
                ui,
                "This repository has none: add one to keep another repository inside it.",
            ));
            return;
        }
        let (said, color) = if out.any() {
            (out.words(), palette.warn)
        } else if out.past > 0 {
            (out.words(), palette.muted)
        } else {
            (
                "Every one set up and at the commit recorded.".to_owned(),
                palette.ok,
            )
        };
        ui.add(egui::Label::new(RichText::new(said).color(color)).wrap());
        ui.add_space(8.0);
        let idle = !shared.busy;
        ui.horizontal_wrapped(|ui| {
            if widgets::primary(ui, idle && out.any(), "Mend everything")
                .on_hover_text(
                    "Sync their addresses with .gitmodules, then set up every one and bring it \
                     to the commit recorded, nested ones too: what usually gets a checkout working",
                )
                .clicked()
            {
                ask(task(SubmoduleTask::Mend));
            }
            let updates = [
                ("Update all to the commits recorded", UpdateTo::Recorded, "Check out the commits this repository records in each (setting up those not here)"),
                ("Update all to their branches' latest", UpdateTo::Latest, "Fetch each one's branch and check out its latest commit: then record them here, to commit"),
            ];
            for (label, to, hover) in updates {
                if widgets::secondary(ui, idle, label).on_hover_text(hover).clicked() {
                    ask(task(SubmoduleTask::Update { paths: Vec::new(), to }));
                }
            }
            if widgets::secondary(ui, idle, "Sync addresses")
                .on_hover_text("Set up each one's address again from .gitmodules (after it changed there)")
                .clicked()
            {
                ask(task(SubmoduleTask::Sync(Vec::new())));
            }
        });
        let mut recurse = shared
            .config
            .and_then(|config| config.get("submodule.recurse"))
            .and_then(egit_git::ConfigEntry::bool)
            .unwrap_or(false);
        if widgets::checkbox(ui, &mut recurse, "Update them on every switch and pull")
            .on_hover_text(
                "submodule.recurse in this repository's configuration: git then moves the \
                 submodules along with what it checks out",
            )
            .changed()
        {
            ask(task(SubmoduleTask::Recurse(recurse)));
        }
    });
}

/// How deep a submodule at `path` is: how many of the others hold it.
fn depth(states: &[SubmoduleStatus], path: &BStr) -> usize {
    states
        .iter()
        .filter(|other| {
            path.len() > other.path.len()
                && path.starts_with(&other.path)
                && path.get(other.path.len()) == Some(&b'/')
        })
        .count()
}

/// A state's icon, colour, and word.
const fn look(checkout: Checkout, palette: Palette) -> (&'static str, egui::Color32, &'static str) {
    match checkout {
        Checkout::NotInitialized => (icon::CIRCLE_DASHED, palette.muted, "not set up"),
        Checkout::AtRecorded => (icon::CHECK_CIRCLE, palette.ok, "at the commit recorded"),
        Checkout::Elsewhere => (
            icon::ARROW_FAT_LINES_RIGHT,
            palette.warn,
            "at another commit",
        ),
        Checkout::Conflict => (icon::WARNING, palette.danger, "in conflict"),
    }
}

/// Every submodule (nested ones indented under theirs), its state, and its commit; a click
/// selects one.
fn list_card(
    ui: &mut Ui,
    shared: &Shared<'_>,
    states: &[SubmoduleStatus],
    view: &mut SubmodulesView,
) {
    let palette = shared.palette;
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Each one");
        let shown = f32::from(u16::try_from(states.len().min(14)).unwrap_or(14));
        ScrollArea::vertical()
            .id_salt(("submodules-list", shared.id))
            .max_height(ROW * shown + 8.0)
            .auto_shrink([false, true])
            .show_rows(ui, ROW, states.len(), |ui, rows| {
                for state in states.get(rows).unwrap_or_default() {
                    let selected = view.selected.as_ref() == Some(&state.path);
                    let (rect, _) = ui
                        .allocate_exact_size(egui::vec2(ui.available_width(), ROW), Sense::hover());
                    let name = state.path.to_str_lossy();
                    let id = Id::new(("submodule-row", shared.id, state.path.as_slice()));
                    let response = ui.interact(rect, id, Sense::click());
                    // Named with its state: the sidebar has a row of the same path.
                    let (glyph, color, word) = look(state.checkout, palette);
                    let named = format!("{name}, {word}");
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            selected,
                            &named,
                        )
                    });
                    // Its labels take the hover from it: the pointer anywhere on the row lights it.
                    widgets::row_fill(
                        ui,
                        rect,
                        f32::from(crate::theme::RADIUS_INNER),
                        selected,
                        response.contains_pointer(),
                    );
                    if response.clicked() {
                        view.selected = Some(state.path.clone());
                        view.url = None;
                        view.branch = None;
                        view.removing = false;
                    }
                    let indent = 18.0
                        * f32::from(u16::try_from(depth(states, state.path.as_ref())).unwrap_or(0));
                    let mut row = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    row.add_space(indent);
                    // Words only: the row takes the clicks.
                    let words = |text: RichText| {
                        egui::Label::new(text)
                            .selectable(false)
                            .sense(Sense::empty())
                    };
                    row.add(words(RichText::new(glyph).color(color)));
                    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(words(RichText::new(word).color(color)));
                        let mut at = state.commit.abbreviated(ID_DIGITS).to_string();
                        if let Some(describe) = &state.describe {
                            let _ = write!(at, " ({})", describe.to_str_lossy());
                        }
                        ui.add(words(RichText::new(at).color(palette.muted)));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(words(RichText::new(name.as_ref()).medium()).truncate());
                        });
                    });
                }
            });
    });
}

/// The selected submodule: what its state means, its facts, what lies between the commit
/// recorded and the one checked out, and what can be done to it.
fn details_card(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (modules, states): (&[Submodule], &[SubmoduleStatus]),
    (view, path): (&mut SubmodulesView, &BStr),
    ask: &mut impl FnMut(SubmodulesEvent),
) {
    let Some(state) = states.iter().find(|state| state.path == path) else {
        return;
    };
    let module = modules.iter().find(|module| module.path == path);
    let read = shared
        .submodule_states
        .map_or(0, |standing| Arc::as_ptr(standing).addr());
    if let (Some(module), Some(git)) = (module, shared.git)
        && state.checkout != Checkout::NotInitialized
        && view.details_for.as_ref() != Some(&(path.to_owned(), read))
    {
        // Another submodule's details go; the same one's show until read again.
        if view
            .details_for
            .as_ref()
            .is_none_or(|(known, _)| known != path)
        {
            view.details = Bind::new(true);
        }
        view.details_for = Some((path.to_owned(), read));
        view.details.request(read_details(
            git.clone(),
            Arc::clone(shared.repository),
            path.to_owned(),
            module.recorded,
        ));
    }
    widgets::card(ui, |ui| {
        widgets::heading(ui, &path.to_str_lossy());
        meaning(ui, shared.palette, state.checkout);
        ui.add_space(8.0);
        facts(ui, shared, module, state);
        if state.checkout != Checkout::NotInitialized {
            inside(ui, shared, view);
        }
        ui.add_space(8.0);
        // Ids of their own: what is drawn above them comes and goes as it is read, and a click
        // on an id that moved meanwhile is lost.
        let builder =
            egui::UiBuilder::new().id(Id::new(("submodule-actions", shared.id, path.as_bytes())));
        ui.scope_builder(builder, |ui| match module {
            Some(module) => actions(ui, shared, (module, state), view, ask),
            None => nested_actions(ui, shared, state, ask),
        });
    });
}

/// What a state means, and what to do about it.
fn meaning(ui: &mut Ui, palette: Palette, checkout: Checkout) {
    let (glyph, color, _) = look(checkout, palette);
    let said = match checkout {
        Checkout::NotInitialized => {
            "Not set up here: its folder is empty. Setting it up clones it (or reuses .git/modules) \
             and checks out the commit recorded."
        }
        Checkout::AtRecorded => "At the commit this repository records: all is as it should be.",
        Checkout::Elsewhere => {
            "At another commit than recorded. Either a pull or switch here recorded a new commit \
             it isn't updated to yet (update it), or you moved it, committing or checking out in \
             it (record its commit here to keep that; updating would leave those commits only \
             in its reflog)."
        }
        Checkout::Conflict => {
            "In a merge conflict: each side records another commit. Check out the one to keep \
             in it, then record it here (stage its path) to resolve the conflict."
        }
    };
    ui.add(egui::Label::new(RichText::new(format!("{glyph}  {said}")).color(color)).wrap());
}

/// Its name, addresses, branch, and commits.
fn facts(ui: &mut Ui, shared: &Shared<'_>, module: Option<&Submodule>, state: &SubmoduleStatus) {
    let label = 120.0;
    let text = |ui: &mut Ui, value: String| {
        ui.add(egui::Label::new(value).truncate());
    };
    if let Some(module) = module {
        widgets::fact(ui, label, "Name", |ui| {
            text(ui, module.name.to_str_lossy().into_owned());
        });
        let declared = module
            .url
            .as_ref()
            .map_or_else(|| "none".to_owned(), |url| url.to_str_lossy().into_owned());
        widgets::fact(ui, label, "Address", |ui| text(ui, declared));
        if url_is_stale(module, shared.config)
            && let Some(configured) = &module.configured_url
        {
            widgets::fact(ui, label, "Set up with", |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} (out of date: sync it)",
                            configured.to_str_lossy()
                        ))
                        .color(shared.palette.warn),
                    )
                    .truncate(),
                );
            });
        }
        let branch = module.branch.as_ref().map_or_else(
            || "its remote's default branch".to_owned(),
            |branch| branch.to_str_lossy().into_owned(),
        );
        widgets::fact(ui, label, "Follows", |ui| text(ui, branch));
        widgets::fact(ui, label, "Recorded", |ui| {
            ui.label(
                RichText::new(module.recorded.abbreviated(ID_DIGITS * 2).to_string()).monospace(),
            );
        });
    }
    if state.checkout != Checkout::NotInitialized {
        widgets::fact(ui, label, "Checked out", |ui| {
            ui.label(
                RichText::new(state.commit.abbreviated(ID_DIGITS * 2).to_string()).monospace(),
            );
            if let Some(describe) = &state.describe {
                ui.label(muted(ui, describe.to_str_lossy()));
            }
        });
    }
}

/// What it is at inside: on a branch or detached, ahead or behind the commit recorded, and the
/// commits between.
fn inside(ui: &mut Ui, shared: &Shared<'_>, view: &mut SubmodulesView) {
    let label = 120.0;
    match view.details.read_as_ref() {
        None => widgets::working(ui, "Looking inside…"),
        Some(Err(problem)) => {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
        }
        Some(Ok(read)) => {
            let (details, commits) = &**read;
            let on = details.branch.as_ref().map_or_else(
                || "no branch: detached at a commit, as submodules usually are".to_owned(),
                |branch| format!("the branch {}", branch.to_str_lossy()),
            );
            widgets::fact(ui, label, "On", |ui| {
                ui.add(egui::Label::new(on).truncate());
            });
            if !details.has_recorded {
                ui.label(
                    RichText::new(
                        "It doesn't have the commit recorded yet: updating it fetches it.",
                    )
                    .color(shared.palette.warn),
                );
            } else if details.ahead > 0 || details.behind > 0 {
                let said = format!(
                    "{} commits past the commit recorded, {} behind it",
                    details.ahead, details.behind
                );
                widgets::fact(ui, label, "Against it", |ui| {
                    ui.label(said);
                });
            }
            if let Some(commits) = commits.as_ref().filter(|commits| !commits.is_empty()) {
                ui.add_space(4.0);
                ui.label(muted(
                    ui,
                    "Its commits the commit recorded doesn't have, newest first:",
                ));
                for (id, subject) in commits.iter().take(20) {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(id.abbreviated(ID_DIGITS).to_string())
                                .monospace()
                                .color(shared.palette.muted),
                        );
                        ui.add(egui::Label::new(subject.to_str_lossy().as_ref()).truncate());
                    });
                }
            }
        }
    }
}

/// What can be done to a submodule this repository declares.
fn actions(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (module, state): (&Submodule, &SubmoduleStatus),
    view: &mut SubmodulesView,
    ask: &mut impl FnMut(SubmodulesEvent),
) {
    let idle = !shared.busy;
    let path = module.path.clone();
    let task = |task| SubmodulesEvent::Task(Task::Submodules(task));
    let folder = shared.repository.work_tree().join(path.to_path_lossy());
    ui.horizontal_wrapped(|ui| {
        let (first, hover) = if state.checkout == Checkout::NotInitialized {
            (
                "Set it up",
                "Clone it (or reuse .git/modules) and check out the commit recorded",
            )
        } else {
            (
                "Update to the commit recorded",
                "Check out the commit this repository records in it",
            )
        };
        if widgets::primary(ui, idle, first)
            .on_hover_text(hover)
            .clicked()
        {
            ask(task(SubmoduleTask::Update {
                paths: vec![path.clone()],
                to: UpdateTo::Recorded,
            }));
        }
        if widgets::secondary(ui, idle, "Update to its branch's latest")
            .on_hover_text("Fetch its branch and check out its latest commit: then record it here")
            .clicked()
        {
            ask(task(SubmoduleTask::Update {
                paths: vec![path.clone()],
                to: UpdateTo::Latest,
            }));
        }
        if matches!(state.checkout, Checkout::Elsewhere | Checkout::Conflict)
            && widgets::secondary(ui, idle, "Record its commit here")
                .on_hover_text(
                    "Stage its path: the commit it is at becomes the one recorded, to commit",
                )
                .clicked()
        {
            ask(SubmodulesEvent::Record(path.clone()));
        }
        if url_is_stale(module, shared.config)
            && widgets::secondary(ui, idle, "Sync its address").clicked()
        {
            ask(task(SubmoduleTask::Sync(vec![path.clone()])));
        }
        if state.checkout != Checkout::NotInitialized
            && widgets::secondary(ui, true, "Open in a tab").clicked()
        {
            ask(SubmodulesEvent::Open(folder.clone()));
        }
    });
    edits(ui, shared, module, view, ask);
    ui.horizontal_wrapped(|ui| {
        if !view.removing {
            if state.checkout != Checkout::NotInitialized
                && widgets::danger(ui, idle, "Forget it here")
                    .on_hover_text("Empty its folder and remove its setup (deinit); still recorded, set up again later")
                    .clicked()
            {
                ask(task(SubmoduleTask::Deinit { path: path.clone(), force: false }));
            }
            if widgets::danger(ui, idle, "Remove…").clicked() {
                view.removing = true;
            }
            return;
        }
        ui.label(format!(
            "Remove {}? Its folder, its gitlink, and its .gitmodules section go (staged, to commit).",
            path.to_str_lossy()
        ));
        if widgets::danger(ui, idle, "Remove it").clicked() {
            ask(task(SubmoduleTask::Remove { path: path.clone(), force: false }));
            view.removing = false;
            view.selected = None;
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            view.removing = false;
        }
    });
}

/// Changing its address or its branch, typed in place.
fn edits(
    ui: &mut Ui,
    shared: &Shared<'_>,
    module: &Submodule,
    view: &mut SubmodulesView,
    ask: &mut impl FnMut(SubmodulesEvent),
) {
    let idle = !shared.busy;
    let path = module.path.clone();
    let task = |task| SubmodulesEvent::Task(Task::Submodules(task));
    ui.horizontal(|ui| {
        let url = view.url.get_or_insert_with(|| {
            module
                .url
                .as_ref()
                .map_or_else(String::new, |url| url.to_str_lossy().into_owned())
        });
        let label = ui.label(muted(ui, "Address"));
        ui.add(widgets::line_field(url).desired_width(320.0))
            .labelled_by(label.id);
        let changed = module
            .url
            .as_ref()
            .is_none_or(|known| known != url.trim().as_bytes());
        if widgets::secondary(
            ui,
            idle && changed && !url.trim().is_empty(),
            "Change address",
        )
        .on_hover_text("In .gitmodules and here (set-url), to commit")
        .clicked()
        {
            ask(task(SubmoduleTask::SetUrl {
                path: path.clone(),
                url: url.trim().to_owned(),
            }));
        }
    });
    ui.horizontal(|ui| {
        let branch = view.branch.get_or_insert_with(|| {
            module
                .branch
                .as_ref()
                .map_or_else(String::new, |branch| branch.to_str_lossy().into_owned())
        });
        let label = ui.label(muted(ui, "Branch"));
        ui.add(
            widgets::line_field(branch)
                .hint_text("its remote's default")
                .desired_width(200.0),
        )
        .labelled_by(label.id);
        let typed = branch.trim();
        let changed = module
            .branch
            .as_ref()
            .map(|known| known.to_str_lossy().into_owned())
            != (!typed.is_empty()).then(|| typed.to_owned());
        if widgets::secondary(ui, idle && changed, "Follow it")
            .on_hover_text(
                "The branch updates to its latest follow (set-branch), in .gitmodules, to commit",
            )
            .clicked()
        {
            ask(task(SubmoduleTask::SetBranch {
                path,
                branch: (!typed.is_empty()).then(|| typed.to_owned()),
            }));
        }
    });
}

/// A nested submodule's: its own repository is its parent's, so it is changed from there.
fn nested_actions(
    ui: &mut Ui,
    shared: &Shared<'_>,
    state: &SubmoduleStatus,
    ask: &mut impl FnMut(SubmodulesEvent),
) {
    ui.add(
        egui::Label::new(muted(
            ui,
            "A submodule of a submodule: updating its parent from here (or Mend everything) \
             brings it along; to change it, open its parent in a tab.",
        ))
        .wrap(),
    );
    if state.checkout != Checkout::NotInitialized
        && widgets::secondary(ui, true, "Open in a tab").clicked()
    {
        ask(SubmodulesEvent::Open(
            shared
                .repository
                .work_tree()
                .join(state.path.to_path_lossy()),
        ));
    }
}

/// The form adding a submodule, when open; returns whether it showed.
fn adding(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut SubmodulesView,
    ask: &mut impl FnMut(SubmodulesEvent),
) -> bool {
    let Some([url, path, branch]) = &mut view.adding else {
        return false;
    };
    let mut close = false;
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Add a submodule");
        let label = 120.0;
        widgets::form_row(ui, label, "Address", |ui| {
            let typed = ui.add(widgets::line_field(url).hint_text("https://example.com/lib.git"));
            if typed.changed() && path.is_empty() {
                // As git names the folder: the address's last part, without .git.
                let name = url
                    .trim_end_matches('/')
                    .rsplit(['/', ':'])
                    .next()
                    .unwrap_or("");
                name.trim_end_matches(".git").clone_into(path);
            }
            typed
        });
        widgets::form_row(ui, label, "Folder", |ui| {
            ui.add(widgets::line_field(path).hint_text("libs/lib"))
        });
        widgets::form_row(ui, label, "Branch", |ui| {
            ui.add(widgets::line_field(branch).hint_text("its remote's default"))
        });
        ui.horizontal(|ui| {
            let ready = !shared.busy && !url.trim().is_empty() && !path.trim().is_empty();
            if widgets::primary(ui, ready, "Add it").clicked() {
                ask(SubmodulesEvent::Task(Task::Submodules(
                    SubmoduleTask::Add {
                        url: url.trim().to_owned(),
                        path: path.trim().into(),
                        branch: (!branch.trim().is_empty()).then(|| branch.trim().to_owned()),
                    },
                )));
                close = true;
            }
            if widgets::secondary(ui, true, "Cancel").clicked() {
                close = true;
            }
        });
    });
    if close {
        view.adding = None;
    }
    true
}
