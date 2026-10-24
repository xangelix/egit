//! The Stats page, in the commit table's place: the history shown, in figures and charts.
//!
//! Tiles (commits and merges, authors, days with commits, the first and latest commit, the
//! longest streak, the busiest day, the files at `HEAD`), then cards: commits each month and
//! authors active each month (egui's charts), the year of contributions to the latest commit (a
//! calendar) and when in the week commits are made (a punch card, by their authors' clocks), the
//! authors with the most commits, the languages and the largest files at `HEAD`, and the lines
//! the newest commits changed, each month and by size. Read in jobs (`stats.rs`), again when
//! the history is read again; what was read shows meanwhile. Escape or History goes back.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use egit_git::CancelToken;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Label, Layout, Margin, Rect, RichText,
    ScrollArea, Sense, Ui, pos2, vec2,
};
use egui_async::Bind;
use egui_plot::{Bar, BarChart, GridInput, GridMark, Line, Plot, PlotPoints};

use super::history::Shared;
use crate::format::{self, count};
use crate::history::History;
use crate::problem::Problem;
use crate::stats::{
    AuthorStats, LINES_COUNTED, LineStats, Stats, date_of, day_of, read_lines, read_stats,
};
use crate::theme::Weighted as _;
use crate::theme::{self, Palette, icon};
use crate::widgets::{self, muted};

/// Below this width, cards stack rather than stand in pairs.
const TWO_COLUMNS: f32 = 860.0;

/// The space between cards.
const GAP: f32 = 12.0;

/// The weeks the contributions calendar shows.
const WEEKS: u8 = 53;

/// The authors listed.
const AUTHORS_LISTED: usize = 12;

/// The languages the bar shows apart (the rest together).
const LANGUAGES_SHOWN: usize = 8;

/// What the page asks of the history view.
#[derive(Debug)]
pub(super) enum StatsEvent {
    Close,
}

/// The page's state: the statistics read (and being read), and the lines counted.
#[derive(Debug)]
pub(super) struct StatsView {
    /// The history the statistics are of (`Names::identity`), once asked.
    made_for: Option<(u64, usize)>,
    stats: Bind<Arc<Stats>, Problem>,
    /// The commits read so far, of how many.
    read: Arc<AtomicU64>,
    reading: u64,
    lines: Bind<Arc<LineStats>, Problem>,
    /// The history the lines were asked for; whether they aren't counted (a partial clone).
    lines_for: Option<(u64, usize)>,
    lines_skipped: bool,
    /// The commits whose lines are counted so far, of how many.
    counted: Arc<AtomicU64>,
    counting: u64,
    cancel: CancelToken,
}

impl Default for StatsView {
    fn default() -> Self {
        // Retained: egui-async drops what a `Bind` not drawn the frame before holds.
        Self {
            made_for: None,
            stats: Bind::new(true),
            read: Arc::default(),
            reading: 0,
            lines: Bind::new(true),
            lines_for: None,
            lines_skipped: false,
            counted: Arc::default(),
            counting: 0,
            cancel: CancelToken::new(),
        }
    }
}

impl Drop for StatsView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl StatsView {
    /// Whether the figures have landed, and the lines (or they aren't counted).
    #[cfg(test)]
    pub(super) fn landed(&mut self) -> (bool, bool) {
        let figures = !self.stats.is_pending() && matches!(self.stats.read_as_ref(), Some(Ok(_)));
        let lines =
            self.lines_skipped || (!self.lines.is_pending() && self.lines.read_as_ref().is_some());
        (figures, figures && lines)
    }

    /// Reads `history`'s statistics unless they are of it already (a read of an older one
    /// stops), once it is read whole and the configuration is known.
    fn follow(&mut self, shared: &Shared<'_>, history: &History) {
        let identity = history.identity();
        let (Some(git), Some(config)) = (shared.git, shared.config) else {
            return;
        };
        if history.is_loading() || self.made_for == Some(identity) {
            return;
        }
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.read = Arc::default();
        self.reading = history.rows();
        self.stats.request(read_stats(
            git.clone(),
            Arc::clone(shared.repository),
            history.names(),
            !config.is_partial_clone(),
            Arc::clone(&self.read),
            self.cancel.clone(),
        ));
        self.made_for = Some(identity);
        self.lines_skipped = config.is_partial_clone();
    }

    /// Counts the newest commits' lines once the rest has landed (never in a partial clone:
    /// git would fetch every file's contents).
    fn follow_lines(&mut self, shared: &Shared<'_>, history: &History) {
        let Some(git) = shared.git else {
            return;
        };
        if self.lines_for == self.made_for
            || self.lines_skipped
            || self.stats.is_pending()
            || !matches!(self.stats.read_as_ref(), Some(Ok(_)))
        {
            return;
        }
        self.counted = Arc::default();
        self.counting = history.rows().min(LINES_COUNTED);
        self.lines.request(read_lines(
            git.clone(),
            Arc::clone(shared.repository),
            history.names(),
            Arc::clone(&self.counted),
            self.cancel.clone(),
        ));
        self.lines_for = self.made_for;
    }
}

