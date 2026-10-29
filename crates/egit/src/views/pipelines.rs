//! Every pipeline of a repository's project (GitLab's), or workflow run (GitHub Actions'), in
//! the commit table's place: the newest first, read a page at a time, the first when the list
//! is shown and each after it as the list is scrolled towards the end of what was read; the
//! newest page read again while one on it may still change. Each row: how it went, its title
//! (a GitHub run's attempt past the first), the branch or tag it ran for, when it was made
//! and how long it ran (ticking while it runs); under them, its commit, a link back to it in
//! the history where the history holds it, its subject, and who made it run and how. A click
//! (or Enter) shows the pipeline's page, which goes back here; several projects are chosen
//! between in the bar.
//!
//! Which commits the history holds is worked out in a job (each history's names scanned
//! against the list's commits, sorted), as the list grows or the history is read again; only
//! those commits' subjects are read, through the history's summaries: a commit the forge ran
//! but this clone hasn't fetched is never asked of git (a partial clone would fetch it).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use egit_forge::{ListedPipeline, PageAfter, PipelineListing, PipelinesPage, pipelines_page};
use egit_git::{CancelToken, ObjectId};
use egui::{Align, Frame, Id, Key, Layout, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;
use web_time::Instant;

use super::history::Shared;
use super::pipeline::{CiWords, ci_look, ci_words, source_words};
use crate::ci::CiMark;
use crate::forge::RepoForge;
use crate::format;
use crate::history::HistoryState;
use crate::jobs;
use crate::problem::Problem;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// A row's height: two lines, each as tall as a link to its commit (28 points).
const ROW_HEIGHT: f32 = 60.0;

/// How few rows past those in view may be left unread before the next page is read: about a
/// screen of them.
const READ_AHEAD: usize = 30;

/// How soon the newest page is read again while one on it may still change.
const EVERY: Duration = Duration::from_secs(20);

/// The help topic the list's parts are explained by.
const TOPIC: &str = "pipelines-list";

/// The list's state.
#[derive(Debug)]
pub(super) struct PipelinesView {
    /// The project shown: its place among the forge's repositories.
    project: usize,
    /// The pipelines read so far, the newest first.
    shown: Vec<ListedPipeline>,
    /// Their ids (a page read again, or one moved along by runs made meanwhile, repeats some).
    ids: HashSet<u64>,
    /// Bumped whenever `shown` changes.
    version: u64,
    /// Where the next page starts: none before the first lands, and after the last.
    next: Option<PageAfter>,
    /// Whether the first page has landed.
    first_landed: bool,
    /// Whether the forge lists none to the account (CI/CD off, or a role that may not see it).
    hidden: bool,
    /// The page being read, and whether it is the newest (merged into what is shown) or the
    /// one after what is shown.
    reading: Bind<(PipelinesPage, bool), Problem>,
    /// Why the last read failed, until one goes through.
    problem: Option<Problem>,
    cancel: CancelToken,
    /// When the newest page is read again (while one on it may still change).
    again: Option<Instant>,
    /// The pipeline selected (by its forge's id), and whether to bring it into view.
    selected: Option<u64>,
    reveal: bool,
    /// Where the pipelines' commits are in the history.
    in_history: InHistory,
}

impl Drop for PipelinesView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the list asks of the history view.
pub(super) enum PipelinesEvent {
    Close,
    /// Show this pipeline's page, in the list's place (going back to it).
    Show(Box<CiMark>),
    /// Select this commit in the history.
    ShowCommit(ObjectId),
    Open(String),
}

impl PipelinesView {
    /// The list of the first project's pipelines, nothing read yet.
    pub(super) fn new() -> Self {
        Self::of(0)
    }

    /// The list of project `project`'s (its place among the forge's repositories).
    fn of(project: usize) -> Self {
        Self {
            project,
            shown: Vec::new(),
            ids: HashSet::new(),
            version: 0,
            next: None,
            first_landed: false,
            hidden: false,
            reading: Bind::new(true),
            problem: None,
            cancel: CancelToken::new(),
            again: None,
            selected: None,
            reveal: false,
            in_history: InHistory::default(),
        }
    }

    /// How many pipelines were read, and whether there are more to read.
    #[cfg(test)]
    pub(super) const fn read_so_far(&self) -> (usize, bool) {
        (self.shown.len(), self.next.is_some())
    }

    /// Reads the newest page (`newest`), or the one after what was read.
    fn read(&mut self, forge: &RepoForge, newest: bool) {
        let Some(repository) = forge.repositories.get(self.project).cloned() else {
            return;
        };
        let words = CiWords::of(repository.host().kind());
        let Some(account) = forge.account(repository.host()) else {
            self.problem = Some(Problem::new(
                format!("The {} couldn't be read", words.nouns),
                format!("egit has no account for {}.", repository.host().name()),
            ));
            return;
        };
        let after = if newest {
            None
        } else {
            match &self.next {
                Some(next) => Some(next.clone()),
                None => return,
            }
        };
        let cancel = self.cancel.clone();
        self.reading.request(jobs::blocking(async move {
            let page = pipelines_page(&account, &repository, after.as_ref(), &cancel)
                .await
                .map_err(|error| {
                    Problem::new(
                        format!("The {} couldn't be read", words.nouns),
                        error.to_string(),
                    )
                })?;
            Ok((page, newest))
        }));
    }

    /// Takes in a page that landed: the newest merged in (each pipeline read again replacing
    /// the one shown, those made since put first), or the next added after what is shown.
    fn take_in(&mut self) {
        let Some(read) = self.reading.take() else {
            return;
        };
        let (page, newest) = match read {
            Ok(read) => read,
            Err(problem) => {
                self.problem = Some(problem);
                // Tried again in a while: a network gone may come back.
                self.again = Instant::now().checked_add(EVERY);
                return;
            }
        };
        self.problem = None;
        if page.listing == PipelineListing::Hidden {
            self.hidden = true;
            return;
        }
        self.hidden = false;
        let PipelinesPage {
            pipelines, next, ..
        } = page;
        let moving = newest
            && pipelines
                .iter()
                .any(|listed| listed.pipeline.status.is_moving());
        // The newest read again leaves where the next page starts as it was.
        if !newest || !self.first_landed {
            self.first_landed = true;
            self.next = next;
        }
        let mut changed = false;
        for listed in pipelines {
            if self.ids.insert(listed.pipeline.id) {
                self.shown.push(listed);
                changed = true;
            } else if let Some(kept) = self
                .shown
                .iter_mut()
                .find(|kept| kept.pipeline.id == listed.pipeline.id)
                && *kept != listed
            {
                *kept = listed;
                changed = true;
            }
        }
        if changed {
            // The newest first: those made since the list was read come before the rest.
            self.shown.sort_by(|a, b| {
                (b.pipeline.created, b.pipeline.id).cmp(&(a.pipeline.created, a.pipeline.id))
            });
            self.version = self.version.wrapping_add(1);
        }
        if newest {
            self.again = moving.then(|| Instant::now().checked_add(EVERY)).flatten();
        }
    }

    /// Reads what is due: the first page, the newest again in its time, or the next once the
    /// rows in view come near the end of what was read (`last_in_view`).
    fn follow(&mut self, ui: &Ui, forge: &RepoForge, last_in_view: Option<usize>) {
        if forge.repositories.get(self.project).is_none() {
            // The remotes changed: the first project's, afresh.
            *self = Self::of(0);
        }
        self.take_in();
        if self.reading.is_pending() || self.hidden {
            return;
        }
        let now = Instant::now();
        if !self.first_landed {
            if self.problem.is_none() || self.again.is_some_and(|again| now >= again) {
                self.again = None;
                self.read(forge, true);
            }
            return;
        }
        if self.again.is_some_and(|again| now >= again) {
            self.again = None;
            self.read(forge, true);
            return;
        }
        let near_end =
            last_in_view.is_some_and(|last| last.saturating_add(READ_AHEAD) >= self.shown.len());
        if near_end && self.next.is_some() && self.problem.is_none() {
            self.read(forge, false);
        } else if let Some(again) = self.again {
            ui.ctx()
                .request_repaint_after(again.saturating_duration_since(now));
        }
    }

    /// Reads everything again from the newest (Refresh), what is shown kept until it lands.
    fn refresh(&mut self, forge: &RepoForge) {
        self.problem = None;
        if self.reading.is_pending() {
            // After the page under way.
            self.again = Some(Instant::now());
        } else {
            self.again = None;
            self.read(forge, true);
        }
    }

    /// Shows project `project`'s pipelines instead, read afresh.
    fn choose(&mut self, project: usize) {
        if project != self.project {
            *self = Self::of(project);
        }
    }
}

/// Where the pipelines' commits are in the history shown: worked out in a job for a history
/// and the list as it was then, the last answer kept while the next is worked out.
#[derive(Debug)]
struct InHistory {
    /// Which history and list it was worked out for.
    of: Option<((u64, usize), u64)>,
    working: Bind<Arc<[(ObjectId, u64)]>, Problem>,
    /// Each commit found and its row, sorted by commit.
    found: Arc<[(ObjectId, u64)]>,
}

impl Default for InHistory {
    fn default() -> Self {
        Self {
            of: None,
            working: Bind::new(true),
            found: Arc::from([]),
        }
    }
}

impl InHistory {
    /// Works it out again where the history or the list (`shown`, as of `version`) changed.
    fn follow(&mut self, history: &HistoryState, (shown, version): (&[ListedPipeline], u64)) {
        if let Some(Ok(found)) = self.working.take() {
            self.found = found;
        }
        let key = (history.shown.identity(), version);
        if self.of == Some(key) || self.working.is_pending() {
            return;
        }
        self.of = Some(key);
        let names = history.shown.names();
        let mut commits: Vec<ObjectId> =
            shown.iter().map(|listed| listed.pipeline.commit).collect();
        commits.sort_unstable();
        commits.dedup();
        if commits.is_empty() {
            self.found = Arc::from([]);
            return;
        }
        self.working.request(jobs::blocking(async move {
            let width = names.width();
            let mut found = Vec::new();
            for (start, chunk) in names.chunks() {
                for (row, name) in (start..).zip(chunk.chunks_exact(width)) {
                    if let Ok(at) = commits.binary_search_by(|id| id.as_bytes().cmp(name))
                        && let Some(&id) = commits.get(at)
                    {
                        found.push((id, row));
                    }
                }
            }
            found.sort_unstable();
            // A commit in two rows (never, but a history read oddly): its first.
            found.dedup_by_key(|(id, _)| *id);
            Ok(Arc::from(found))
        }));
    }

    /// `commit`'s row in the history, if it holds it.
    fn row_of(&self, commit: &ObjectId) -> Option<u64> {
        self.found
            .binary_search_by(|(id, _)| id.cmp(commit))
            .ok()
            .and_then(|at| self.found.get(at))
            .map(|&(_, row)| row)
    }
}

/// Draws the list; returns what it asks of the history view.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &mut HistoryState,
    view: &mut PipelinesView,
) -> Option<PipelinesEvent> {
    let forge = shared.forge;
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(PipelinesEvent::Close);
    let words = CiWords::of(forge.kind_of(view.project));
    if let Some(asked) = keys(ui, view) {
        event = Some(asked);
    }
    if let Some(asked) = bar(ui, shared, words, view) {
        event = Some(asked);
    }
    let mut last_in_view = None;
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| {
            if view.shown.is_empty() {
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.vertical(|ui| empty(ui, shared, words, view));
                });
                return;
            }
            let (asked, last) = list(ui, shared, history, view);
            last_in_view = last;
            if let Some(asked) = asked {
                event = Some(asked);
            }
        });
    view.follow(ui, forge, last_in_view);
    view.in_history.follow(history, (&view.shown, view.version));
    event
}

