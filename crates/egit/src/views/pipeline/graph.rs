//! A pipeline's jobs as a graph, as GitLab's and GitHub's pipeline pages draw them: each job a card (its
//! status, name, and how long it ran) in columns, by stage (`JobsLayout::Stages`: the stages in
//! the order they run, lines drawn to what each job `needs`) or by what each waits for
//! (`JobsLayout::Dependencies`: a job in the column after the furthest of those it waits for: its
//! `needs`, else every job of the stage before it). The pointer resting on a card lights what it
//! waits for and what waits for it, all the way along (a stage's waits drawn then too), and dims
//! the rest. A card's click shows its log, its menu is a row's ([`super::list::job_menu`]).
//!
//! The layout is worked out once a read of the pipeline and a layout ([`Graph`], kept by the
//! page), what the pointer lights once it rests on another card; a stage's waits are kept as
//! the stage (never a line from each of its jobs to each waiting), and only what is in view is
//! drawn.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egit_forge::{Job, PipelineDetails};
use egui::epaint::CubicBezierShape;
use egui::{Align, Color32, CornerRadius, Id, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui};

use super::list::{JOB_TOPIC, clicked, job_menu};
use super::{CiWords, JobAsk, JobsLayout, ci_look, ci_words, duration};
use crate::theme::Weighted as _;
use crate::theme::{Palette, RADIUS_CONTROL};

/// A card's size.
const CARD: egui::Vec2 = egui::vec2(220.0, 36.0);

/// The room between columns (where lines run) and between cards.
const COLUMN_GAP: f32 = 64.0;
const ROW_GAP: f32 = 10.0;

/// A stage's title's height over its column.
const TITLE: f32 = 30.0;

/// The graph of a pipeline's jobs, as last worked out, and what the pointer lights.
#[derive(Debug, Default)]
pub(super) struct Graph {
    laid: Option<Laid>,
    /// The job the pointer rested on last frame, and what that lights (it, all it waits for,
    /// all that wait for it).
    hovered: Option<usize>,
    lit: Vec<bool>,
}

/// What a job waits for.
#[derive(Debug)]
enum Wait {
    /// Its needs, by place.
    Named(Vec<usize>),
    /// Every job of a stage (the nearest before its own that has any; none for the first).
    Stage(Option<usize>),
}

/// The jobs laid out, for one read of a pipeline and one layout.
#[derive(Debug)]
struct Laid {
    details: Arc<PipelineDetails>,
    by: JobsLayout,
    /// Each job's stage and place in it, in the stages' order.
    jobs: Vec<(usize, usize)>,
    /// Each stage's jobs, a range of `jobs`.
    stages: Vec<Range<usize>>,
    waits: Vec<Wait>,
    /// Those waiting for each job by name.
    named_waiters: Vec<Vec<usize>>,
    /// Those waiting for each stage's jobs (scheduled by stage).
    stage_waiters: Vec<Vec<usize>>,
    /// Each job's column and row.
    places: Vec<(usize, usize)>,
    size: egui::Vec2,
}

impl Laid {
    fn new(details: &Arc<PipelineDetails>, by: JobsLayout) -> Self {
        let mut jobs = Vec::new();
        let mut stages = Vec::with_capacity(details.stages.len());
        for (stage, of) in details.stages.iter().enumerate() {
            let start = jobs.len();
            jobs.extend((0..of.jobs.len()).map(|place| (stage, place)));
            stages.push(start..jobs.len());
        }
        let job = |at: &(usize, usize)| job_at(details, *at);
        let by_name: HashMap<&str, usize> = jobs
            .iter()
            .enumerate()
            .filter_map(|(at, place)| Some((job(place)?.name.as_str(), at)))
            .collect();
        let stage_before = |stage: usize| {
            (0..stage)
                .rev()
                .find(|before| stages.get(*before).is_some_and(|jobs| !jobs.is_empty()))
        };
        let waits: Vec<Wait> = jobs
            .iter()
            .map(|place| {
                job(place).and_then(|job| job.needs.as_ref()).map_or_else(
                    || Wait::Stage(stage_before(place.0)),
                    |needs| {
                        Wait::Named(
                            needs
                                .iter()
                                .filter_map(|need| by_name.get(need.as_str()).copied())
                                .collect(),
                        )
                    },
                )
            })
            .collect();
        let mut named_waiters = vec![Vec::new(); jobs.len()];
        let mut stage_waiters = vec![Vec::new(); stages.len()];
        for (at, wait) in waits.iter().enumerate() {
            match wait {
                Wait::Named(on) => {
                    for from in on {
                        if let Some(waiting) = named_waiters.get_mut(*from) {
                            waiting.push(at);
                        }
                    }
                }
                Wait::Stage(Some(stage)) => {
                    if let Some(waiting) = stage_waiters.get_mut(*stage) {
                        waiting.push(at);
                    }
                }
                Wait::Stage(None) => {}
            }
        }
        let (places, size) = place(&jobs, (&stages, &waits), by);
        Self {
            details: Arc::clone(details),
            by,
            jobs,
            stages,
            waits,
            named_waiters,
            stage_waiters,
            places,
            size,
        }
    }