/// Draws the page; returns what it asked for.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &History,
    view: &mut StatsView,
) -> Option<StatsEvent> {
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut event = escape.then_some(StatsEvent::Close);
    view.follow(shared, history);
    view.follow_lines(shared, history);
    let reading = view.stats.is_pending() || view.lines.is_pending();
    if reading {
        // What has been read so far, as it goes.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT))
                .on_hover_text("Back to the history (Escape)")
                .clicked()
            {
                event = Some(StatsEvent::Close);
            }
            ui.label(RichText::new("Statistics").semibold());
            ui.label(muted(
                ui,
                format!(
                    "{} commits, as the history shows them",
                    count(history.rows())
                ),
            ));
            if view.stats.is_pending() {
                ui.spinner();
            }
        });
    });
    let stats = match view.stats.read_as_ref() {
        Some(Ok(stats)) => Arc::clone(stats),
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return event;
        }
        None => {
            let said = if history.is_loading() || view.made_for.is_none() {
                "Waiting for the history to be read…".to_owned()
            } else {
                format!(
                    "Reading {} of {} commits…",
                    count(view.read.load(Ordering::Relaxed)),
                    count(view.reading)
                )
            };
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| widgets::working(ui, &said));
            return event;
        }
    };
    let lines = Counted {
        lines: view.lines.read_as_ref().and_then(Result::ok).cloned(),
        problem: view.lines.read_as_ref().and_then(Result::err).cloned(),
        skipped: view.lines_skipped,
        progress: (view.counted.load(Ordering::Relaxed), view.counting),
    };
    ScrollArea::vertical()
        .id_salt(("stats-page", shared.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(1_200.0));
                    body(ui, shared, &stats, &lines);
                });
        });
    event
}

/// Where counting the newest commits' lines stands.
struct Counted {
    lines: Option<Arc<LineStats>>,
    problem: Option<Problem>,
    /// Not counted: a partial clone.
    skipped: bool,
    /// Commits counted so far, of how many.
    progress: (u64, u64),
}

/// The tiles and cards.
fn body(ui: &mut Ui, shared: &Shared<'_>, stats: &Stats, counted: &Counted) {
    let palette = shared.palette;
    if stats.commits == 0 {
        ui.label(muted(ui, "No commits yet."));
        return;
    }
    let lines = counted.lines.as_deref();
    tiles(ui, stats, palette);
    ui.add_space(GAP);
    card(ui, |ui| commits_chart(ui, shared, stats));
    ui.add_space(GAP);
    card(ui, |ui| calendar(ui, stats, palette));
    ui.add_space(GAP);
    pair(
        ui,
        "when",
        |ui| punch_card(ui, stats, palette),
        |ui| active_chart(ui, shared, stats),
    );
    ui.add_space(GAP);
    pair(
        ui,
        "who",
        |ui| authors(ui, stats, lines, palette),
        |ui| languages(ui, stats, palette),
    );
    ui.add_space(GAP);
    pair(
        ui,
        "lines",
        |ui| lines_chart(ui, shared, counted),
        |ui| sizes_chart(ui, shared, lines),
    );
    ui.add_space(GAP);
    card(ui, |ui| largest(ui, stats));
}

/// A card, as wide as there is room.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    sized_card(ui, 0.0, add);
}

/// A card, as wide as there is room and at least `least` tall; returns how tall its content
/// is.
fn sized_card(ui: &mut Ui, least: f32, add: impl FnOnce(&mut Ui)) -> f32 {
    let mut natural = 0.0;
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        // Reserved from the top before anything is added (egui's measures from the cursor).
        ui.set_min_height(least);
        natural = ui.scope(add).response.rect.height();
    });
    natural
}

/// Two cards side by side where there is room, as tall as each other (the taller's height
/// last frame, kept as `name`'s), else one above the other.
fn pair(ui: &mut Ui, name: &str, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    if ui.available_width() < TWO_COLUMNS {
        card(ui, left);
        ui.add_space(GAP);
        card(ui, right);
        return;
    }
    let key = ui.id().with(("stats-pair", name));
    let tallest: f32 = ui.ctx().data(|data| data.get_temp(key)).unwrap_or(0.0);
    let (mut first_height, mut second_height) = (0.0_f32, 0.0_f32);
    ui.spacing_mut().item_spacing.x = GAP;
    ui.columns(2, |columns| {
        if let [first, second] = columns {
            first_height = sized_card(first, tallest, left);
            second_height = sized_card(second, tallest, right);
        }
    });
    let wanted = first_height.max(second_height);
    if (wanted - tallest).abs() > 0.5 {
        ui.ctx().data_mut(|data| data.insert_temp(key, wanted));
        ui.ctx().request_repaint();
    }
}

