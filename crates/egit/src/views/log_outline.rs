//! A job's log's outline, at the log's left: its sections as a tree (a GitHub job's steps
//! first, what they hold stepped in under them; a folded section's held back, as its lines are
//! in the log), each with how it went, the line heading it in the log, how long it took, and a
//! bar under it on the job's timeline (where it began and how long it ran: where the time
//! went), the section the log shows lit; then the lines the runner marked as errors or warnings.
//! A click goes to a section's header or a marked line (unfolding what hides it); a section's
//! caret folds or unfolds it in the log, as a click on its header there does.

use egit_forge::{CiStatus, MarkKind, Section};
use egui::{Align, Color32, CornerRadius, Id, Layout, Rect, Sense, Ui, pos2, vec2};

use super::pipeline::ci_look;
use crate::job_log::LogSource;
use crate::log_folds::Folds;
use crate::theme::{Palette, SMALL, icon};
use crate::widgets::{self, Head};

/// A row's height.
const ROW: f32 = 30.0;

/// How far each level steps in.
const INDENT: f32 = 14.0;

/// The caret's width (a target of its own, left of the row's).
const CARET: f32 = 24.0;

/// The help topic explaining it.
const TOPIC: &str = "job-log-sections";

/// What the outline asks of the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutlineAsk {
    /// Show the whole log's line (a section's header, a marked line), unfolding what hides it.
    Go(u32),
    /// Fold section (its place among the log's), or unfold it.
    Fold(usize),
    /// Fold every section (true), or unfold them all.
    Every(bool),
}

/// The outline's state between frames: the section lit last, and the rows last in view.
#[derive(Debug, Default)]
pub(super) struct Outline {
    lit: Option<usize>,
    visible: std::ops::Range<usize>,
}

/// One of its rows.
#[derive(Clone, Copy)]
enum Row {
    /// A section, by its place.
    Section(usize),
    /// The marked lines' heading.
    Marks,
    /// A marked line, by its place.
    Mark(usize),
}

/// How the job the log is of stands, for its sections' looks.
#[derive(Clone, Copy)]
pub(super) struct Standing {
    /// The job's status.
    pub(super) status: CiStatus,
    /// Now (seconds since the Unix epoch).
    pub(super) now: i64,
}

/// The outline of `log` (its sections folded as `folds` says; `lit` the one the log shows);
/// returns what was asked.
pub(super) fn show(
    ui: &mut Ui,
    (log, folds): (&LogSource, &Folds),
    (lit, standing): (Option<usize>, Standing),
    outline: &mut Outline,
) -> Option<OutlineAsk> {
    let palette = Palette::of(ui.ctx());
    let mut asked = None;
    widgets::panel_head(ui, Head::Control, |ui| {
        ui.horizontal(|ui| {
            let title = widgets::section_title(
                ui,
                "Sections",
                Some(log.sections.len()),
                &format!("Sections: {}", log.sections.len()),
            );
            crate::help::note_topic(ui, title.rect, TOPIC);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let open = icon::ARROWS_OUT_LINE_VERTICAL;
                if widgets::icon_tool(ui, true, open, "Unfold every section")
                    .on_hover_text("Unfold every section")
                    .clicked()
                {
                    asked = Some(OutlineAsk::Every(false));
                }
                let shut = icon::ARROWS_IN_LINE_VERTICAL;
                if widgets::icon_tool(ui, true, shut, "Fold every section")
                    .on_hover_text("Fold every section")
                    .clicked()
                {
                    asked = Some(OutlineAsk::Every(true));
                }
            });
        });
    });
    let rows = rows(log, folds);
    let span = timeline(log, standing.now);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("job-log-outline")
        .auto_shrink([false, false]);
    // The section lit moved out of view: it is brought in.
    if lit != outline.lit {
        outline.lit = lit;
        if let Some(at) = lit.and_then(|lit| {
            rows.iter()
                .position(|row| matches!(row, Row::Section(place) if *place == lit))
        }) && !outline.visible.contains(&at)
        {
            scroll = scroll.vertical_scroll_offset(
                f32::from(u16::try_from(at.saturating_sub(2)).unwrap_or(u16::MAX)) * ROW,
            );
        }
    }
    scroll.show_rows(ui, ROW, rows.len(), |ui, visible| {
        ui.spacing_mut().item_spacing.y = 0.0;
        outline.visible = visible.clone();
        for row in rows.get(visible).unwrap_or_default() {
            let row_asked = match *row {
                Row::Section(place) => log.sections.get(place).and_then(|section| {
                    let looks = Looks {
                        palette,
                        span,
                        standing,
                    };
                    section_row(ui, (log, section, place), folds, lit == Some(place), looks)
                }),
                Row::Marks => {
                    marks_heading(ui, log.marks.len());
                    None
                }
                Row::Mark(place) => log
                    .marks
                    .get(place)
                    .and_then(|mark| mark_row(ui, log, *mark, palette)),
            };
            if row_asked.is_some() {
                asked = row_asked;
            }
        }
    });
    asked
}