    /// Whether it lays out `details` (the same read) as `by` says.
    fn is_of(&self, details: &Arc<PipelineDetails>, by: JobsLayout) -> bool {
        Arc::ptr_eq(&self.details, details) && self.by == by
    }

    fn job(&self, at: usize) -> Option<&Job> {
        job_at(&self.details, *self.jobs.get(at)?)
    }

    /// The jobs lit by `hovered`: it, all it waits for, and all that wait for it, along (each
    /// job and each stage's waits walked once).
    fn lit(&self, hovered: Option<usize>) -> Vec<bool> {
        let mut lit = vec![false; self.jobs.len()];
        let Some(hovered) = hovered else {
            return lit;
        };
        let mut stage_walked = vec![false; self.stages.len()];
        let mut up = vec![hovered];
        while let Some(at) = up.pop() {
            let Some(seen) = lit.get_mut(at).filter(|seen| !**seen) else {
                continue;
            };
            *seen = true;
            match self.waits.get(at) {
                Some(Wait::Named(on)) => up.extend(on),
                Some(Wait::Stage(Some(stage))) => {
                    if let Some(walked) = stage_walked.get_mut(*stage).filter(|walked| !**walked) {
                        *walked = true;
                        up.extend(self.stages.get(*stage).cloned().unwrap_or_default());
                    }
                }
                _ => {}
            }
        }
        let mut went = vec![false; self.jobs.len()];
        let mut stage_went = vec![false; self.stages.len()];
        let mut down = vec![hovered];
        while let Some(at) = down.pop() {
            let Some(seen) = went.get_mut(at).filter(|seen| !**seen) else {
                continue;
            };
            *seen = true;
            if let Some(slot) = lit.get_mut(at) {
                *slot = true;
            }
            down.extend(self.named_waiters.get(at).into_iter().flatten());
            let stage = self.jobs.get(at).map_or(usize::MAX, |(stage, _)| *stage);
            if let Some(gone) = stage_went.get_mut(stage).filter(|gone| !**gone) {
                *gone = true;
                down.extend(self.stage_waiters.get(stage).into_iter().flatten());
            }
        }
        lit
    }
}

/// Where each job goes, its column and row (its stage's column, among those with jobs; or by
/// what it waits for), and the room they take.
fn place(
    jobs: &[(usize, usize)],
    (stages, waits): (&[Range<usize>], &[Wait]),
    by: JobsLayout,
) -> (Vec<(usize, usize)>, egui::Vec2) {
    let columns = if by == JobsLayout::Dependencies {
        depths(waits, stages)
    } else {
        // Stages without jobs take no column.
        let mut column_of = Vec::with_capacity(stages.len());
        let mut next = 0_usize;
        for jobs in stages {
            column_of.push(next);
            if !jobs.is_empty() {
                next = next.saturating_add(1);
            }
        }
        jobs.iter()
            .map(|(stage, _)| column_of.get(*stage).copied().unwrap_or(0))
            .collect()
    };
    let mut filled = vec![
        0_usize;
        columns
            .iter()
            .max()
            .map_or(0, |most| most.saturating_add(1))
    ];
    let places: Vec<(usize, usize)> = columns
        .into_iter()
        .map(|column| {
            let row = filled.get(column).copied().unwrap_or(0);
            if let Some(slot) = filled.get_mut(column) {
                *slot = row.saturating_add(1);
            }
            (column, row)
        })
        .collect();
    let rows = filled.iter().max().copied().unwrap_or(0);
    let top = if by == JobsLayout::Stages { TITLE } else { 0.0 };
    let size = egui::vec2(
        count(filled.len()) * (CARD.x + COLUMN_GAP),
        count(rows).mul_add(CARD.y + ROW_GAP, top),
    );
    (places, size)
}