/// ↑ and ↓ step through the pipelines, Enter shows the one selected, while no text field has
/// the keys.
fn keys(ui: &Ui, view: &mut PipelinesView) -> Option<PipelinesEvent> {
    // Not while a field or a dialog has the keys.
    let dialog = ui.ctx().memory(|memory| memory.top_modal_layer().is_some());
    if ui.ctx().text_edit_focused() || dialog || view.shown.is_empty() {
        return None;
    }
    let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
    let at = view.selected.and_then(|id| {
        view.shown
            .iter()
            .position(|listed| listed.pipeline.id == id)
    });
    if pressed(Key::Enter) {
        let listed = view.shown.get(at?)?;
        return Some(show_pipeline(view.project, listed));
    }
    let to = if pressed(Key::ArrowUp) {
        at.map_or(0, |at| at.saturating_sub(1))
    } else if pressed(Key::ArrowDown) {
        at.map_or(0, |at| {
            at.saturating_add(1).min(view.shown.len().saturating_sub(1))
        })
    } else {
        return None;
    };
    view.selected = view.shown.get(to).map(|listed| listed.pipeline.id);
    view.reveal = true;
    None
}

/// The pipeline's page, asked for.
fn show_pipeline(project: usize, listed: &ListedPipeline) -> PipelinesEvent {
    PipelinesEvent::Show(Box::new(CiMark {
        repository: project,
        pipeline: listed.pipeline.clone(),
        others: Vec::new(),
    }))
}