/// The figures at the top, each in a tile of its own.
fn tiles(ui: &mut Ui, stats: &Stats, palette: Palette) {
    let now = format::now();
    let ago =
        |time: i64| u64::try_from(time).map_or_else(|_| String::new(), |time| long_ago(time, now));
    let mut tiles: Vec<(String, &str, Option<String>)> = Vec::with_capacity(8);
    let merges = (stats.merges > 0).then(|| {
        format!(
            "{} merges ({}%)",
            count(stats.merges),
            percent(stats.merges, stats.commits)
        )
    });
    tiles.push((
        count(stats.commits),
        noun(stats.commits, "commit", "commits"),
        merges,
    ));
    let authors = u64::try_from(stats.authors.len()).unwrap_or(u64::MAX);
    tiles.push((count(authors), noun(authors, "author", "authors"), None));
    let days = u64::try_from(stats.days.len()).unwrap_or(u64::MAX);
    tiles.push((
        count(days),
        noun(days, "day with commits", "days with commits"),
        None,
    ));
    if let Some(first) = stats.first {
        tiles.push((month_year(first), "first commit", Some(ago(first))));
    }
    if let Some(last) = stats.last {
        tiles.push((month_year(last), "latest commit", Some(ago(last))));
    }
    if let Some((length, end)) = stats.streak() {
        let days = format!(
            "{} {}",
            count(u64::from(length)),
            noun(u64::from(length), "day", "days")
        );
        tiles.push((
            days,
            "longest streak",
            Some(format!("to {}", day_text(end))),
        ));
    }
    if let Some((day, commits)) = stats.busiest() {
        let commits = u64::from(commits);
        tiles.push((
            format!("{} {}", count(commits), noun(commits, "commit", "commits")),
            "busiest day",
            Some(day_text(day)),
        ));
    }
    if let Some(files) = &stats.files {
        tiles.push((
            count(files.files),
            noun(files.files, "file at HEAD", "files at HEAD"),
            files.bytes.map(format::size),
        ));
    }
    // As many in a row as fit at their least width, sharing the row's width.
    let width = ui.available_width();
    let per_row = fits(width + GAP, TILE + GAP).max(1);
    let each = (width - GAP * (row_length(per_row) - 1.0)) / row_length(per_row);
    for row in tiles.chunks(per_row) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (value, label, note) in row {
                tile(ui, each, (value, label, note.as_deref()), palette);
            }
        });
        ui.add_space(GAP);
    }
}

/// The least width of a tile.
const TILE: f32 = 150.0;

/// One tile, `width` wide: its figure, what it counts, and a note beneath (an empty line
/// without one, so tiles are as tall as each other).
fn tile(
    ui: &mut Ui,
    width: f32,
    (value, label, note): (&str, &str, Option<&str>),
    palette: Palette,
) {
    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        Frame::new()
            .fill(palette.surface)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(CornerRadius::same(theme::RADIUS_BOX))
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(width - 30.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(Label::new(RichText::new(value).size(theme::TITLE).semibold()).truncate());
                ui.add(Label::new(muted(ui, label)).truncate());
                ui.add(Label::new(muted(ui, note.unwrap_or_default()).small()).truncate());
            });
    });
}

/// How many of `each` fit in `width`.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a few hundred first; float-to-int `as` saturates anyway"
)]
fn fits(width: f32, each: f32) -> usize {
    if each <= 0.0 {
        return 1;
    }
    (width / each).floor().clamp(1.0, 256.0) as usize
}

/// A row's length, as a width's divisor.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "a row of a few tiles"
)]
const fn row_length(tiles: usize) -> f32 {
    tiles as f32
}

/// How long ago `then` was, in the largest unit that makes sense: "3 weeks ago", "2 years ago".
fn long_ago(then: u64, now: u64) -> String {
    let days = now.saturating_sub(then).checked_div(86_400).unwrap_or(0);
    let (count, unit) = if days < 14 {
        return format::ago(then, now);
    } else if days < 60 {
        (days.checked_div(7).unwrap_or(0), "week")
    } else if days < 730 {
        (days.checked_div(30).unwrap_or(0), "month")
    } else {
        (days.checked_div(365).unwrap_or(0), "year")
    };
    format!("{count} {unit}s ago")
}

/// `one` or `many`, as `count` asks.
const fn noun<'a>(count: u64, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// `part` of `whole` in whole percent (at least 1 when there is any).
fn percent(part: u64, whole: u64) -> u64 {
    let percent = part.saturating_mul(100).checked_div(whole).unwrap_or(0);
    if part > 0 { percent.max(1) } else { 0 }
}

/// "Mar 2024", of a time.
fn month_year(time: i64) -> String {
    day_of(time)
        .and_then(date_of)
        .map_or_else(String::new, |date| date.strftime("%b %Y").to_string())
}

/// "Tue 12 Mar 2024", of a day.
fn day_text(day: i32) -> String {
    date_of(day).map_or_else(String::new, |date| {
        date.strftime("%a %-d %b %Y").to_string()
    })
}

/// "March 2024", of a month (since January 1970).
fn month_text(month: i32) -> String {
    let year = month.div_euclid(12).checked_add(1970);
    let month = month.rem_euclid(12).checked_add(1);
    year.zip(month)
        .and_then(|(year, month)| {
            let date =
                jiff::civil::Date::new(i16::try_from(year).ok()?, i8::try_from(month).ok()?, 1);
            date.ok()
        })
        .map_or_else(String::new, |date| date.strftime("%B %Y").to_string())
}

/// A count as a chart's coordinate.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "a count past 2^53 needs no more precision on a chart"
)]
const fn plotted(count: u64) -> f64 {
    count as f64
}

/// A chart's coordinate as a whole number, when it is one (within `i32`).
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "a whole number, checked to be within `i32` first"
)]
fn whole(value: f64) -> Option<i32> {
    let rounded = value.round();
    ((rounded - value).abs() < 1e-6
        && rounded >= f64::from(i32::MIN)
        && rounded <= f64::from(i32::MAX))
    .then_some(rounded as i32)
}

/// `part` of `whole`, between 0 and 1.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "a share drawn on screen: a pixel's precision is enough"
)]
fn share(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    ((part as f64) / (whole as f64)).clamp(0.0, 1.0) as f32
}