/// The job at `(stage, place)` of `details`.
fn job_at(details: &PipelineDetails, (stage, place): (usize, usize)) -> Option<&Job> {
    details.stages.get(stage)?.jobs.get(place)
}

/// A count as points' factor.
fn count(of: usize) -> f32 {
    f32::from(u16::try_from(of).unwrap_or(u16::MAX))
}

/// Each job's column by what it waits for: after the furthest of those (a round of waits is
/// broken where it would close); a stage's furthest worked out once.
fn depths(waits: &[Wait], stages: &[Range<usize>]) -> Vec<usize> {
    struct Walk<'a> {
        waits: &'a [Wait],
        stages: &'a [Range<usize>],
        depth: Vec<Option<usize>>,
        stage_depth: Vec<Option<usize>>,
        walking: Vec<bool>,
    }
    impl Walk<'_> {
        fn job(&mut self, at: usize) -> usize {
            if let Some(known) = self.depth.get(at).copied().flatten() {
                return known;
            }
            if self.walking.get(at).copied().unwrap_or(true) {
                return 0;
            }
            if let Some(walking) = self.walking.get_mut(at) {
                *walking = true;
            }
            let found = match self.waits.get(at) {
                Some(Wait::Named(on)) => on
                    .iter()
                    .map(|from| self.job(*from).saturating_add(1))
                    .max()
                    .unwrap_or(0),
                Some(Wait::Stage(Some(stage))) => self.stage(*stage).saturating_add(1),
                _ => 0,
            };
            if let Some(walking) = self.walking.get_mut(at) {
                *walking = false;
            }
            if let Some(slot) = self.depth.get_mut(at) {
                *slot = Some(found);
            }
            found
        }

        fn stage(&mut self, stage: usize) -> usize {
            if let Some(known) = self.stage_depth.get(stage).copied().flatten() {
                return known;
            }
            let found = self
                .stages
                .get(stage)
                .cloned()
                .unwrap_or_default()
                .map(|at| self.job(at))
                .max()
                .unwrap_or(0);
            if let Some(slot) = self.stage_depth.get_mut(stage) {
                *slot = Some(found);
            }
            found
        }
    }
    let mut walk = Walk {
        waits,
        stages,
        depth: vec![None; waits.len()],
        stage_depth: vec![None; stages.len()],
        walking: vec![false; waits.len()],
    };
    (0..waits.len()).map(|at| walk.job(at)).collect()
}

/// The graph of `details`'s jobs as `by` lays them out, kept in `graph` from frame to frame.
pub(super) fn graph(
    ui: &mut Ui,
    (palette, said): (Palette, CiWords),
    details: &Arc<PipelineDetails>,
    (by, graph): (JobsLayout, &mut Graph),
) -> Option<JobAsk> {
    if !graph
        .laid
        .as_ref()
        .is_some_and(|laid| laid.is_of(details, by))
    {
        let laid = Laid::new(details, by);
        graph.hovered = None;
        graph.lit = vec![false; laid.jobs.len()];
        graph.laid = Some(laid);
    }
    let Some(laid) = &graph.laid else {
        return None;
    };
    if laid.jobs.is_empty() {
        ui.label(crate::widgets::muted(ui, "It has no jobs."));
        return None;
    }
    let mut asked = None;
    let mut now_hovered = None;
    egui::ScrollArea::both()
        .id_salt("pipeline-graph")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (area, _) = ui.allocate_exact_size(laid.size, Sense::hover());
            let top = if by == JobsLayout::Stages { TITLE } else { 0.0 };
            let rects: Vec<Rect> = laid
                .places
                .iter()
                .map(|(column, row)| {
                    let x = count(*column) * (CARD.x + COLUMN_GAP);
                    let y = count(*row).mul_add(CARD.y + ROW_GAP, top);
                    Rect::from_min_size(egui::pos2(area.min.x + x, area.min.y + y), CARD)
                })
                .collect();
            let shown = ui.clip_rect();
            lines(ui, palette, (laid, &rects), (&graph.lit, graph.hovered));
            if by == JobsLayout::Stages {
                titles(ui, palette, laid, &rects);
            }
            for (at, rect) in rects.iter().enumerate() {
                let Some(job) = laid.job(at).filter(|_| shown.intersects(*rect)) else {
                    continue;
                };
                let dim = graph.hovered.is_some() && !graph.lit.get(at).copied().unwrap_or(false);
                let (card_asked, is_hovered) = card(ui, (palette, said), (job, *rect), (dim, at));
                if is_hovered {
                    now_hovered = Some(at);
                }
                if card_asked.is_some() {
                    asked = card_asked;
                }
            }
        });
    if now_hovered != graph.hovered {
        graph.hovered = now_hovered;
        graph.lit = laid.lit(now_hovered);
        ui.ctx().request_repaint();
    }
    asked
}