/// The rows shown: each section not inside a folded one, then the marked lines.
fn rows(log: &LogSource, folds: &Folds) -> Vec<Row> {
    let mut rows = Vec::with_capacity(log.sections.len().saturating_add(log.marks.len()));
    // The last line of the folded section the sections are in, while they are.
    let mut folded_until: Option<u32> = None;
    for (place, section) in log.sections.iter().enumerate() {
        if folded_until.is_some_and(|last| section.line <= last) {
            continue;
        }
        folded_until = None;
        rows.push(Row::Section(place));
        if folds.is_folded(log, section) {
            folded_until = Some(log.last_line(section));
        }
    }
    if !log.marks.is_empty() {
        rows.push(Row::Marks);
        rows.extend((0..log.marks.len()).map(Row::Mark));
    }
    rows
}

/// The job's timeline: from its first section's start to the last's end (or now).
fn timeline(log: &LogSource, now: i64) -> Option<(i64, i64)> {
    let starts = log
        .sections
        .iter()
        .map(|section| section.started)
        .filter(|started| *started != 0);
    let first = starts.min()?;
    let last = log
        .sections
        .iter()
        .filter(|section| section.started != 0)
        .map(|section| section.ended.map_or(now, |(ended, _)| ended))
        .max()?;
    (last > first).then_some((first, last))
}

/// What a section's row is drawn with.
#[derive(Clone, Copy)]
struct Looks {
    palette: Palette,
    span: Option<(i64, i64)>,
    standing: Standing,
}

/// How `section` went: its step's own say, else failed where it holds an error (or was open as
/// the job failed), running while open in a job under way, passed otherwise.
fn status_of(log: &LogSource, section: &Section, standing: Standing) -> CiStatus {
    if let Some(status) = section.status {
        return status;
    }
    let last = log.last_line(section);
    let failed = log
        .marks
        .iter()
        .any(|mark| mark.kind == MarkKind::Error && mark.line > section.line && mark.line <= last);
    match (section.ended, failed) {
        (_, true) => CiStatus::Failed,
        (None, false) if standing.status.is_moving() => CiStatus::Running,
        (None, false) if standing.status == CiStatus::Failed => CiStatus::Failed,
        _ => CiStatus::Success,
    }
}

/// The words heading `section` in the log (its header's line), else its name.
fn title_of<'a>(log: &'a LogSource, section: &'a Section) -> std::borrow::Cow<'a, str> {
    use crate::grid::GridSource as _;
    log.text()
        .line(section.line)
        .map(|line| String::from_utf8_lossy(line))
        .filter(|line| !line.trim().is_empty())
        .map_or(std::borrow::Cow::Borrowed(section.name.as_str()), |line| {
            std::borrow::Cow::Owned(line.trim().to_owned())
        })
}

/// A section's row: its caret, how it went, its title, how long it took, and its bar on the
/// timeline; lit where the log shows it. Returns what it asked.
fn section_row(
    ui: &mut Ui,
    (log, section, place): (&LogSource, &Section, usize),
    folds: &Folds,
    lit: bool,
    looks: Looks,
) -> Option<OutlineAsk> {
    let palette = looks.palette;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let title = title_of(log, section);
    let took = LogSource::took(section, looks.standing.now);
    let words = took.map_or_else(
        || title.to_string(),
        |took| format!("{title} · {}", crate::format::duration(took)),
    );
    let step_in = f32::from(section.depth.min(12)) * INDENT;
    let caret_rect = Rect::from_min_size(
        pos2(rect.left() + 4.0 + step_in, rect.top()),
        vec2(CARET, ROW),
    );
    let body = Rect::from_min_max(pos2(caret_rect.right(), rect.top()), rect.max);
    let row = ui.interact(body, Id::new(("job-log-section", place)), Sense::click());
    row.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &words));
    crate::help::note_topic(ui, rect, TOPIC);
    widgets::row_fill(
        ui,
        rect,
        crate::theme::RADIUS_INNER.into(),
        lit,
        row.hovered(),
    );
    let folded = folds.is_folded(log, section);
    let mut asked = (log.folds(section) && caret(ui, (caret_rect, place), folded, &title, palette))
        .then_some(OutlineAsk::Fold(place));
    let status = status_of(log, section, looks.standing);
    let (glyph, color) = ci_look(status, palette);
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body.shrink2(vec2(2.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    inside.label(egui::RichText::new(glyph).color(color));
    inside.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let width = widgets::duration_width(ui, &egui::TextStyle::Small);
        widgets::column(ui, (width, ROW), Align::Max, |ui| {
            if let Some(took) = took {
                ui.label(
                    egui::RichText::new(crate::format::duration(took))
                        .small()
                        .color(palette.muted),
                );
            }
        });
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let strong = section.depth == 0 && section.status.is_some();
            let text = egui::RichText::new(title.as_ref()).color(palette.text);
            let text = if strong {
                crate::theme::Weighted::medium(text)
            } else {
                text
            };
            ui.add(egui::Label::new(text).selectable(false).truncate());
        });
    });
    if let Some((first, last)) = looks.span {
        paint_bar(
            ui,
            (body, section),
            (first, last),
            looks.standing.now,
            color,
        );
    }
    let row = row.on_hover_ui(|ui| about(ui, (log, section), (&title, status, took)));
    if row.clicked() {
        asked = Some(OutlineAsk::Go(section.line));
    }
    asked
}