/// A chart in the client's look: no background or zooming (the page scrolls), a faint grid
/// across, counts along the side.
fn chart(
    id: impl std::hash::Hash + std::fmt::Debug,
    height: f32,
    palette: Palette,
) -> Plot<'static> {
    Plot::new(egui::Id::new(id))
        .height(height)
        .show_background(false)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show_grid([false, true])
        .grid_color(palette.border)
        .show_crosshair(false)
        .cursor_color(palette.border_strong)
        .y_axis_min_width(44.0)
        .include_y(0.0)
        .label_formatter(|_| None)
        .y_axis_formatter(|mark, _| {
            whole(mark.value)
                .map(|value| u64::from(value.unsigned_abs()))
                .map_or_else(String::new, count)
        })
}

/// Marks at each January (every year, or every few when there are many), named by the year.
fn years(input: &GridInput) -> Vec<GridMark> {
    let (low, high) = input.bounds;
    let span = (high - low) / 12.0;
    let step = if span <= 12.0 {
        1
    } else if span <= 30.0 {
        2
    } else if span <= 60.0 {
        5
    } else {
        10
    };
    let (Some(first), Some(last)) = (whole((low / 12.0).ceil()), whole((high / 12.0).floor()))
    else {
        return Vec::new();
    };
    (first..=last)
        .filter(|year| {
            year.checked_add(1970)
                .is_some_and(|year| year.checked_rem(step) == Some(0))
        })
        .filter_map(|year| year.checked_mul(12))
        .map(|month| GridMark {
            value: f64::from(month),
            step_size: f64::from(step.saturating_mul(12)),
        })
        .collect()
}

/// A January's year, as its mark.
fn year_label(mark: GridMark) -> String {
    whole(mark.value)
        .filter(|month| month.rem_euclid(12) == 0)
        .and_then(|month| month.div_euclid(12).checked_add(1970))
        .map_or_else(String::new, |year| year.to_string())
}

/// A bar of a chart, filled solid in `color` (egui's are faint, outlined).
fn bar(at: f64, value: f64, color: Color32, width: f64) -> Bar {
    Bar::new(at, value)
        .width(width)
        .fill(color)
        .stroke(egui::Stroke::new(1.0, color))
}

/// What a chart's pointer rests on: the whole argument nearest it (a month, a size), as the
/// frame before saw it (its bar lighter), kept under `id`. Its words show as a tooltip at the
/// pointer, never clipped by the chart (egui's own labels are drawn inside it).
struct Hover {
    id: egui::Id,
    lit: Option<i32>,
}

impl Hover {
    fn of(ui: &Ui, id: egui::Id) -> Self {
        Self {
            id,
            lit: ui.ctx().data(|data| data.get_temp(id)),
        }
    }

    /// `color`, lighter at the argument the pointer rests on.
    fn color(&self, at: i32, color: Color32, palette: Palette) -> Color32 {
        if self.lit == Some(at) {
            color.lerp_to_gamma(palette.text, 0.35)
        } else {
            color
        }
    }

    /// The argument the pointer rests on in the chart drawn (noted, to be lit next frame).
    fn note<R>(&self, shown: &egui_plot::PlotResponse<R>) -> Option<i32> {
        let now = shown
            .response
            .hover_pos()
            .and_then(|pointer| whole(shown.transform.value_from_position(pointer).x.round()));
        if now != self.lit {
            let ctx = &shown.response.ctx;
            ctx.data_mut(|data| match now {
                Some(at) => {
                    data.insert_temp(self.id, at);
                }
                None => data.remove::<i32>(self.id),
            });
            ctx.request_repaint();
        }
        now
    }
}

/// Names a chart for those who can't see it.
fn named(response: &egui::Response, name: &str) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, name));
}

/// Commits each month, from the first to the latest.
fn commits_chart(ui: &mut Ui, shared: &Shared<'_>, stats: &Stats) {
    let palette = shared.palette;
    widgets::heading(ui, "Commits each month");
    ui.add_space(4.0);
    let (Some(&(first, _)), Some(&(last, _))) = (stats.months.first(), stats.months.last()) else {
        return;
    };
    let hover = Hover::of(ui, egui::Id::new(("stats-commits-hover", shared.id)));
    let mut months = stats.months.iter().peekable();
    let bars: Vec<Bar> = (first..=last)
        .map(|month| {
            let commits = months
                .next_if(|(at, _)| *at == month)
                .map_or(0, |&(_, commits)| commits);
            let color = hover.color(month, palette.accent, palette);
            bar(f64::from(month), f64::from(commits), color, 0.85)
        })
        .collect();
    let bars = BarChart::new("Commits", bars).allow_hover(false);
    let shown = chart(("stats-commits", shared.id), 180.0, palette)
        .x_grid_spacer(|input| years(&input))
        .x_axis_formatter(|mark, _| year_label(mark))
        .show(ui, |plot| plot.bar_chart(bars));
    named(&shown.response, "Commits each month");
    if let Some(month) = hover
        .note(&shown)
        .filter(|month| (first..=last).contains(month))
    {
        let commits = u64::from(find(&stats.months, month).unwrap_or(0));
        shown.response.on_hover_text_at_pointer(format!(
            "{}: {} {}",
            month_text(month),
            count(commits),
            noun(commits, "commit", "commits")
        ));
    }
}

/// The figure `month` has among `months` (sorted), if any.
fn find(months: &[(i32, u32)], month: i32) -> Option<u32> {
    let at = months.binary_search_by_key(&month, |&(at, _)| at).ok()?;
    months.get(at).map(|&(_, figure)| figure)
}

