//! A pipeline's jobs as a list: each stage a card (a band with its status, name, and how many
//! jobs, over its jobs' rows), in the order the stages run, drawn as far as they show. A job's
//! row: its status, name, what is special about it (may fail, manual, how many runs), and at
//! its right what can be done to it (retry, cancel, start), its status in words, how long it
//! ran, and the mark of a log, in columns of one width on every row ([`Columns`]): they line up
//! down the list, and a job's time growing moves nothing. A click shows its log (a trigger
//! job's: the pipeline it runs, on GitLab), its menu the rest ([`job_menu`], the graph's cards'
//! too). A GitHub run's stages are its workflow's jobs, each with its matrix's beside it.

use egit_forge::{CiAction, CiStatus, Job, PipelineDetails};
use egui::{Align, CornerRadius, Id, Layout, RichText, Sense, Stroke, Ui};

use super::{CiWords, JobAsk, ci_look, duration};
use crate::format;
use crate::theme::Weighted as _;
use crate::theme::{Palette, RADIUS_BOX, icon};
use crate::widgets::{self, muted};

/// A row's height.
pub(super) const ROW_HEIGHT: f32 = 34.0;

/// The width of the column marking what a click opens (a log, a page).
const MARK: f32 = 18.0;

/// The room above a stage's card (its band starts that far down its row).
const GAP: f32 = 10.0;

/// The help topic a job's parts are explained by.
pub(super) const JOB_TOPIC: &str = "pipeline-job";

/// The columns at a job row's right, as wide on every row: its status in words (the widest
/// any job here says) and how long it ran (the longest a duration is written).
#[derive(Clone, Copy)]
struct Columns {
    status: f32,
    took: f32,
}

impl Columns {
    /// The columns for `details`' jobs.
    fn of(ui: &Ui, details: &PipelineDetails) -> Self {
        let mut words: Vec<&'static str> = Vec::new();
        for job in details.stages.iter().flat_map(|stage| &stage.jobs) {
            let word = job.status.word();
            if !words.contains(&word) {
                words.push(word);
            }
        }
        let status = words
            .iter()
            .map(|word| widgets::text_width(ui, word))
            .fold(0.0_f32, f32::max);
        Self {
            status,
            took: widgets::duration_width(ui, &egui::TextStyle::Body),
        }
    }
}

/// A row of the list.
#[derive(Clone, Copy)]
enum Row<'a> {
    /// A card's band, its top: a stage's (or a GitHub matrix's) name, how its jobs stand, and
    /// how many.
    Band(&'a str, CiStatus, usize),
    /// One of a card's jobs; whether it ends the card.
    Job(&'a Job, bool),
    /// A job that stands alone (a GitHub run's, not one of a matrix): a card of its own.
    Lone(&'a Job),
}

impl Row<'_> {
    /// How tall it is: a card's first row has the room between cards above it.
    const fn height(self) -> f32 {
        match self {
            Self::Band(..) | Self::Job(..) => ROW_HEIGHT,
            Self::Lone(_) => ROW_HEIGHT + GAP,
        }
    }
}

/// The list's rows: each stage a card, its band over its jobs; a GitHub run's job that stands
/// alone (a stage of one, of its own name: no stage at all) a card of its own.
fn card_rows(details: &PipelineDetails) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for stage in &details.stages {
        let lone = !details.ordered_stages
            && stage.jobs.len() == 1
            && stage.jobs.first().is_some_and(|job| job.name == stage.name);
        if lone {
            rows.extend(stage.jobs.iter().map(Row::Lone));
            continue;
        }
        rows.push(Row::Band(&stage.name, stage.status, stage.jobs.len()));
        let last = stage.jobs.len().saturating_sub(1);
        rows.extend(
            stage
                .jobs
                .iter()
                .enumerate()
                .map(|(at, job)| Row::Job(job, at == last)),
        );
    }
    rows
}

/// A job's look: its status's, but an allowed failure's a warning (as its forge shows it).
pub(super) const fn job_look(job: &Job, palette: Palette) -> (&'static str, egui::Color32) {
    if job.allow_failure && matches!(job.status, egit_forge::CiStatus::Failed) {
        (icon::WARNING_CIRCLE, palette.warn)
    } else {
        ci_look(job.status, palette)
    }
}

/// The stages in the order they run, each a card of its jobs; returns what a job asked.
pub(super) fn stages(
    ui: &mut Ui,
    (palette, said): (Palette, CiWords),
    details: &PipelineDetails,
) -> Option<JobAsk> {
    let rows = card_rows(details);
    if rows.is_empty() {
        ui.label(muted(ui, "It has no jobs."));
        return None;
    }
    let columns = Columns::of(ui, details);
    let mut asked = None;
    let total: f32 = rows.iter().map(|row| row.height()).sum();
    egui::ScrollArea::vertical()
        .id_salt("pipeline-jobs")
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            ui.set_height(total);
            // Rows touch: the cards' edges join from one to the next.
            ui.spacing_mut().item_spacing.y = 0.0;
            // Only the rows in view are drawn, from the first one reaching into it.
            let mut top = 0.0;
            let mut first = rows.len();
            for (place, row) in rows.iter().enumerate() {
                if top + row.height() > viewport.min.y {
                    first = place;
                    break;
                }
                top += row.height();
            }
            ui.add_space(top);
            let mut y = top;
            for (place, row) in rows.iter().enumerate().skip(first) {
                if y > viewport.max.y {
                    break;
                }
                y += row.height();
                let row_asked = match *row {
                    Row::Band(name, status, jobs) => {
                        stage_band(ui, palette, (name, status, jobs));
                        None
                    }
                    Row::Job(job, last) => {
                        let corners = if last { Corners::Last } else { Corners::Middle };
                        job_row(ui, (palette, said), (job, corners), (place, columns))
                    }
                    Row::Lone(job) => {
                        ui.add_space(GAP);
                        job_row(ui, (palette, said), (job, Corners::Alone), (place, columns))
                    }
                };
                if row_asked.is_some() {
                    asked = row_asked;
                }
            }
        });
    asked
}