/// Each stage's title over its column, where it shows: its status and name.
fn titles(ui: &mut Ui, palette: Palette, laid: &Laid, rects: &[Rect]) {
    let shown = ui.clip_rect();
    for (stage, jobs) in laid.stages.iter().enumerate() {
        let (Some(first), Some(of)) = (rects.get(jobs.start), laid.details.stages.get(stage))
        else {
            continue;
        };
        let title = Rect::from_min_size(
            egui::pos2(first.left(), first.top() - TITLE),
            egui::vec2(CARD.x, TITLE - 6.0),
        );
        if jobs.is_empty() || !shown.intersects(title) {
            continue;
        }
        crate::help::note_topic(ui, title, super::TOPIC);
        let mut inside = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(title)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let (glyph, color) = ci_look(of.status, palette);
        inside.label(RichText::new(glyph).color(color));
        inside.add(
            egui::Label::new(RichText::new(&of.name).semibold())
                .selectable(false)
                .truncate(),
        );
    }
}

/// The lines from what each job waits for to it, where they show: by name always; a stage's
/// waits only where lit (else every job of a stage would join every one of the next). Lit lines
/// stand out.
fn lines(
    ui: &Ui,
    palette: Palette,
    (laid, rects): (&Laid, &[Rect]),
    (lit, hovered): (&[bool], Option<usize>),
) {
    let painter = ui.painter();
    let shown = ui.clip_rect();
    let is_lit = |at: usize| lit.get(at).copied().unwrap_or(false);
    let line = |from: usize, to: usize| {
        let (Some(from_rect), Some(to_rect)) = (rects.get(from), rects.get(to)) else {
            return;
        };
        if !shown.intersects(from_rect.union(*to_rect)) {
            return;
        }
        let stroke = if is_lit(from) && is_lit(to) {
            Stroke::new(2.0, palette.accent)
        } else if hovered.is_some() {
            Stroke::new(1.0, palette.border)
        } else {
            Stroke::new(1.0, palette.border_strong)
        };
        painter.add(curve(*from_rect, *to_rect, stroke));
    };
    for (at, wait) in laid.waits.iter().enumerate() {
        match wait {
            Wait::Named(on) => {
                for from in on {
                    line(*from, at);
                }
            }
            Wait::Stage(Some(stage)) if is_lit(at) => {
                for from in laid.stages.get(*stage).cloned().unwrap_or_default() {
                    if is_lit(from) {
                        line(from, at);
                    }
                }
            }
            Wait::Stage(_) => {}
        }
    }
}

/// A line from card `from` to card `to`: from its right edge to the other's left, or, where
/// `to` isn't to its right (one stage's job waiting for another's), down from below it.
fn curve(from: Rect, to: Rect, stroke: Stroke) -> CubicBezierShape {
    let (start, end, out, into) = if to.left() > from.right() {
        let start = Pos2::new(from.right(), from.center().y);
        let end = Pos2::new(to.left(), to.center().y);
        let bend = (end.x - start.x) / 2.0;
        (start, end, egui::vec2(bend, 0.0), egui::vec2(-bend, 0.0))
    } else {
        let start = Pos2::new(from.left() + 12.0, from.bottom());
        let end = Pos2::new(to.left() + 12.0, to.top());
        (
            start,
            end,
            egui::vec2(-12.0, 12.0),
            egui::vec2(-12.0, -12.0),
        )
    };
    let toward = |at: Pos2, by: egui::Vec2| Pos2::new(at.x + by.x, at.y + by.y);
    CubicBezierShape::from_points_stroke(
        [start, toward(start, out), toward(end, into), end],
        false,
        Color32::TRANSPARENT,
        stroke,
    )
}