/// How many authors made a commit each month.
fn active_chart(ui: &mut Ui, shared: &Shared<'_>, stats: &Stats) {
    let palette = shared.palette;
    widgets::heading(ui, "Authors active each month");
    ui.add(Label::new(muted(ui, "Everyone who made a commit that month.")).wrap());
    ui.add_space(4.0);
    let (Some(&(first, _)), Some(&(last, _))) = (stats.months.first(), stats.months.last()) else {
        return;
    };
    let mut active = stats.active.iter().peekable();
    let points: Vec<[f64; 2]> = (first..=last)
        .map(|month| {
            let authors = active
                .next_if(|(at, _)| *at == month)
                .map_or(0, |&(_, authors)| authors);
            [f64::from(month), f64::from(authors)]
        })
        .collect();
    let hover = Hover::of(ui, egui::Id::new(("stats-active-hover", shared.id)));
    // The month the pointer rests on, marked on the line.
    let marked = hover
        .lit
        .filter(|month| (first..=last).contains(month))
        .map(|month| {
            let authors = find(&stats.active, month).unwrap_or(0);
            egui_plot::Points::new("Hovered", vec![[f64::from(month), f64::from(authors)]])
                .radius(4.0)
                .color(palette.ok)
                .allow_hover(false)
        });
    let line = Line::new("Authors", PlotPoints::from(points))
        .color(palette.ok)
        .width(2.0)
        .fill(0.0)
        .fill_alpha(0.15)
        .allow_hover(false);
    let shown = chart(("stats-active", shared.id), 200.0, palette)
        .x_grid_spacer(|input| years(&input))
        .x_axis_formatter(|mark, _| year_label(mark))
        .show(ui, |plot| {
            plot.line(line);
            if let Some(marked) = marked {
                plot.points(marked);
            }
        });
    named(&shown.response, "Authors active each month");
    if let Some(month) = hover
        .note(&shown)
        .filter(|month| (first..=last).contains(month))
    {
        let authors = u64::from(find(&stats.active, month).unwrap_or(0));
        shown.response.on_hover_text_at_pointer(format!(
            "{}: {} {}",
            month_text(month),
            count(authors),
            noun(authors, "author", "authors")
        ));
    }
}