/// Which corners of a job's row are rounded: none in a card's middle, the bottom ones ending
/// it, all of a card of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Corners {
    Middle,
    Last,
    Alone,
}

/// A card's band, its top: how its jobs stand, its stage's name, and how many jobs it holds.
fn stage_band(ui: &mut Ui, palette: Palette, (name, status, jobs): (&str, CiStatus, usize)) {
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
    let (glyph, color) = ci_look(status, palette);
    inside.label(RichText::new(glyph).color(color));
    inside.add(
        egui::Label::new(RichText::new(name).semibold())
            .selectable(false)
            .truncate(),
    );
    inside.label(muted(
        &inside,
        format!("{jobs} job{}", if jobs == 1 { "" } else { "s" }),
    ));
}

/// A job's row, inside its stage's card (its corners as `corners` says), or a card itself: its
/// status, name, chips, and at its right its actions, status in words, duration, and a log's
/// mark; returns what it asked.
fn job_row(
    ui: &mut Ui,
    (palette, said): (Palette, CiWords),
    (job, corners): (&Job, Corners),
    (place, columns): (usize, Columns),
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
    let corners = match corners {
        Corners::Middle => CornerRadius::ZERO,
        Corners::Last => CornerRadius {
            nw: 0,
            ne: 0,
            sw: RADIUS_BOX,
            se: RADIUS_BOX,
        },
        Corners::Alone => CornerRadius::same(RADIUS_BOX),
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
    let mut asked =
        row_contents(ui, (palette, said), (job, rect), (place, columns)).map(JobAsk::Act);
    let response = response.on_hover_text(if job.id.is_some() {
        "Show its log, as it runs"
    } else if job.url.is_some() {
        "Open the pipeline it runs on GitLab"
    } else {
        "Its forge gives no page for it"
    });
    response.context_menu(|ui| {
        if let Some(menu_asked) = job_menu(ui, job, said) {
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
    (palette, said): (Palette, CiWords),
    (job, rect): (&Job, egui::Rect),
    (place, columns): (usize, Columns),
) -> Option<CiAction> {
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let mut asked = None;
    row.add_space(20.0);
    let (glyph, color) = job_look(job, palette);
    row.label(RichText::new(glyph).color(color));
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let mark = if job.id.is_some() {
            icon::TERMINAL_WINDOW
        } else if job.url.is_some() {
            icon::ARROW_SQUARE_OUT
        } else {
            ""
        };
        widgets::column(ui, (MARK, ROW_HEIGHT), Align::Max, |ui| {
            ui.label(muted(ui, mark));
        });
        widgets::column(ui, (columns.took, ROW_HEIGHT), Align::Max, |ui| {
            ui.label(muted(ui, job.duration.map_or_default(duration)));
        });
        widgets::column(ui, (columns.status, ROW_HEIGHT), Align::Min, |ui| {
            ui.label(RichText::new(job.status.word()).color(color));
        });
        // What comes and goes (as the job moves on) in a `Ui` of its own, as wide on every row.
        ui.scope_builder(
            egui::UiBuilder::new()
                .id(Id::new(("pipeline-job-actions", place)))
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.set_min_width(crate::theme::CONTROL);
                asked = quick_actions(ui, job, said);
            },
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
        widgets::chip(ui, "may fail", palette.muted);
    }
    if job.manual {
        widgets::chip(ui, "manual", palette.muted);
    }
    if !job.earlier.is_empty() {
        widgets::chip(
            ui,
            &format!("{} runs", job.earlier.len().saturating_add(1)),
            palette.accent,
        );
    }
}

/// The buttons doing what can be done to a job (laid right to left): start a manual one,
/// cancel one under way, retry one that ended; none where the account may not.
pub(super) fn quick_actions(ui: &mut Ui, job: &Job, said: CiWords) -> Option<CiAction> {
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
        said.retry_verb,
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
/// page on its forge (in `said`'s words).
pub(super) fn job_menu(ui: &mut Ui, job: &Job, said: CiWords) -> Option<JobAsk> {
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
                    .map_or_default(|started| format::ago(started, now));
                let words = format!(
                    "Run #{} · {}{} · {when}",
                    run.id,
                    run.status.word(),
                    run.duration
                        .map_or_default(|took| format!(" in {}", duration(took)))
                );
                let (glyph, _) = ci_look(run.status, Palette::of(ui.ctx()));
                if super::super::actions::item_of(ui, (glyph, &words), Ok(()), JOB_TOPIC) {
                    asked = Some(JobAsk::Log(job.clone(), run.id));
                }
            }
        });
    }
    if let Some((artifact, _)) =
        super::artifact_items(ui, job.artifacts.iter().map(|artifact| (artifact, None)))
    {
        asked = Some(JobAsk::Download(artifact, job.name.clone()));
    }
    if let Some(id) = job.id {
        let why_not = if job.can_update {
            Ok(())
        } else {
            Err("Your role in the repository may not run its pipelines")
        };
        if job.actions.retry && item(ui, (icon::ARROW_CLOCKWISE, said.retry_job), why_not) {
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
        && item(
            ui,
            (icon::ARROW_SQUARE_OUT, &format!("Open on {}", said.forge)),
            Ok(()),
        )
    {
        asked = Some(JobAsk::Open(url.clone()));
    }
    asked
}