/// The bar: back, which project (a choice where there are several), how many were read,
/// reading, Refresh, and its forge's page of them.
fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    words: CiWords,
    view: &mut PipelinesView,
) -> Option<PipelinesEvent> {
    let forge = shared.forge;
    let palette = shared.palette;
    let reading = view.reading.is_pending();
    let mut event = None;
    let mut chosen = None;
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked() {
                event = Some(PipelinesEvent::Close);
            }
            ui.label(RichText::new(words.list).color(palette.muted));
            let name = |at: usize| {
                forge
                    .repositories
                    .get(at)
                    .map_or_default(egit_forge::ForgeRepository::full_name)
            };
            if forge.repositories.len() > 1 {
                let label = ui.label(muted(ui, "of"));
                egui::ComboBox::from_id_salt(("pipelines-project", shared.id))
                    .icon(widgets::combo_caret)
                    .selected_text(name(view.project))
                    .show_ui(ui, |ui| {
                        for at in 0..forge.repositories.len() {
                            if ui.selectable_label(at == view.project, name(at)).clicked() {
                                chosen = Some(at);
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id);
            } else {
                ui.add(
                    egui::Label::new(RichText::new(name(view.project)).color(palette.text))
                        .truncate(),
                );
            }
            let read = u64::try_from(view.shown.len()).unwrap_or(u64::MAX);
            if view.first_landed && read > 0 {
                let said = if view.next.is_some() {
                    format!("the newest {}", format::count(read))
                } else {
                    format!("all {}", format::count(read))
                };
                ui.label(muted(ui, said));
            }
            // Always pressable: a read under way reads again after it.
            if widgets::secondary(ui, true, "Refresh").clicked() {
                view.refresh(forge);
            }
            if let Some(repository) = forge.repositories.get(view.project) {
                let open = format!("{}  Open on {}", icon::ARROW_SQUARE_OUT, words.forge);
                if widgets::secondary(ui, true, &open).clicked() {
                    event = Some(PipelinesEvent::Open(repository.pipelines_url()));
                }
            }
            // What comes and goes follows the buttons.
            if reading {
                ui.spinner()
                    .on_hover_text(format!("Reading {}…", words.nouns));
            }
            if !view.shown.is_empty()
                && let Some(problem) = &view.problem
            {
                ui.label(
                    RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, problem.title))
                        .color(palette.danger),
                )
                .on_hover_text(&problem.detail);
            }
        });
    });
    if let Some(project) = chosen {
        view.choose(project);
    }
    event
}

