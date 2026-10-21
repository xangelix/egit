//! A GitLab pipeline in the commit table's place, from the mark on its commit's row: how it
//! stands (its status, branch or tag, commit, who started it and how, when, for how long), then
//! its stages in the order they run, each a list of its jobs with theirs. That is as far as egit
//! goes: a job opens its page on GitLab, in the browser, for its log and the rest. Read again
//! every few seconds while it may still change by itself.
//!
//! Where the pipelines couldn't be read, a notice under the commit table says why ([`unread`]).
//!
//! Every status shows as one of five, by colour and by icon alike (for those who don't tell
//! the colours apart): passed green, failed red, under way blue, waiting for someone (or for its
//! time) orange, cancelled or skipped grey.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_forge::{CiGroup, CiStatus, Job, PipelineDetails, Stage, pipeline_details};
use egit_git::{CancelToken, ObjectId};
use egui::{Align, Color32, Frame, Id, Key, Layout, Margin, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;

use super::history::Shared;
use crate::ci::CiMark;
use crate::forge::RepoForge;
use crate::format;
use crate::jobs;
use crate::problem::Problem;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// A row's height in the list of stages and jobs.
const ROW_HEIGHT: f32 = 28.0;

/// How soon a pipeline that may still change is read again.
const READ_AGAIN: Duration = Duration::from_secs(5);

/// The help topic the page's parts are explained by.
const TOPIC: &str = "pipeline-page";

/// A status's icon and colour: its group's colour, its own icon.
pub(super) const fn ci_look(status: CiStatus, palette: Palette) -> (&'static str, Color32) {
    let color = match status.group() {
        CiGroup::Success => palette.ok,
        CiGroup::Failed => palette.danger,
        CiGroup::Running => palette.running,
        CiGroup::Paused => palette.paused,
        CiGroup::Canceled => palette.muted,
    };
    let glyph = match status {
        CiStatus::Success => icon::CHECK_CIRCLE,
        CiStatus::Failed => icon::X_CIRCLE,
        CiStatus::Running => icon::CIRCLE_HALF,
        CiStatus::Created
        | CiStatus::WaitingForResource
        | CiStatus::Preparing
        | CiStatus::WaitingForCallback
        | CiStatus::Pending => icon::CLOCK,
        CiStatus::Manual => icon::PAUSE_CIRCLE,
        CiStatus::Scheduled => icon::CLOCK_COUNTDOWN,
        CiStatus::Canceling | CiStatus::Canceled => icon::PROHIBIT,
        CiStatus::Skipped => icon::SKIP_FORWARD_CIRCLE,
        CiStatus::Unknown => icon::MINUS_CIRCLE,
    };
    (glyph, color)
}

/// A pipeline's status in words: "passed", "passed with warnings", "waiting for someone".
pub(super) fn ci_words(status: CiStatus, warnings: bool) -> String {
    match status {
        CiStatus::Success if warnings => "passed with warnings".to_owned(),
        CiStatus::Manual => "waiting for someone to start a job".to_owned(),
        CiStatus::Scheduled => "waiting for a delayed job's time".to_owned(),
        other => other.word().to_owned(),
    }
}

/// What the commit table's mark asked.
pub(super) enum MarkEvent {
    /// The pipeline's page, in the commit table's place.
    Show(CiMark),
    /// Its page on GitLab.
    Open(String),
}

/// The width a pipeline's mark takes in a commit's row (room kept on every row while any has
/// one, so the commits' names line up).
pub(super) const MARK_WIDTH: f32 = 24.0;

/// A commit's pipeline as a mark in its row: an icon in its status's colour, its pipeline's
/// number and status on hover; a click shows the pipeline, its menu opens it on GitLab. `None`
/// keeps the room.
pub(super) fn mark(
    ui: &mut Ui,
    mark: Option<&CiMark>,
    commit: ObjectId,
    palette: Palette,
) -> Option<MarkEvent> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(MARK_WIDTH, MARK_WIDTH), Sense::hover());
    let mark = mark?;
    let pipeline = &mark.pipeline;
    let response = ui.interact(rect, Id::new(("pipeline-mark", commit)), Sense::click());
    let said = format!(
        "Pipeline #{}: {}",
        pipeline.id,
        ci_words(pipeline.status, pipeline.warnings)
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &said));
    crate::help::note_topic(ui, rect, "pipeline-mark");
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            crate::theme::RADIUS_INNER,
            ui.style().interact(&response).weak_bg_fill,
        );
    }
    let (glyph, color) = ci_look(pipeline.status, palette);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(15.0),
        color,
    );
    let mut event = None;
    let response = response.on_hover_ui(|ui| {
        ui.label(&said);
        if let Some(git_ref) = &pipeline.git_ref {
            ui.label(muted(ui, git_ref));
        }
        ui.label(muted(ui, "Click to see its stages and jobs"));
    });
    if response.clicked() {
        event = Some(MarkEvent::Show(mark.clone()));
    }
    response.context_menu(|ui| {
        super::actions::menu_rows(ui);
        if super::actions::item(ui, (icon::FLOW_ARROW, "Show the pipeline"), Ok(())) {
            event = Some(MarkEvent::Show(mark.clone()));
        }
        if super::actions::item(ui, (icon::ARROW_SQUARE_OUT, "Open on GitLab"), Ok(())) {
            event = Some(MarkEvent::Open(pipeline.url.clone()));
        }
    });
    event
}