/// A job's card at `rect`: its status, name, and how long it ran (dimmed while another's path
/// is lit); returns what it asked, and whether the pointer rests on it.
fn card(
    ui: &mut Ui,
    (palette, said): (Palette, CiWords),
    (job, rect): (&Job, Rect),
    (dim, at): (bool, usize),
) -> (Option<JobAsk>, bool) {
    let opens = job.id.is_some() || job.url.is_some();
    let response = ui.interact(
        rect,
        Id::new(("pipeline-card", at)),
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
    let fade = |color: Color32| {
        if dim {
            color.gamma_multiply(0.35)
        } else {
            color
        }
    };
    let edge = if response.hovered() {
        palette.accent
    } else {
        palette.border
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS_CONTROL),
        if response.hovered() {
            palette.surface_alt
        } else {
            palette.surface
        },
        Stroke::new(1.0, fade(edge)),
        egui::StrokeKind::Inside,
    );
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(10.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let (glyph, color) = super::list::job_look(job, palette);
    inside.label(RichText::new(glyph).color(fade(color)));
    let pointed = ui.rect_contains_pointer(rect) && !dim;
    let mut acted = None;
    inside.with_layout(Layout::right_to_left(Align::Center), |ui| {
        acted = slot(ui, (job, at), (pointed, said), fade(palette.muted));
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(&job.name).color(fade(palette.text)))
                    .selectable(false)
                    .truncate(),
            );
        });
    });
    let is_hovered = response.hovered();
    let response = response.on_hover_ui(|ui| {
        ui.label(RichText::new(&job.name).semibold());
        ui.label(ci_words(job.status, false));
        let waits = match job.needs.as_deref() {
            None => "Waits for the stages before it".to_owned(),
            Some([]) => "Waits for nothing: starts at once".to_owned(),
            Some(needs) => format!("Needs {}", needs.join(", ")),
        };
        ui.label(crate::widgets::muted(ui, waits));
        if !job.earlier.is_empty() {
            ui.label(crate::widgets::muted(
                ui,
                format!("Ran {} times", job.earlier.len().saturating_add(1)),
            ));
        }
    });
    let mut asked = None;
    response.context_menu(|ui| {
        if let Some(menu_asked) = job_menu(ui, job, said) {
            asked = Some(menu_asked);
        }
    });
    if response.clicked() {
        asked = clicked(job);
    }
    if let Some(action) = acted {
        asked = Some(JobAsk::Act(action));
    }
    (asked, is_hovered)
}

/// A card's right: how long its job took, in a place as wide whatever it says (the name beside
/// it keeps its room as it grows); under the pointer (`pointed`), what can be done to it
/// (retry, cancel, start) in its place. Returns what was asked.
fn slot(
    ui: &mut Ui,
    (job, at): (&Job, usize),
    (pointed, said): (bool, CiWords),
    muted: egui::Color32,
) -> Option<egit_forge::CiAction> {
    let width = crate::widgets::duration_width(ui, &egui::TextStyle::Small);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id(Id::new(("pipeline-card-slot", at)))
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.set_min_width(width);
            let acting = pointed && job.can_update && job.id.is_some() && has_action(job);
            if acting {
                return super::list::quick_actions(ui, job, said);
            }
            if let Some(took) = job.duration {
                ui.label(RichText::new(duration(took)).color(muted).small());
            }
            None
        },
    )
    .inner
}

/// Whether a job offers something to do (start, cancel, retry).
const fn has_action(job: &Job) -> bool {
    job.actions.play || job.actions.cancel || job.actions.retry
}

#[cfg(test)]
mod tests {
    use egit_forge::{CiStatus, JobActions, Pipeline, Stage};
    use egit_git::ObjectFormat;