/// The year of contributions to the latest commit, a square a day (Monday at the top), darker
/// the more commits it had.
fn calendar(ui: &mut Ui, stats: &Stats, palette: Palette) {
    widgets::heading(ui, "Contributions");
    let Some(&(latest, _)) = stats.days.last() else {
        return;
    };
    // The calendar ends with the Sunday of the latest commit's week.
    let weekday = weekday_of(latest);
    let (Some(end), Some(length)) = (
        latest.checked_add(6_i32.saturating_sub(weekday)),
        i32::from(WEEKS).checked_mul(7),
    ) else {
        return;
    };
    let start = end.saturating_sub(length).saturating_add(1);
    let from = stats.days.partition_point(|&(day, _)| day < start);
    let mut days = stats.days.get(from..).unwrap_or_default().iter().peekable();
    let counts: Vec<u32> = (start..=end)
        .map(|day| {
            days.next_if(|(at, _)| *at == day)
                .map_or(0, |&(_, commits)| commits)
        })
        .collect();
    let total: u64 = counts.iter().copied().map(u64::from).sum();
    let most = counts.iter().copied().max().unwrap_or(0);
    ui.add(
        Label::new(muted(
            ui,
            format!(
                "{} {} in the year to {}.",
                count(total),
                noun(total, "commit", "commits"),
                day_text(latest)
            ),
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    calendar_grid(ui, (&counts, start, most), palette);
}

/// The calendar's squares, `counts` a day each from `start` (`most` the busiest's): a column a
/// week, with months' names over them and weekdays' beside, and its legend below.
fn calendar_grid(ui: &mut Ui, (counts, start, most): (&[u32], i32, u32), palette: Palette) {
    let labels = 32.0;
    let weeks = f32::from(WEEKS);
    let pitch = ((ui.available_width() - labels) / weeks).clamp(8.0, 18.0);
    let cell = pitch - 3.0;
    let size = vec2(labels + weeks * pitch, 16.0 + 7.0 * pitch + 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    named(&response, "Contributions calendar");
    let painter = ui.painter_at(rect);
    let grid = Rect::from_min_size(
        pos2(rect.min.x + labels, rect.min.y + 16.0),
        vec2(weeks * pitch, 7.0 * pitch),
    );
    let small = FontId::proportional(theme::SMALL);
    let mut hovered = None;
    let mut last_month = None;
    for (at, &commits) in counts.iter().enumerate() {
        let (Some(week), Some(row)) = (at.checked_div(7), at.checked_rem(7)) else {
            continue;
        };
        let (Ok(week), Ok(row)) = (u8::try_from(week), u8::try_from(row)) else {
            continue;
        };
        let place = pos2(
            grid.min.x + f32::from(week) * pitch,
            grid.min.y + f32::from(row) * pitch,
        );
        let square = Rect::from_min_size(place, vec2(cell, cell));
        painter.rect_filled(square, CornerRadius::same(2), level(commits, most, palette));
        let day = i32::try_from(at).ok().and_then(|at| start.checked_add(at));
        if response.hovered()
            && let Some(pointer) = response.hover_pos()
            && square.expand(1.5).contains(pointer)
        {
            hovered = day.map(|day| (day, commits));
        }
        // Each month's name over its first week.
        if row == 0
            && let Some(date) = day.and_then(date_of)
            && last_month != Some(date.month())
        {
            if last_month.is_some() && week < WEEKS.saturating_sub(2) {
                painter.text(
                    pos2(place.x, rect.min.y),
                    Align2::LEFT_TOP,
                    date.strftime("%b").to_string(),
                    small.clone(),
                    palette.muted,
                );
            }
            last_month = Some(date.month());
        }
    }
    for (row, name) in [(0_u8, "Mon"), (2, "Wed"), (4, "Fri")] {
        painter.text(
            pos2(rect.min.x, grid.min.y + f32::from(row) * pitch + cell / 2.0),
            Align2::LEFT_CENTER,
            name,
            small.clone(),
            palette.muted,
        );
    }
    legend(
        &painter,
        pos2(grid.max.x, grid.max.y + 8.0),
        (cell, pitch),
        palette,
    );
    if let Some((day, commits)) = hovered {
        let commits = u64::from(commits);
        response.on_hover_text_at_pointer(format!(
            "{} {} on {}",
            count(commits),
            noun(commits, "commit", "commits"),
            day_text(day)
        ));
    }
}

/// The calendar's legend, ending at `right`: "Less", a square of each shade, "More".
fn legend(painter: &egui::Painter, right: egui::Pos2, (cell, pitch): (f32, f32), palette: Palette) {
    let small = FontId::proportional(theme::SMALL);
    let more = painter.text(
        right,
        Align2::RIGHT_TOP,
        "More",
        small.clone(),
        palette.muted,
    );
    let mut x = more.min.x - 6.0 - 5.0 * pitch;
    for shade in [0_u32, 1, 2, 3, 4] {
        let square = Rect::from_min_size(pos2(x, right.y + 1.0), vec2(cell, cell));
        painter.rect_filled(square, CornerRadius::same(2), level(shade, 4, palette));
        x += pitch;
    }
    painter.text(
        pos2(more.min.x - 12.0 - 5.0 * pitch, right.y),
        Align2::RIGHT_TOP,
        "Less",
        small,
        palette.muted,
    );
}

/// A day's shade: none, then four steps of the accent up to the busiest.
fn level(commits: u32, most: u32, palette: Palette) -> Color32 {
    if commits == 0 || most == 0 {
        return palette.surface_alt;
    }
    let step = share(u64::from(commits), u64::from(most));
    let alpha = if step <= 0.25 {
        0.3
    } else if step <= 0.5 {
        0.5
    } else if step <= 0.75 {
        0.75
    } else {
        1.0
    };
    palette.accent.gamma_multiply(alpha)
}

/// The weekday of a day (since the Unix epoch), Monday 0.
fn weekday_of(day: i32) -> i32 {
    // The epoch was a Thursday.
    i64::from(day)
        .checked_add(3)
        .map_or(0, |days| i32::try_from(days.rem_euclid(7)).unwrap_or(0))
}

/// When in the week commits are made, by their authors' clocks: a circle for each weekday's
/// hour, larger the more commits.
fn punch_card(ui: &mut Ui, stats: &Stats, palette: Palette) {
    const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const DAYS: [&str; 7] = [
        "Mondays",
        "Tuesdays",
        "Wednesdays",
        "Thursdays",
        "Fridays",
        "Saturdays",
        "Sundays",
    ];
    widgets::heading(ui, "When commits are made");
    ui.add(Label::new(muted(ui, "Each weekday's hours, by their authors' clocks.")).wrap());
    ui.add_space(8.0);
    let most = stats.punch.iter().flatten().copied().max().unwrap_or(0);
    let labels = 34.0;
    let pitch = ((ui.available_width() - labels) / 24.0).clamp(10.0, 26.0);
    let size = vec2(labels + 24.0 * pitch, 7.0 * pitch + 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    named(&response, "Commits by weekday and hour");
    let painter = ui.painter_at(rect);
    let small = FontId::proportional(theme::SMALL);
    let hover = response.hover_pos();
    let mut hovered = None;
    for ((row, hours), name) in (0_u8..).zip(&stats.punch).zip(WEEKDAYS) {
        let y = rect.min.y + f32::from(row) * pitch + pitch / 2.0;
        painter.text(
            pos2(rect.min.x, y),
            Align2::LEFT_CENTER,
            name,
            small.clone(),
            palette.muted,
        );
        for (hour, &commits) in (0_u8..).zip(hours) {
            let center = pos2(
                rect.min.x + labels + f32::from(hour) * pitch + pitch / 2.0,
                y,
            );
            painter.circle_filled(center, 1.0, palette.border);
            if commits > 0 {
                let radius = pitch * 0.45 * share(u64::from(commits), u64::from(most)).sqrt();
                painter.circle_filled(center, radius.max(1.5), palette.accent);
            }
            if hover.is_some_and(|pointer| {
                (pointer.x - center.x).abs() <= pitch / 2.0
                    && (pointer.y - center.y).abs() <= pitch / 2.0
            }) {
                hovered = Some((usize::from(row), hour, commits));
            }
        }
    }
    for hour in (0_u8..24).step_by(3) {
        painter.text(
            pos2(
                rect.min.x + labels + f32::from(hour) * pitch + pitch / 2.0,
                rect.max.y,
            ),
            Align2::CENTER_BOTTOM,
            format!("{hour:02}"),
            small.clone(),
            palette.muted,
        );
    }
    if let Some((row, hour, commits)) = hovered {
        let commits = u64::from(commits);
        response.on_hover_text_at_pointer(format!(
            "{}, {hour:02}:00 to {:02}:00: {} {}",
            DAYS.get(row).copied().unwrap_or_default(),
            hour.saturating_add(1),
            count(commits),
            noun(commits, "commit", "commits")
        ));
    }
}

/// The authors with the most commits: their avatar, name, years, commits (a bar against the
/// most), and the lines they changed once counted.
fn authors(ui: &mut Ui, stats: &Stats, lines: Option<&LineStats>, palette: Palette) {
    widgets::heading(ui, "Most commits");
    ui.add(
        Label::new(muted(
            ui,
            "Through the mailmap: one person under each of their names.",
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    let most = stats.authors.first().map_or(0, |author| author.commits);
    for author in stats.authors.iter().take(AUTHORS_LISTED) {
        author_row(ui, author, most, lines, palette);
    }
    let more = stats.authors.len().saturating_sub(AUTHORS_LISTED);
    if more > 0 {
        let more = u64::try_from(more).unwrap_or(u64::MAX);
        ui.label(muted(
            ui,
            format!(
                "and {} more {}",
                count(more),
                noun(more, "author", "authors")
            ),
        ));
    }
}

/// One author's row.
fn author_row(
    ui: &mut Ui,
    author: &AuthorStats,
    most: u64,
    lines: Option<&LineStats>,
    palette: Palette,
) {
    let changed = lines.and_then(|lines| lines.authors.get(&author.email.to_lowercase()));
    let years = match (
        day_of(author.first).and_then(date_of),
        day_of(author.last).and_then(date_of),
    ) {
        (Some(first), Some(last)) if first.year() == last.year() => first.year().to_string(),
        (Some(first), Some(last)) => format!("{}–{}", first.year(), last.year()),
        _ => String::new(),
    };
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 40.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            widgets::avatar(ui, 26.0, (&author.name, &author.email), palette.surface);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let commits = format!(
                            "{} {}",
                            count(author.commits),
                            noun(author.commits, "commit", "commits")
                        );
                        ui.label(RichText::new(commits).color(palette.text));
                        if let Some(&(added, removed)) = changed {
                            ui.label(
                                RichText::new(format!("−{}", count(removed)))
                                    .color(palette.danger)
                                    .small(),
                            );
                            ui.label(
                                RichText::new(format!("+{}", count(added)))
                                    .color(palette.ok)
                                    .small(),
                            );
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(Label::new(RichText::new(&author.name).medium()).truncate())
                                .on_hover_text(&author.email);
                            ui.label(muted(ui, years).small());
                        });
                    });
                });
                let (bar, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
                ui.painter()
                    .rect_filled(bar, CornerRadius::same(2), palette.surface_alt);
                let filled = Rect::from_min_size(
                    bar.min,
                    vec2(bar.width() * share(author.commits, most), bar.height()),
                );
                ui.painter()
                    .rect_filled(filled, CornerRadius::same(2), palette.accent);
            });
        },
    );
}