/// The pipeline page's state.
#[derive(Debug)]
pub(super) struct PipelineView {
    /// The pipeline, as its mark had it (shown until its details land), and its repository.
    mark: CiMark,
    details: Bind<Arc<PipelineDetails>, Problem>,
    /// When it is read again (while it may still change).
    next: Option<Instant>,
    /// Whether a read is due at once (the first, or Refresh).
    due: bool,
    /// Whether the last read's end was taken in (when the next is due worked out).
    landed: bool,
    cancel: CancelToken,
}

impl Drop for PipelineView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the page asks of the history view.
pub(super) enum PipelineEvent {
    Close,
    ShowCommit(ObjectId),
    Open(String),
}

impl PipelineView {
    /// The page of `mark`'s pipeline, read at once.
    pub(super) fn new(mark: CiMark) -> Self {
        Self {
            mark,
            details: Bind::new(true),
            next: None,
            due: true,
            landed: false,
            cancel: CancelToken::new(),
        }
    }

    /// Reads it (again) when due; schedules the next read while it may still change.
    fn follow(&mut self, ui: &Ui, forge: &RepoForge) {
        let now = Instant::now();
        if self.details.is_pending() {
            return;
        }
        if !self.landed
            && let Some(read) = self.details.read_as_ref()
        {
            self.landed = true;
            // One that failed is tried again in a while: a network gone may come back.
            let moving = read.map_or(true, |details| {
                details.pipeline.status.is_moving()
                    || details
                        .stages
                        .iter()
                        .flat_map(|stage| &stage.jobs)
                        .any(|job| job.status.is_moving())
            });
            self.next = moving.then(|| now.checked_add(READ_AGAIN)).flatten();
        }
        let due = std::mem::take(&mut self.due) || self.next.is_some_and(|next| now >= next);
        if due {
            self.next = None;
            self.read(forge);
        } else if let Some(next) = self.next {
            ui.ctx()
                .request_repaint_after(next.saturating_duration_since(now));
        }
    }