/// No pipelines shown: still reading, none run, none the forge shows, or why they couldn't be
/// read.
fn empty(ui: &mut Ui, shared: &Shared<'_>, words: CiWords, view: &mut PipelinesView) {
    let name = shared
        .forge
        .repositories
        .get(view.project)
        .map_or_default(egit_forge::ForgeRepository::full_name);
    if let Some(problem) = view.problem.clone() {
        widgets::problem_box(ui, &problem, |ui| {
            if widgets::secondary(ui, true, "Read again").clicked() {
                view.refresh(shared.forge);
            }
        });
    } else if view.hidden {
        let said = match shared.forge.kind_of(view.project) {
            egit_forge::ForgeKind::GitLab => format!(
                "GitLab lists no pipelines of {name} to this account: CI/CD may be off for the \
                 project, or your role there may not see them."
            ),
            egit_forge::ForgeKind::GitHub => format!(
                "GitHub shows this account no Actions in {name}: they may be off, or the \
                 account may not see them."
            ),
        };
        ui.add(egui::Label::new(said).wrap());
    } else if view.first_landed {
        ui.label(format!("{name} has no {} yet.", words.nouns));
    } else {
        widgets::working(ui, &format!("Reading {name}'s {}…", words.nouns));
    }
}

/// What a row asked.
enum RowAsk {
    /// Show its pipeline.
    Show,
    /// Select its commit in the history.
    Commit,
    Open(String),
}