    use super::*;

    fn job(name: &str, needs: Option<&[&str]>) -> Job {
        Job {
            id: Some(1),
            name: name.to_owned(),
            status: CiStatus::Success,
            allow_failure: false,
            manual: false,
            duration: None,
            started: None,
            url: None,
            needs: needs.map(|needs| needs.iter().map(|&need| need.to_owned()).collect()),
            actions: JobActions::default(),
            can_update: true,
            earlier: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    fn details(stages: Vec<(&str, Vec<Job>)>) -> Arc<PipelineDetails> {
        Arc::new(PipelineDetails {
            pipeline: Pipeline {
                id: 1,
                iid: 1,
                name: None,
                number: 1,
                series: 0,
                attempt: 1,
                commit: ObjectFormat::Sha1.zero(),
                status: CiStatus::Success,
                warnings: false,
                git_ref: None,
                created: 0,
                url: String::new(),
            },
            started: None,
            finished: None,
            duration: None,
            user: None,
            source: None,
            stages: stages
                .into_iter()
                .map(|(name, jobs)| Stage {
                    name: name.to_owned(),
                    status: CiStatus::Success,
                    jobs,
                })
                .collect(),
            actions: egit_forge::PipelineActions::default(),
            can_update: true,
            ordered_stages: true,
            workflow: None,
            artifacts: Vec::new(),
        })
    }

    /// compile; unit needs compile, e2e waits for build, docs needs nothing; production needs
    /// unit; an empty stage between.
    fn pipeline() -> Arc<PipelineDetails> {
        details(vec![
            ("build", vec![job("compile", None)]),
            (
                "test",
                vec![
                    job("unit", Some(&["compile"])),
                    job("e2e", None),
                    job("docs", Some(&[])),
                ],
            ),
            ("nothing", Vec::new()),
            ("deploy", vec![job("production", Some(&["unit"]))]),
        ])
    }

    fn names(laid: &Laid, lit: &[bool]) -> Vec<String> {
        lit.iter()
            .enumerate()
            .filter(|(_, lit)| **lit)
            .filter_map(|(at, _)| Some(laid.job(at)?.name.clone()))
            .collect()
    }

    #[test]
    fn jobs_stand_after_what_they_wait_for_or_in_their_stage_s_column() {
        let pipeline = pipeline();
        let by_needs = Laid::new(&pipeline, JobsLayout::Dependencies);
        let columns: Vec<usize> = by_needs.places.iter().map(|(column, _)| *column).collect();
        // compile, unit, e2e, docs, production.
        assert_eq!(columns, [0, 1, 1, 0, 2]);
        let by_stage = Laid::new(&pipeline, JobsLayout::Stages);
        let columns: Vec<usize> = by_stage.places.iter().map(|(column, _)| *column).collect();
        // The empty stage takes no column.
        assert_eq!(columns, [0, 1, 1, 1, 2]);
        let rows: Vec<usize> = by_stage.places.iter().map(|(_, row)| *row).collect();
        assert_eq!(rows, [0, 0, 1, 2, 0]);
    }

    #[test]
    fn a_job_lights_what_it_waits_for_and_what_waits_for_it_all_along() {
        let pipeline = pipeline();
        let laid = Laid::new(&pipeline, JobsLayout::Stages);
        assert_eq!(
            names(&laid, &laid.lit(Some(0))),
            ["compile", "unit", "e2e", "production"]
        );
        assert_eq!(
            names(&laid, &laid.lit(Some(1))),
            ["compile", "unit", "production"]
        );
        assert_eq!(names(&laid, &laid.lit(Some(3))), ["docs"]);
        assert!(laid.lit(None).iter().all(|lit| !lit));
    }

    #[test]
    fn a_round_of_needs_is_laid_out_all_the_same() {
        let round = details(vec![(
            "test",
            vec![
                job("a", Some(&["b"])),
                job("b", Some(&["a"])),
                job("c", Some(&["a"])),
            ],
        )]);
        let laid = Laid::new(&round, JobsLayout::Dependencies);
        assert_eq!(laid.places.len(), 3);
        assert_eq!(names(&laid, &laid.lit(Some(2))), ["a", "b", "c"]);
    }
}