/// The files at `HEAD` by language: a bar of their shares, and each language's.
fn languages(ui: &mut Ui, stats: &Stats, palette: Palette) {
    widgets::heading(ui, "Languages at HEAD");
    let Some(files) = &stats.files else {
        ui.label(muted(ui, "The files at HEAD couldn't be listed."));
        return;
    };
    // By bytes where they are known, else by files.
    let measure = |files: u64, bytes: u64| {
        if stats.files.as_ref().is_some_and(|all| all.bytes.is_some()) {
            bytes
        } else {
            files
        }
    };
    let total = measure(files.files, files.bytes.unwrap_or(0));
    let by = if files.bytes.is_some() {
        "by size"
    } else {
        "by files"
    };
    ui.add(
        Label::new(muted(
            ui,
            format!("{} files, {by}, from their names.", count(files.files)),
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    let mut shown: Vec<(&str, u64, u64)> = files
        .languages
        .iter()
        .take(LANGUAGES_SHOWN)
        .map(|language| {
            (
                language.name,
                measure(language.files, language.bytes),
                language.files,
            )
        })
        .collect();
    let rest = files.languages.iter().skip(LANGUAGES_SHOWN).fold(
        (0_u64, 0_u64),
        |(measured, count), language| {
            (
                measured.saturating_add(measure(language.files, language.bytes)),
                count.saturating_add(language.files),
            )
        },
    );
    if rest.1 > 0 {
        shown.push(("The rest", rest.0, rest.1));
    }
    let (bar, response) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    named(&response, "Languages' shares");
    let mut x = bar.min.x;
    let last = shown.len().saturating_sub(1);
    for (at, (&(_, measured, _), color)) in
        shown.iter().zip(palette.avatars.iter().cycle()).enumerate()
    {
        let width = bar.width() * share(measured, total);
        let corners = CornerRadius {
            nw: if at == 0 { 4 } else { 0 },
            sw: if at == 0 { 4 } else { 0 },
            ne: if at == last { 4 } else { 0 },
            se: if at == last { 4 } else { 0 },
        };
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(x, bar.min.y), vec2(width, bar.height())),
            corners,
            *color,
        );
        x += width;
    }
    ui.add_space(8.0);
    for (&(name, measured, files), color) in shown.iter().zip(palette.avatars.iter().cycle()) {
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 5.0, *color);
            ui.label(name);
            ui.label(muted(ui, format!("{}%", percent(measured, total))));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(muted(
                    ui,
                    format!("{} {}", count(files), noun(files, "file", "files")),
                ));
            });
        });
    }
}