/// The rows, only those in view drawn; returns what they asked, and the last in view.
fn list(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &mut HistoryState,
    view: &mut PipelinesView,
) -> (Option<PipelinesEvent>, Option<usize>) {
    let now = format::now();
    let pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
    let offset_id = Id::new(("pipelines-offset", shared.id));
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("pipelines-rows", shared.id, view.project))
        .auto_shrink([false, false]);
    if std::mem::take(&mut view.reveal)
        && let Some(at) = view.selected.and_then(|id| {
            view.shown
                .iter()
                .position(|listed| listed.pipeline.id == id)
        })
    {
        let row = f32::from(u16::try_from(at).unwrap_or(u16::MAX));
        let offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(offset_id))
            .unwrap_or(0.0);
        let top = (row - 1.0).max(0.0) * pitch;
        let bottom = (row + 2.0) * pitch - ui.available_height().max(pitch);
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if bottom > offset {
            scroll = scroll.vertical_scroll_offset(bottom);
        }
    }
    let (mut asked, mut last) = (None, None);
    let mut moving = false;
    let shown = scroll.show_rows(ui, ROW_HEIGHT, view.shown.len(), |ui, visible| {
        last = visible.end.checked_sub(1);
        // The subjects of those the history holds, as its table reads them.
        let held: Vec<ObjectId> = view
            .shown
            .get(visible.clone())
            .unwrap_or_default()
            .iter()
            .map(|listed| listed.pipeline.commit)
            .filter(|commit| view.in_history.row_of(commit).is_some())
            .collect();
        if let Some(git) = shared.git {
            history
                .summaries
                .want(git, shared.repository, &history.objects, held);
        }
        for listed in view.shown.get(visible).unwrap_or_default() {
            let commit = &listed.pipeline.commit;
            let in_history = view.in_history.row_of(commit).is_some();
            let subject = history
                .summaries
                .peek(commit)
                .map(|summary| summary.subject.to_string());
            moving |= listed.pipeline.status.is_moving();
            let selected = view.selected == Some(listed.pipeline.id);
            let said = (in_history, subject.as_deref());
            match row(ui, shared, (listed, view.project), (selected, said), now) {
                Some(RowAsk::Show) => {
                    view.selected = Some(listed.pipeline.id);
                    asked = Some(show_pipeline(view.project, listed));
                }
                Some(RowAsk::Commit) => asked = Some(PipelinesEvent::ShowCommit(*commit)),
                Some(RowAsk::Open(url)) => asked = Some(PipelinesEvent::Open(url)),
                None => {}
            }
        }
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(offset_id, shown.state.offset.y));
    // A running one's time ticks.
    if moving {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    (asked, last)
}

/// One pipeline's row (`said`: whether the history holds its commit, and the commit's
/// subject once read); returns what it asked.
fn row(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (listed, project): (&ListedPipeline, usize),
    (selected, said): (bool, (bool, Option<&str>)),
    now: u64,
) -> Option<RowAsk> {
    let pipeline = &listed.pipeline;
    let words = ci_words(pipeline.status, pipeline.warnings);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("pipelines-row", project, pipeline.id)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            format!("{}: {words}", pipeline.title()),
        )
    });
    widgets::row_fill(ui, rect, 0.0, selected, response.hovered());
    crate::help::note_topic(ui, rect, TOPIC);
    let half = ROW_HEIGHT / 2.0 - 2.0;
    let top = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 10.0, rect.top() + 2.0),
        egui::vec2(rect.width() - 20.0, half),
    );
    let bottom = egui::Rect::from_min_size(
        egui::pos2(top.left(), top.bottom()),
        egui::vec2(top.width(), half),
    );
    first_line(ui, shared.palette, (listed, &words), (top, now));
    let mut asked = second_line(ui, shared.palette, listed, (bottom, said));
    let in_history = said.0;
    response.context_menu(|ui| {
        if let Some(chosen) = row_menu(ui, shared, (pipeline, project), in_history) {
            asked = Some(chosen);
        }
    });
    if response.clicked() {
        asked = Some(RowAsk::Show);
    }
    asked
}

