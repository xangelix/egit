//! A pipeline's jobs as a list: each stage a card (a band with its status, name, and how many
//! jobs, over its jobs' rows), in the order the stages run, drawn as far as they show. A job's
//! row: its status, name, what is special about it (may fail, manual, how many runs), and at
//! its right what can be done to it (retry, cancel, start), its status in words, how long it
//! ran, and the mark of a log; a click shows its log (a trigger job's: the pipeline it runs, on
//! GitLab), its menu the rest ([`job_menu`], the graph's cards' too).

use egit_forge::{CiAction, Job, PipelineDetails, Stage};
use egui::{Align, Color32, CornerRadius, Frame, Id, Layout, Margin, RichText, Sense, Stroke, Ui};

use super::{JobAsk, ci_look, duration};
use crate::format;
use crate::theme::Weighted as _;
use crate::theme::{Palette, RADIUS_BOX, icon};
use crate::widgets::{self, muted};

/// A row's height.
pub(super) const ROW_HEIGHT: f32 = 34.0;

/// The room above a stage's card (its band starts that far down its row).
const GAP: f32 = 10.0;

/// The help topic a job's parts are explained by.
pub(super) const JOB_TOPIC: &str = "pipeline-job";

/// A row of the list.
#[derive(Clone, Copy)]
enum Row<'a> {
    /// A stage's band: the top of its card.
    Stage(&'a Stage),
    /// One of its jobs; whether it ends the card.
    Job(&'a Job, bool),
}

/// The stages in the order they run, each a card of its jobs; returns what a job asked.
pub(super) fn stages(ui: &mut Ui, palette: Palette, details: &PipelineDetails) -> Option<JobAsk> {
    let rows: Vec<Row<'_>> = details
        .stages
        .iter()
        .flat_map(|stage| {
            let last = stage.jobs.len().saturating_sub(1);
            std::iter::once(Row::Stage(stage)).chain(
                stage
                    .jobs
                    .iter()
                    .enumerate()
                    .map(move |(at, job)| Row::Job(job, at == last)),
            )
        })
        .collect();
    if rows.is_empty() {
        ui.label(muted(ui, "It has no jobs."));
        return None;
    }
    let mut asked = None;
    egui::ScrollArea::vertical()
        .id_salt("pipeline-jobs")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, visible| {
            // Rows touch: the cards' edges join from one to the next.
            ui.spacing_mut().item_spacing.y = 0.0;
            for (at, row) in rows
                .get(visible.clone())
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let place = visible.start.saturating_add(at);
                match row {
                    Row::Stage(stage) => stage_band(ui, palette, stage),
                    Row::Job(job, last) => {
                        if let Some(job_asked) = job_row(ui, palette, (job, *last), place) {
                            asked = Some(job_asked);
                        }
                    }
                }
            }
        });
    asked
}

/// A stage's band, the top of its card: its status, name, and how many jobs it has.
fn stage_band(ui: &mut Ui, palette: Palette, stage: &Stage) {
    let (row, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let band = egui::Rect::from_min_max(egui::pos2(row.left(), row.top() + GAP), row.max);
    let corners = CornerRadius {
        nw: RADIUS_BOX,
        ne: RADIUS_BOX,
        sw: 0,
        se: 0,
    };
    ui.painter().rect(
        band,
        corners,
        palette.surface_alt,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
    crate::help::note_topic(ui, band, super::TOPIC);
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(band.shrink2(egui::vec2(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let (glyph, color) = ci_look(stage.status, palette);
    inside.label(RichText::new(glyph).color(color));
    inside.add(
        egui::Label::new(RichText::new(&stage.name).semibold())
            .selectable(false)
            .truncate(),
    );
    let jobs = stage.jobs.len();
    inside.label(muted(
        &inside,
        format!("{jobs} job{}", if jobs == 1 { "" } else { "s" }),
    ));
}

/// A job's row, inside its stage's card (`last`: the card's last, its corners rounded): its
/// status, name, chips, and at its right its actions, status in words, duration, and a log's
/// mark; returns what it asked.
fn job_row(
    ui: &mut Ui,
    palette: Palette,
    (job, last): (&Job, bool),
    place: usize,
) -> Option<JobAsk> {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let opens = job.id.is_some() || job.url.is_some();
    let response = ui.interact(
        rect,
        Id::new(("pipeline-job", place)),
        if opens {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let words = job.status.word();
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            opens,
            format!("{}: {words}", job.name),
        )
    });
    crate::help::note_topic(ui, rect, JOB_TOPIC);
    let corners = if last {
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: RADIUS_BOX,
            se: RADIUS_BOX,
        }
    } else {
        CornerRadius::ZERO
    };
    let fill = if response.hovered() {
        palette.surface_alt
    } else {
        palette.surface
    };
    ui.painter().rect(
        rect,
        corners,
        fill,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
    let mut asked = row_contents(ui, palette, (job, rect), place).map(JobAsk::Act);
    let response = response.on_hover_text(if job.id.is_some() {
        "Show its log, as it runs"
    } else if job.url.is_some() {
        "Open the pipeline it runs on GitLab"
    } else {
        "GitLab gives no page for it"
    });
    response.context_menu(|ui| {
        if let Some(menu_asked) = job_menu(ui, job) {
            asked = Some(menu_asked);
        }
    });
    if response.clicked() {
        asked = clicked(job);
    }
    asked
}

/// What a job's row shows inside `rect`: its status, name, and chips, and at its right its
/// actions, status in words, duration, and a log's mark; returns the action asked.
fn row_contents(
    ui: &mut Ui,
    palette: Palette,
    (job, rect): (&Job, egui::Rect),
    place: usize,
) -> Option<CiAction> {
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let mut asked = None;
    row.add_space(20.0);
    let (glyph, color) = ci_look(job.status, palette);
    row.label(RichText::new(glyph).color(color));
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if job.id.is_some() {
            ui.label(muted(ui, icon::TERMINAL_WINDOW));
        } else if job.url.is_some() {
            ui.label(muted(ui, icon::ARROW_SQUARE_OUT));
        }
        let took = job.duration.map(duration).unwrap_or_default();
        ui.add_sized(
            [72.0, ROW_HEIGHT],
            egui::Label::new(muted(ui, took)).selectable(false),
        );
        ui.add_sized(
            [110.0, ROW_HEIGHT],
            egui::Label::new(RichText::new(job.status.word()).color(color)).selectable(false),
        );
        // What comes and goes (as the job moves on) in a `Ui` of its own.
        ui.scope_builder(
            egui::UiBuilder::new()
                .id(Id::new(("pipeline-job-actions", place)))
                .layout(Layout::right_to_left(Align::Center)),
            |ui| asked = quick_actions(ui, job),
        );
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(&job.name).color(palette.text))
                    .selectable(false)
                    .truncate(),
            );
            chips(ui, palette, job);
        });
    });
    asked
}