    /// Starts reading it.
    fn read(&mut self, forge: &RepoForge) {
        self.landed = false;
        let repository = forge.repositories.get(self.mark.repository).cloned();
        let account = repository
            .as_ref()
            .and_then(|repository| forge.account(repository.host()));
        let (Some(repository), Some(account)) = (repository, account) else {
            self.details.fill(Err(Problem::new(
                "The pipeline couldn't be read",
                "Its project is no longer among the remotes.",
            )));
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        let cancel = self.cancel.clone();
        let iid = self.mark.pipeline.iid;
        self.details.request(jobs::blocking(move || {
            pipeline_details(&account, &repository, iid, &cancel)
                .map(Arc::new)
                .map_err(|error| Problem::new("The pipeline couldn't be read", error.to_string()))
        }));
    }
}

/// A row of the list: a stage's header, or one of its jobs.
#[derive(Clone, Copy)]
enum Row<'a> {
    Stage(&'a Stage),
    Job(&'a Job),
}

/// Draws the pipeline's page; returns what it asks of the history view.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut PipelineView,
) -> Option<PipelineEvent> {
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(PipelineEvent::Close);
    view.follow(ui, shared.forge);
    let reading = view.details.is_pending();
    let details = view
        .details
        .read_as_ref()
        .map(|read| read.map(Arc::clone).map_err(Clone::clone));
    let (details, problem) = match details {
        Some(Ok(details)) => (Some(details), None),
        Some(Err(problem)) => (None, Some(problem)),
        None => (None, None),
    };
    let pipeline = details
        .as_ref()
        .map_or(&view.mark.pipeline, |details| &details.pipeline);
    match bar(ui, shared, pipeline, reading) {
        Some(BarAsk::Refresh) => view.due = true,
        Some(BarAsk::Close) => event = Some(PipelineEvent::Close),
        Some(BarAsk::Open(url)) => event = Some(PipelineEvent::Open(url)),
        None => {}
    }
    let palette = shared.palette;
    egui::CentralPanel::default()
        .frame(Frame::new().inner_margin(Margin::same(16)))
        .show(ui, |ui| {
            if let Some(asked) = summary(ui, palette, pipeline, details.as_deref()) {
                event = Some(asked);
            }
            ui.add_space(12.0);
            match (&details, &problem) {
                (Some(details), _) => {
                    if let Some(asked) = stages(ui, palette, details) {
                        event = Some(asked);
                    }
                }
                (None, Some(problem)) => {
                    widgets::problem_box(ui, problem, |ui| {
                        let again = widgets::secondary(ui, true, "Try again");
                        if again.clicked() {
                            view.due = true;
                        }
                    });
                }
                (None, None) => widgets::working(ui, "Reading its stages and jobs…"),
            }
        });
    event
}

/// What the bar asked.
enum BarAsk {
    Close,
    Refresh,
    Open(String),
}

/// The bar: back, the pipeline's number and status, Refresh (a spinner while reading), and its
/// page on GitLab.
fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    pipeline: &egit_forge::Pipeline,
    reading: bool,
) -> Option<BarAsk> {
    let palette = shared.palette;
    let mut event = None;
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked() {
                event = Some(BarAsk::Close);
            }
            let title = ui.label(RichText::new(format!("Pipeline #{}", pipeline.id)).semibold());
            crate::help::note_topic(ui, title.rect, TOPIC);
            let (glyph, color) = ci_look(pipeline.status, palette);
            ui.label(
                RichText::new(format!(
                    "{glyph}  {}",
                    ci_words(pipeline.status, pipeline.warnings)
                ))
                .color(color),
            );
            if widgets::secondary(ui, !reading, "Refresh").clicked() {
                event = Some(BarAsk::Refresh);
            }
            if widgets::secondary(
                ui,
                true,
                &format!("{}  Open on GitLab", icon::ARROW_SQUARE_OUT),
            )
            .clicked()
            {
                event = Some(BarAsk::Open(pipeline.url.clone()));
            }
            // What comes and goes follows the buttons.
            ui.scope_builder(
                egui::UiBuilder::new().id(Id::new("pipeline-reading")),
                |ui| {
                    if reading {
                        ui.spinner();
                    }
                },
            );
        });
    });
    event
}