/// A section's caret in `rect` (its place `place`), a target of its own folding or unfolding it
/// (`folded`: whether it is); whether it was clicked.
fn caret(
    ui: &Ui,
    (rect, place): (Rect, usize),
    folded: bool,
    title: &str,
    palette: Palette,
) -> bool {
    let name = if folded {
        format!("Unfold {title}")
    } else {
        format!("Fold {title}")
    };
    let toggle = ui.interact(rect, Id::new(("job-log-fold", place)), Sense::click());
    toggle.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    let glyph = if folded {
        icon::CARET_RIGHT
    } else {
        icon::CARET_DOWN
    };
    let color = if toggle.hovered() {
        palette.text
    } else {
        palette.muted
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(SMALL),
        color,
    );
    toggle.on_hover_text(&name).clicked()
}

/// What a section's row says on hover: its title (and name, where they differ), how it went,
/// how many lines it holds, and how long it took.
fn about(
    ui: &mut Ui,
    (log, section): (&LogSource, &Section),
    (title, status, took): (&str, CiStatus, Option<u64>),
) {
    ui.label(egui::RichText::new(title).strong());
    if section.name != title {
        ui.label(widgets::muted(ui, &section.name));
    }
    let lines = log.last_line(section).saturating_sub(section.line);
    let mut said = format!(
        "{} · {} line{}",
        super::pipeline::ci_words(status, false),
        crate::format::count(u64::from(lines)),
        if lines == 1 { "" } else { "s" }
    );
    if let Some(took) = took {
        said.push_str(" · ");
        said.push_str(&crate::format::duration(took));
    }
    ui.label(widgets::muted(ui, said));
}

/// `section`'s bar on the job's timeline (`first` to `last`), under its row: where it began,
/// and how long it ran.
fn paint_bar(
    ui: &Ui,
    (body, section): (Rect, &Section),
    (first, last): (i64, i64),
    now: i64,
    color: Color32,
) {
    if section.started == 0 {
        return;
    }
    let whole = last.saturating_sub(first).max(1);
    let until = section.ended.map_or(now, |(ended, _)| ended);
    let left = body.left() + 2.0;
    let width = (body.right() - 8.0 - left).max(1.0);
    let at = |time: i64| {
        let part = u64::try_from(time.saturating_sub(first).clamp(0, whole)).unwrap_or(0);
        left + width * crate::grid::ratio(part, u64::try_from(whole).unwrap_or(1))
    };
    let (from, to) = (at(section.started), at(until));
    let palette = Palette::of(ui.ctx());
    // The whole job's time, faint, the section's part of it in its colour.
    let track = Rect::from_min_max(
        pos2(left, body.bottom() - 5.0),
        pos2(left + width, body.bottom() - 3.0),
    );
    ui.painter().rect_filled(
        track,
        CornerRadius::same(1),
        palette.border.gamma_multiply(0.6),
    );
    let bar = Rect::from_min_max(
        pos2(from, track.top()),
        pos2(to.max(from + 3.0), track.bottom()),
    );
    ui.painter().rect_filled(bar, CornerRadius::same(1), color);
}

/// The marked lines' heading.
fn marks_heading(ui: &mut Ui, count: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(8.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let title = widgets::section_title(
        &mut inside,
        "Errors and warnings",
        Some(count),
        &format!("Errors and warnings: {count}"),
    );
    crate::help::note_topic(ui, title.rect, TOPIC);
}

/// A marked line's row: an error's or a warning's mark, and the line; a click goes to it.
fn mark_row(
    ui: &mut Ui,
    log: &LogSource,
    mark: egit_forge::LogMark,
    palette: Palette,
) -> Option<OutlineAsk> {
    use crate::grid::GridSource as _;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let line = log
        .text()
        .line(mark.line)
        .map_or_default(String::from_utf8_lossy);
    let line = line.trim();
    let words = format!("Line {}: {line}", u64::from(mark.line).saturating_add(1));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &words));
    crate::help::note_topic(ui, rect, TOPIC);
    widgets::row_fill(
        ui,
        rect,
        crate::theme::RADIUS_INNER.into(),
        false,
        response.hovered(),
    );
    let (glyph, color) = match mark.kind {
        MarkKind::Error => (icon::X_CIRCLE, palette.danger),
        MarkKind::Warning => (icon::WARNING, palette.warn),
    };
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(10.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    inside.label(egui::RichText::new(glyph).color(color));
    inside.add(
        egui::Label::new(egui::RichText::new(line).color(palette.text))
            .selectable(false)
            .truncate(),
    );
    response
        .on_hover_text(&words)
        .clicked()
        .then_some(OutlineAsk::Go(mark.line))
}