/// A row's first line, in `top`: how it went (`words`), its title, and what it ran for; at the
/// right how long it ran and when it was made.
fn first_line(
    ui: &mut Ui,
    palette: crate::theme::Palette,
    (listed, words): (&ListedPipeline, &str),
    (top, now): (egui::Rect, u64),
) {
    let pipeline = &listed.pipeline;
    let mut first = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(top)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let (glyph, color) = ci_look(pipeline.status, palette);
    first
        .add(egui::Label::new(RichText::new(glyph).color(color)).selectable(false))
        .on_hover_text(words);
    let height = top.height();
    first.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let age = u64::try_from(pipeline.created).map_or_else(
            |_| String::new(),
            |created| crate::blame::compact_age(created, now),
        );
        let age_width = widgets::text_width(ui, "12mo");
        widgets::column(ui, (age_width, height), Align::Max, |ui| {
            ui.add(egui::Label::new(muted(ui, age)).selectable(false));
        });
        let took_width = widgets::duration_width(ui, &egui::TextStyle::Body);
        widgets::column(ui, (took_width, height), Align::Max, |ui| {
            if let Some(took) = ran_for(listed, now) {
                ui.add(egui::Label::new(muted(ui, took)).selectable(false));
            }
        });
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(pipeline.title()).color(palette.text))
                    .selectable(false)
                    .truncate(),
            );
            if pipeline.attempt > 1 {
                widgets::chip(ui, &format!("attempt {}", pipeline.attempt), palette.muted);
            }
            if let Some(git_ref) = &pipeline.git_ref {
                let reference = format!("{} {git_ref}", icon::GIT_BRANCH);
                ui.add(
                    egui::Label::new(muted(ui, reference))
                        .selectable(false)
                        .truncate(),
                );
            }
        });
    });
}

/// A row's second line, in `bottom`: its commit, a link back to it in the history where the
/// history holds it (`said`: whether, and its subject), and the subject; at the right who
/// made it run, and how. Returns whether the link was followed.
fn second_line(
    ui: &mut Ui,
    palette: crate::theme::Palette,
    listed: &ListedPipeline,
    (bottom, (in_history, subject)): (egui::Rect, (bool, Option<&str>)),
) -> Option<RowAsk> {
    let mut second = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bottom)
            .layout(Layout::left_to_right(Align::Center)),
    );
    second.add_space(20.0);
    let mut asked = None;
    let short = listed
        .pipeline
        .commit
        .abbreviated(super::history::ID_DIGITS)
        .to_string();
    if in_history {
        let words = RichText::new(&short)
            .monospace()
            .color(second.visuals().hyperlink_color);
        let commit =
            widgets::link(&mut second, words).on_hover_text("Show the commit in the history");
        crate::help::note_topic(&second, commit.rect, TOPIC);
        if commit.clicked() {
            asked = Some(RowAsk::Commit);
        }
    } else {
        second
            .add(
                egui::Label::new(RichText::new(&short).monospace().color(palette.muted))
                    .selectable(false),
            )
            .on_hover_text("Not in the history shown: fetch it, or show every branch");
    }
    second.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let by = [
            listed.user.clone(),
            listed.source.as_deref().map(source_words),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        if !by.is_empty() {
            ui.add(egui::Label::new(muted(ui, by).small()).selectable(false));
        }
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(
                egui::Label::new(muted(ui, subject.unwrap_or_default()))
                    .selectable(false)
                    .truncate(),
            );
        });
    });
    asked
}

/// A row's menu: its page, its commit (where the history holds it), its page on its forge.
fn row_menu(
    ui: &mut Ui,
    shared: &Shared<'_>,
    (pipeline, project): (&egit_forge::Pipeline, usize),
    in_history: bool,
) -> Option<RowAsk> {
    let words = CiWords::of(shared.forge.kind_of(project));
    let show = format!("Show the {}", words.noun);
    if super::actions::item(ui, (icon::FLOW_ARROW, &show), Ok(())) {
        return Some(RowAsk::Show);
    }
    let commit = if in_history {
        Ok(())
    } else {
        Err("Not in the history shown")
    };
    if super::actions::item(ui, (icon::GIT_COMMIT, "Show the commit"), commit) {
        return Some(RowAsk::Commit);
    }
    let open = format!("Open on {}", words.forge);
    super::actions::item(ui, (icon::ARROW_SQUARE_OUT, &open), Ok(()))
        .then(|| RowAsk::Open(pipeline.url.clone()))
}

/// How long it ran, or has so far while it runs; none before it starts.
fn ran_for(listed: &ListedPipeline, now: u64) -> Option<String> {
    if let Some(seconds) = listed.duration {
        return Some(format::duration(seconds));
    }
    let started = u64::try_from(listed.started?).ok()?;
    if listed.pipeline.status.is_moving() {
        return Some(format::duration(now.saturating_sub(started)));
    }
    let finished = u64::try_from(listed.finished?).ok()?;
    Some(format::duration(finished.saturating_sub(started)))
}