/// What a job's click asks: its latest run's log, or a trigger job's pipeline on GitLab.
pub(super) fn clicked(job: &Job) -> Option<JobAsk> {
    job.id.map_or_else(
        || job.url.clone().map(JobAsk::Open),
        |id| Some(JobAsk::Log(job.clone(), id)),
    )
}

/// The chips after a job's name: may fail, manual, how many times it ran.
fn chips(ui: &mut Ui, palette: Palette, job: &Job) {
    if job.allow_failure {
        chip(ui, "may fail", palette.muted);
    }
    if job.manual {
        chip(ui, "manual", palette.muted);
    }
    if !job.earlier.is_empty() {
        chip(
            ui,
            &format!("{} runs", job.earlier.len().saturating_add(1)),
            palette.accent,
        );
    }
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

/// The buttons doing what can be done to a job (laid right to left): start a manual one,
/// cancel one under way, retry one that ended; none where the account may not.
pub(super) fn quick_actions(ui: &mut Ui, job: &Job) -> Option<CiAction> {
    let id = job.id?;
    if !job.can_update {
        return None;
    }
    let mut button = |shown: bool, glyph: &str, verb: &str, hover: &str| {
        shown
            && widgets::icon_tool_of(
                ui,
                (glyph, &format!("{verb} {}", job.name)),
                "pipeline-actions",
            )
            .on_hover_text(hover)
            .clicked()
    };
    let play = button(
        job.actions.play,
        icon::PLAY,
        "Start",
        "Start this manual job",
    );
    let cancel = button(
        job.actions.cancel,
        icon::PROHIBIT,
        "Cancel",
        "Cancel this job",
    );
    let retry = button(
        job.actions.retry,
        icon::ARROW_CLOCKWISE,
        "Retry",
        "Run this job again: a new run in its place, this one kept",
    );
    [
        (play, CiAction::PlayJob(id)),
        (cancel, CiAction::CancelJob(id)),
        (retry, CiAction::RetryJob(id)),
    ]
    .into_iter()
    .find_map(|(clicked, action)| clicked.then_some(action))
}

/// A job's menu: its log, its earlier runs' logs, retrying, cancelling, or starting it, and its
/// page on GitLab.
pub(super) fn job_menu(ui: &mut Ui, job: &Job) -> Option<JobAsk> {
    use super::super::actions::{item, menu_header, menu_rows, submenu};
    menu_rows(ui);
    let mut asked = job
        .id
        .filter(|_| item(ui, (icon::TERMINAL_WINDOW, "Show its log"), Ok(())))
        .map(|id| JobAsk::Log(job.clone(), id));
    if !job.earlier.is_empty() {
        submenu(ui, (icon::CLOCK_COUNTER_CLOCKWISE, "Earlier runs"), |ui| {
            menu_rows(ui);
            menu_header(ui, "Their logs");
            let now = format::now();
            for run in &job.earlier {
                let when = run
                    .started
                    .and_then(|started| u64::try_from(started).ok())
                    .map(|started| format::ago(started, now))
                    .unwrap_or_default();
                let words = format!(
                    "Run #{} · {}{} · {when}",
                    run.id,
                    run.status.word(),
                    run.duration
                        .map(|took| format!(" in {}", duration(took)))
                        .unwrap_or_default()
                );
                let (glyph, _) = ci_look(run.status, Palette::of(ui.ctx()));
                if super::super::actions::item_of(ui, (glyph, &words), Ok(()), JOB_TOPIC) {
                    asked = Some(JobAsk::Log(job.clone(), run.id));
                }
            }
        });
    }
    if let Some(id) = job.id {
        let why_not = if job.can_update {
            Ok(())
        } else {
            Err("Your role on this project may not run its pipelines")
        };
        if job.actions.retry && item(ui, (icon::ARROW_CLOCKWISE, "Retry the job"), why_not) {
            asked = Some(JobAsk::Act(CiAction::RetryJob(id)));
        }
        if job.actions.cancel && item(ui, (icon::PROHIBIT, "Cancel the job"), why_not) {
            asked = Some(JobAsk::Act(CiAction::CancelJob(id)));
        }
        if job.actions.play && item(ui, (icon::PLAY, "Start the job"), why_not) {
            asked = Some(JobAsk::Act(CiAction::PlayJob(id)));
        }
    }
    if let Some(url) = &job.url
        && item(ui, (icon::ARROW_SQUARE_OUT, "Open on GitLab"), Ok(()))
    {
        asked = Some(JobAsk::Open(url.clone()));
    }
    asked
}