/// How it stands: its branch or tag, its commit (and the way to it in the history), who
/// started it and how, when, and how long it ran (or has run so far).
fn summary(
    ui: &mut Ui,
    palette: Palette,
    pipeline: &egit_forge::Pipeline,
    details: Option<&PipelineDetails>,
) -> Option<PipelineEvent> {
    let mut event = None;
    let now = format::now();
    let width = widgets::FACT_LABEL;
    if let Some(git_ref) = &pipeline.git_ref {
        widgets::fact_of(ui, width, ("Ran for", TOPIC), |ui| {
            ui.add(egui::Label::new(RichText::new(git_ref).monospace()).truncate());
        });
    }
    widgets::fact_of(ui, width, ("Commit", TOPIC), |ui| {
        ui.label(
            RichText::new(pipeline.commit.abbreviated(10).to_string())
                .monospace()
                .color(palette.muted),
        );
        if widgets::secondary(ui, true, "Show the commit").clicked() {
            event = Some(PipelineEvent::ShowCommit(pipeline.commit));
        }
    });
    let created = u64::try_from(pipeline.created).unwrap_or(0);
    let when = format::ago(created, now);
    let started = details
        .and_then(|details| details.user.as_deref())
        .map_or_else(|| when.clone(), |user| format!("{when} by {user}"));
    let how = details
        .and_then(|details| details.source.as_deref())
        .map(source_words);
    widgets::fact_of(ui, width, ("Started", TOPIC), |ui| {
        ui.label(started);
        if let Some(how) = how {
            ui.label(muted(ui, how));
        }
    });
    if let Some(took) = details.and_then(|details| ran_for(details, now)) {
        widgets::fact_of(ui, width, ("Ran", TOPIC), |ui| {
            ui.label(took);
        });
    }
    event
}

/// What started a pipeline, in words.
fn source_words(source: &str) -> String {
    match source {
        "push" => "on a push".to_owned(),
        "merge_request_event" => "for a merge request".to_owned(),
        "schedule" => "on a schedule".to_owned(),
        "web" => "from GitLab's page".to_owned(),
        "api" => "through the API".to_owned(),
        "trigger" => "by a trigger".to_owned(),
        "pipeline" | "parent_pipeline" => "by another pipeline".to_owned(),
        other => format!("by {}", other.replace('_', " ")),
    }
}

/// How long it ran, or has run so far ("1 min 35 s so far"); none before it starts.
fn ran_for(details: &PipelineDetails, now: u64) -> Option<String> {
    if let Some(seconds) = details.duration {
        return Some(duration(seconds));
    }
    let started = u64::try_from(details.started?).ok()?;
    if details.pipeline.status.is_moving() {
        return Some(format!("{} so far", duration(now.saturating_sub(started))));
    }
    let finished = u64::try_from(details.finished?).ok()?;
    Some(duration(finished.saturating_sub(started)))
}

/// A number of seconds as a person says it: "42 s", "1 min 35 s", "2 h 5 min".
pub(super) fn duration(seconds: u64) -> String {
    let hours = seconds.checked_div(3_600).unwrap_or(0);
    let minutes = seconds.checked_div(60).unwrap_or(0) % 60;
    let rest = seconds % 60;
    match (hours, minutes) {
        (0, 0) => format!("{rest} s"),
        (0, _) => format!("{minutes} min {rest} s"),
        _ => format!("{hours} h {minutes} min"),
    }
}

/// The stages in the order they run, each a header over its jobs, drawn as far as they show;
/// a job opens its page on GitLab.
fn stages(ui: &mut Ui, palette: Palette, details: &PipelineDetails) -> Option<PipelineEvent> {
    let rows: Vec<Row<'_>> = details
        .stages
        .iter()
        .flat_map(|stage| std::iter::once(Row::Stage(stage)).chain(stage.jobs.iter().map(Row::Job)))
        .collect();
    if rows.is_empty() {
        ui.label(muted(ui, "It has no jobs."));
        return None;
    }
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("pipeline-jobs")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, visible| {
            for (at, row) in rows
                .get(visible.clone())
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let place = visible.start.saturating_add(at);
                match row {
                    Row::Stage(stage) => stage_row(ui, palette, stage),
                    Row::Job(job) => {
                        if let Some(url) = job_row(ui, palette, job, place) {
                            event = Some(PipelineEvent::Open(url));
                        }
                    }
                }
            }
        });
    event
}