/// The largest files at `HEAD`.
fn largest(ui: &mut Ui, stats: &Stats) {
    widgets::heading(ui, "Largest files at HEAD");
    let Some(files) = stats
        .files
        .as_ref()
        .filter(|files| !files.largest.is_empty())
    else {
        ui.label(muted(
            ui,
            "Their sizes aren't known in a partial clone without them.",
        ));
        return;
    };
    ui.add_space(4.0);
    let font = egui::TextStyle::Body.resolve(ui.style());
    for (path, size) in &files.largest {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format::size(*size)).monospace());
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let shown = crate::tables::elided_start(ui, path, &font, ui.available_width());
                    let elided = matches!(shown, std::borrow::Cow::Owned(_));
                    let label = ui.add(Label::new(shown.into_owned()).truncate());
                    if elided {
                        label.on_hover_text(path.as_str());
                    }
                });
            });
        });
    }
}

/// The lines the newest commits added (above) and removed (below) each month.
fn lines_chart(ui: &mut Ui, shared: &Shared<'_>, counted: &Counted) {
    let palette = shared.palette;
    widgets::heading(ui, "Lines changed");
    if counted.skipped {
        ui.add(
            Label::new(muted(
                ui,
                "Not counted in a partial clone: git would fetch every file's contents.",
            ))
            .wrap(),
        );
        return;
    }
    if let Some(problem) = &counted.problem {
        widgets::problem_box(ui, problem, |_| {});
        return;
    }
    let Some(lines) = counted.lines.as_deref() else {
        let (done, of) = counted.progress;
        widgets::working(
            ui,
            &format!("Counting lines: {} of {} commits…", count(done), count(of)),
        );
        return;
    };
    let which = if lines.commits >= LINES_COUNTED {
        format!("the newest {} commits", count(lines.commits))
    } else {
        "every commit".to_owned()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("+{}", count(lines.added))).color(palette.ok));
        ui.label(RichText::new(format!("−{}", count(lines.removed))).color(palette.danger));
        ui.label(muted(ui, format!("in {which}")));
    });
    ui.add_space(4.0);
    let hover = Hover::of(ui, egui::Id::new(("stats-lines-hover", shared.id)));
    let (added, removed): (Vec<Bar>, Vec<Bar>) = lines
        .months
        .iter()
        .map(|&(month, added, removed)| {
            let at = f64::from(month);
            (
                bar(
                    at,
                    plotted(added),
                    hover.color(month, palette.ok, palette),
                    0.85,
                ),
                bar(
                    at,
                    -plotted(removed),
                    hover.color(month, palette.danger, palette),
                    0.85,
                ),
            )
        })
        .unzip();
    let added = BarChart::new("Added", added).allow_hover(false);
    let removed = BarChart::new("Removed", removed).allow_hover(false);
    let shown = chart(("stats-lines", shared.id), 200.0, palette)
        .x_grid_spacer(|input| years(&input))
        .x_axis_formatter(|mark, _| year_label(mark))
        .show(ui, |plot| {
            plot.bar_chart(added);
            plot.bar_chart(removed);
        });
    named(&shown.response, "Lines added and removed each month");
    let hovered = hover.note(&shown).and_then(|month| {
        let at = lines
            .months
            .binary_search_by_key(&month, |&(at, ..)| at)
            .ok()?;
        lines.months.get(at).copied()
    });
    if let Some((month, added, removed)) = hovered {
        shown.response.on_hover_text_at_pointer(format!(
            "{}: +{} and −{} lines",
            month_text(month),
            count(added),
            count(removed)
        ));
    }
}

/// How many commits changed how many lines.
fn sizes_chart(ui: &mut Ui, shared: &Shared<'_>, lines: Option<&LineStats>) {
    const SIZES: [&str; 6] = ["none", "1–9", "10–99", "100–999", "1k–10k", "10k+"];
    let palette = shared.palette;
    widgets::heading(ui, "How large commits are");
    ui.add(Label::new(muted(ui, "Lines added and removed, a commit at a time.")).wrap());
    ui.add_space(4.0);
    let Some(lines) = lines else {
        return;
    };
    let hover = Hover::of(ui, egui::Id::new(("stats-sizes-hover", shared.id)));
    let bars: Vec<Bar> = (0_u8..)
        .zip(lines.sizes)
        .map(|(at, commits)| {
            let color = hover.color(i32::from(at), palette.accent, palette);
            bar(f64::from(at), plotted(commits), color, 0.7)
        })
        .collect();
    let bars = BarChart::new("Commits", bars).allow_hover(false);
    let shown = chart(("stats-sizes", shared.id), 200.0, palette)
        .x_grid_spacer(|_| {
            (0_u8..6)
                .map(|at| GridMark {
                    value: f64::from(at),
                    step_size: 1.0,
                })
                .collect()
        })
        .x_axis_formatter(|mark, _| {
            whole(mark.value)
                .and_then(|at| usize::try_from(at).ok())
                .and_then(|at| SIZES.get(at))
                .map_or_else(String::new, |size| (*size).to_owned())
        })
        .show(ui, |plot| plot.bar_chart(bars));
    named(&shown.response, "Commits by lines changed");
    let hovered = hover
        .note(&shown)
        .and_then(|at| usize::try_from(at).ok())
        .and_then(|at| Some((*SIZES.get(at)?, *lines.sizes.get(at)?)));
    if let Some((size, commits)) = hovered {
        shown.response.on_hover_text_at_pointer(format!(
            "{} {} changed {size} lines",
            count(commits),
            noun(commits, "commit", "commits")
        ));
    }
}