/// A stage's header: its status and name, and how many jobs it has.
fn stage_row(ui: &mut Ui, palette: Palette, stage: &Stage) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    crate::help::note_topic(ui, rect, TOPIC);
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let (glyph, color) = ci_look(stage.status, palette);
    row.label(RichText::new(glyph).color(color));
    row.add(
        egui::Label::new(RichText::new(&stage.name).semibold())
            .selectable(false)
            .truncate(),
    );
    let jobs = stage.jobs.len();
    row.label(muted(
        &row,
        format!("{jobs} job{}", if jobs == 1 { "" } else { "s" }),
    ));
}

/// A job's row, stepped in under its stage: its status, name, what is special about it, and
/// how long it ran; a click opens its page. Returns that page when clicked.
fn job_row(ui: &mut Ui, palette: Palette, job: &Job, place: usize) -> Option<String> {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let sense = if job.url.is_some() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, Id::new(("pipeline-job", place)), sense);
    let words = job.status.word();
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            job.url.is_some(),
            format!("{}: {words}", job.name),
        )
    });
    crate::help::note_topic(ui, rect, "pipeline-job");
    widgets::row_fill(
        ui,
        rect,
        f32::from(crate::theme::RADIUS_INNER),
        false,
        response.hovered(),
    );
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.add_space(24.0);
    let (glyph, color) = ci_look(job.status, palette);
    row.label(RichText::new(glyph).color(color));
    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if job.url.is_some() {
            ui.label(muted(ui, icon::ARROW_SQUARE_OUT));
        }
        let took = job.duration.map(duration).unwrap_or_default();
        ui.add_sized(
            [80.0, ROW_HEIGHT],
            egui::Label::new(muted(ui, took)).selectable(false),
        );
        ui.add_sized(
            [120.0, ROW_HEIGHT],
            egui::Label::new(RichText::new(words).color(color)).selectable(false),
        );
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(&job.name).color(palette.text))
                    .selectable(false)
                    .truncate(),
            );
            if job.allow_failure {
                chip(ui, "may fail", palette.muted);
            }
            if job.manual {
                chip(ui, "manual", palette.muted);
            }
        });
    });
    let response = response.on_hover_text(if job.url.is_some() {
        "Open the job's page on GitLab: its log, artifacts, and the rest"
    } else {
        "GitLab gives no page for it"
    });
    response.clicked().then(|| job.url.clone()).flatten()
}

/// A small label in a frame.
fn chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_INNER))
        .inner_margin(Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).selectable(false));
        });
}

/// The notice under the commit table where the pipelines couldn't be read: `why` (cut to its
/// line, whole on hover), and Dismiss; returns whether it was dismissed.
pub(super) fn unread(ui: &mut Ui, repo: crate::repo::RepoId, why: &str) -> bool {
    let palette = Palette::of(ui.ctx());
    let mut dismissed = false;
    ui.scope_builder(
        egui::UiBuilder::new().id(Id::new(("pipelines-unread", repo))),
        |ui| {
            Frame::new()
                .fill(palette.warn.gamma_multiply(0.10))
                .stroke(egui::Stroke::new(1.0, palette.warn.gamma_multiply(0.6)))
                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_BOX))
                .inner_margin(Margin::symmetric(12, 2))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    crate::help::note_topic(ui, ui.max_rect(), "pipeline-mark");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        dismissed = widgets::icon_tool(ui, true, icon::X, "Dismiss").clicked();
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{}  ", icon::WARNING)).color(palette.warn),
                            );
                            ui.add(egui::Label::new(muted(ui, why)).truncate())
                                .on_hover_text(why);
                        });
                    });
                });
        },
    );
    dismissed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_said_as_people_say_them() {
        assert_eq!(duration(42), "42 s");
        assert_eq!(duration(95), "1 min 35 s");
        assert_eq!(duration(7_500), "2 h 5 min");
    }
}
