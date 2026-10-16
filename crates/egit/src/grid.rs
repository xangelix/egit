//! The text grid: a monospace view of a measured text (`egit-text`), as a terminal draws it.
//!
//! Each visible row is one egui galley whose sections keep every glyph on its cells: a section
//! holds characters of one colour and one advance, spaced so each takes exactly its cells (two
//! for CJK and emoji, whatever the fallback font's advance). egui adds a section's letter spacing
//! before each of its glyphs but the row's first, so a section's leading space trades its own
//! spacing for the previous section's. Nothing off screen is laid out, and rows are
//! drawn down from the [`Position`] at the top, so a frame costs the same anywhere in a text and
//! a new width wraps around the same top line. Only the scroll bar uses [`Rows`], rebuilt when
//! the width or the text changes.

use std::collections::HashMap;
use std::ops::Range;

use egit_text::{Cells, Glyph, Measure, Position, Rows, Text, Wrap};
use egui::text::LayoutJob;
use egui::{
    Color32, CornerRadius, FontId, Id, Key, Modifiers, Painter, Rect, Response, Sense, Stroke,
    TextFormat, Ui, pos2, vec2,
};

use crate::fonts;
use crate::theme::Palette;

/// Cells of padding right of the text, where a wide character in a row's last cell hangs.
const OVERHANG: u32 = 1;

/// The scroll bars' thickness.
const BAR: f32 = 10.0;

/// The space between panes side by side.
const SEPARATOR: f32 = 2.0;

/// The fewest cells a wrapped row has.
const MIN_WRAP: u32 = 8;

/// A colour for some of a line's bytes, and maybe a background behind their cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Styled {
    /// The bytes, in the line.
    pub(crate) bytes: Range<u32>,
    /// Their colour.
    pub(crate) color: Color32,
    /// Behind their cells (over the line's background).
    pub(crate) background: Option<Color32>,
}

/// A background behind some of a row's cells (counted from the row's first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Highlight {
    cells: (u32, u32),
    color: Color32,
}

/// What a grid shows: a measured text, and how to colour it.
pub(crate) trait GridSource {
    /// The text.
    fn text(&self) -> &Text;

    /// Its measure.
    fn measure(&self) -> &Measure;

    /// Colours for line `line`'s bytes, in order and not overlapping (bytes outside them take
    /// the default text colour).
    fn styles(&self, _line: u32, _palette: &Palette, _out: &mut Vec<Styled>) {}

    /// The background of line `line`'s rows.
    fn background(&self, _line: u32, _palette: &Palette) -> Option<Color32> {
        None
    }

    /// What the gutter shows beside line `line`'s first row (its number, by default).
    fn gutter(&self, line: u32, out: &mut String) {
        use std::fmt::Write as _;
        let _ = write!(out, "{}", u64::from(line).saturating_add(1));
    }

    /// How many cells the gutter takes.
    fn gutter_cells(&self) -> u32 {
        self.measure().lines().max(1).ilog10().saturating_add(2)
    }
}

/// A text as it is, numbered.
#[derive(Debug)]
pub(crate) struct Plain {
    text: Text,
    measure: Measure,
}

impl Plain {
    /// `text`, measured (on the calling thread: a job's, for a large text).
    pub(crate) fn new(text: Text, options: egit_text::Options) -> Self {
        let measure = Measure::all(&text, options);
        Self { text, measure }
    }
}

impl GridSource for Plain {
    fn text(&self) -> &Text {
        &self.text
    }

    fn measure(&self) -> &Measure {
        &self.measure
    }
}

/// A text grid's state: where it is scrolled, and how it wraps.
#[derive(Debug)]
pub(crate) struct TextGrid {
    id: Id,
    /// The row at the top.
    top: Position,
    /// How far `top` is scrolled out of view, in points (less than a row).
    offset: f32,
    /// Cells scrolled sideways (unwrapped only).
    sideways: u32,
    wrap: bool,
    /// Where every row is at the current wrap, for the scroll bar, and the measure's size it
    /// was built from (to notice another text).
    rows: Option<(Rows, (u32, u32))>,
    /// Glyph advances of the characters drawn so far, in `font`.
    advances: HashMap<char, f32>,
    /// The font and font generation (see [`fonts::generation`]) `advances` were measured in.
    font: Option<(FontId, usize)>,
    /// The rows a screen holds (paging moves by as many).
    screen_rows: u32,
}

/// One frame's layout.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// A cell's width.
    cell: f32,
    /// A row's height.
    row: f32,
    wrap: Wrap,
    /// The cells an unwrapped row shows.
    visible_cells: u32,
}

impl TextGrid {
    pub(crate) fn new(id: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id),
            top: Position::default(),
            offset: 0.0,
            sideways: 0,
            wrap: false,
            rows: None,
            advances: HashMap::new(),
            font: None,
            screen_rows: 1,
        }
    }

    /// Whether long lines wrap.
    pub(crate) const fn wraps(&self) -> bool {
        self.wrap
    }

    /// Wraps long lines, or scrolls them sideways; the top line stays at the top.
    pub(crate) const fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
        self.sideways = 0;
    }

    /// The row at the top.
    #[cfg(test)]
    pub(crate) const fn top(&self) -> Position {
        self.top
    }

    /// Scrolls `line` to the top.
    pub(crate) const fn scroll_to_line(&mut self, line: u32) {
        self.top = Position { line, cell: 0 };
        self.offset = 0.0;
    }

    /// Draws `source` in the space left in `ui`.
    pub(crate) fn show(&mut self, ui: &mut Ui, source: &dyn GridSource) -> Response {
        self.show_panes(ui, &[source])
    }

    /// Draws `panes` side by side in the space left in `ui`, scrolled as one: the sides of a
    /// diff, whose lines line up one for one and take the same rows when wrapped (their
    /// measures matched). The first pane's measure moves the rows.
    pub(crate) fn show_panes(&mut self, ui: &mut Ui, panes: &[&dyn GridSource]) -> Response {
        let palette = Palette::of(ui.ctx());
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let measured_in = (font.clone(), fonts::generation(ui.ctx()));
        if self.font.as_ref() != Some(&measured_in) {
            self.advances.clear();
            self.font = Some(measured_in);
        }
        let (cell, row) =
            ui.fonts_mut(|fonts| (fonts.glyph_width(&font, '0'), fonts.row_height(&font)));
        let outer = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(outer, Sense::click_and_drag());
        let Some(first) = panes.first() else {
            return response;
        };
        let measure = first.measure();

        let bottom = outer.bottom() - if self.wrap { 0.0 } else { BAR };
        let rects = pane_rects(outer, bottom, panes, cell);
        let narrowest = rects
            .iter()
            .map(|(_, text)| text.width())
            .fold(f32::INFINITY, f32::min);
        let wrap = if self.wrap {
            Wrap::at(
                fit_cells(narrowest, cell)
                    .saturating_sub(OVERHANG)
                    .max(MIN_WRAP),
            )
        } else {
            Wrap::Off
        };
        let frame = Frame {
            cell,
            row,
            wrap,
            visible_cells: fit_cells(narrowest, cell).saturating_add(1),
        };
        self.screen_rows = fit_cells(bottom - outer.top(), row).max(1);
        self.index_rows(measure, wrap);

        // The top stays on a row start, in the text.
        if measure.lines() > 0 {
            self.top.line = self.top.line.min(measure.lines().saturating_sub(1));
        }
        self.top.cell = wrap.row_start(self.top.cell.min(measure.cells(self.top.line)));

        if response.hovered() {
            let delta = ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta));
            self.scroll(measure, &frame, -delta.y);
            self.scroll_sideways(measure, &frame, -delta.x / cell);
        }
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        if response.has_focus() {
            self.keys(ui, measure, &frame);
        }
        self.clamp(measure);

        let painter = ui.painter_at(outer);
        painter.rect_filled(outer, CornerRadius::ZERO, palette.surface);
        for (pane, (rect, text)) in panes.iter().zip(&rects) {
            painter.rect_filled(
                Rect::from_min_max(rect.min, pos2(text.left(), rect.bottom())),
                CornerRadius::ZERO,
                palette.panel,
            );
            self.draw_rows(ui, &painter, *pane, &font, &frame, *rect, *text, palette);
        }
        for (rect, _) in rects.iter().skip(1) {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(rect.left() - SEPARATOR, outer.top()),
                    pos2(rect.left(), outer.bottom()),
                ),
                CornerRadius::ZERO,
                palette.border,
            );
        }
        self.scroll_bar(ui, measure, outer, palette);
        if !self.wrap
            && let (Some((_, first_text)), Some((last, _))) = (rects.first(), rects.last())
        {
            let track = Rect::from_min_max(
                pos2(first_text.left(), outer.bottom() - BAR),
                pos2(last.right(), outer.bottom()),
            );
            self.sideways_bar(ui, measure, &frame, track, palette);
        }
        if response.has_focus() {
            painter.rect_stroke(
                outer.shrink(1.0),
                CornerRadius::ZERO,
                Stroke::new(1.0, palette.accent),
                egui::StrokeKind::Inside,
            );
        }
        response
    }

    /// Builds the row index when the wrap or the text changed.
    fn index_rows(&mut self, measure: &Measure, wrap: Wrap) {
        let size = (measure.lines(), measure.widest());
        let current = self
            .rows
            .as_ref()
            .is_some_and(|(rows, of)| rows.wrap() == wrap && *of == size);
        if !current {
            self.rows = Some((Rows::new(measure, wrap), size));
        }
    }

    /// Draws the rows from the top down, with their backgrounds and gutters.
    #[allow(clippy::too_many_arguments)]
    fn draw_rows(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        source: &dyn GridSource,
        font: &FontId,
        frame: &Frame,
        pane: Rect,
        text: Rect,
        palette: Palette,
    ) {
        let measure = source.measure();
        let text_painter = painter.with_clip_rect(
            text.expand2(vec2(
                frame.cell * f32::from(u16::try_from(OVERHANG).unwrap_or(1)),
                0.0,
            ))
            .intersect(pane),
        );
        let mut styles = Vec::new();
        let mut highlights = Vec::new();
        let mut gutter = String::new();
        let mut position = (measure.lines() > 0).then_some(self.top);
        let mut y = text.top() - self.offset;
        while let Some(at) = position
            && y < text.bottom()
        {
            if let Some(background) = source.background(at.line, &palette) {
                let rect = Rect::from_min_size(
                    pos2(pane.left(), y),
                    vec2(text.right() - pane.left(), frame.row),
                );
                painter.rect_filled(rect, CornerRadius::ZERO, background);
            }
            if at.cell == 0 {
                gutter.clear();
                source.gutter(at.line, &mut gutter);
                painter.text(
                    pos2(text.left() - frame.cell * 1.5, y),
                    egui::Align2::RIGHT_TOP,
                    &gutter,
                    font.clone(),
                    palette.muted,
                );
            }
            let cells = match frame.wrap {
                Wrap::At(width) => at.cell..at.cell.saturating_add(width),
                Wrap::Off => self.sideways..self.sideways.saturating_add(frame.visible_cells),
            };
            styles.clear();
            highlights.clear();
            source.styles(at.line, &palette, &mut styles);
            let mut job = LayoutJob::default();
            self.lay_out_row(
                ui,
                source,
                at.line,
                cells,
                &styles,
                font,
                frame,
                palette,
                &mut job,
                &mut highlights,
            );
            for highlight in &highlights {
                let x = |cell: u32| {
                    f32::from(u16::try_from(cell).unwrap_or(u16::MAX))
                        .mul_add(frame.cell, text.left())
                };
                let rect = Rect::from_min_max(
                    pos2(x(highlight.cells.0), y),
                    pos2(x(highlight.cells.1), y + frame.row),
                );
                text_painter.rect_filled(rect, CornerRadius::ZERO, highlight.color);
            }
            if !job.text.is_empty() {
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                text_painter.galley(pos2(text.left(), y), galley, palette.text);
            }
            y += frame.row;
            position = frame.wrap.next(measure, at);
        }
    }

    /// Lays out cells `cells` of line `line` into `job` (sections of one colour and one
    /// advance, spaced to their cells), and the styles' backgrounds into `highlights`.
    #[allow(clippy::too_many_arguments)]
    fn lay_out_row(
        &mut self,
        ui: &Ui,
        source: &dyn GridSource,
        line: u32,
        cells: Range<u32>,
        styles: &[Styled],
        font: &FontId,
        frame: &Frame,
        palette: Palette,
        job: &mut LayoutJob,
        highlights: &mut Vec<Highlight>,
    ) {
        let text = source.text();
        let measure = source.measure();
        let Some(bytes) = text.line(line) else {
            return;
        };
        let found = measure.seek(text, line, cells.start);
        let mut run = Run::new(font, frame.cell);
        let mut style = styles.iter().peekable();
        let mut shown = String::new();
        for cell in Cells::resume(bytes, found.byte, found.cell, measure.options()) {
            if cell.cell >= cells.end {
                break;
            }
            if cell.width == 0 {
                // A combining mark joins the character before it, in its section.
                if let Glyph::Char(c) = cell.glyph {
                    run.join(c);
                }
                continue;
            }
            if cell.cell < cells.start {
                // The tail of a tab, or a wide character hanging from the row before.
                for _ in 0..cell.end().saturating_sub(cells.start) {
                    run.push(job, ' ', frame.cell, 1, palette.text);
                }
                continue;
            }
            while style
                .next_if(|styled| styled.bytes.end <= cell.byte)
                .is_some()
            {}
            let covering = style
                .peek()
                .filter(|styled| styled.bytes.start <= cell.byte);
            let color = if cell.glyph.is_escaped() {
                palette.warn
            } else {
                covering.map_or(palette.text, |styled| styled.color)
            };
            if let Some(background) = covering.and_then(|styled| styled.background) {
                let from = cell.cell.saturating_sub(cells.start);
                let to = cell.end().saturating_sub(cells.start);
                match highlights.last_mut() {
                    Some(last) if last.cells.1 == from && last.color == background => {
                        last.cells.1 = to;
                    }
                    _ => highlights.push(Highlight {
                        cells: (from, to),
                        color: background,
                    }),
                }
            }
            match cell.glyph {
                Glyph::Char(c) => {
                    let advance = self.advance(ui, font, c, frame.cell);
                    run.push(job, c, advance, cell.width, color);
                }
                Glyph::Hidden => {}
                glyph => {
                    shown.clear();
                    glyph.write(cell.width, &mut shown);
                    for c in shown.chars() {
                        run.push(job, c, frame.cell, 1, color);
                    }
                }
            }
        }
        run.flush(job);
    }

    /// `c`'s advance in `font`: a cell for printable ASCII (the font is monospace), half a
    /// space for thin spaces (egui's own rule), looked up once for anything else.
    fn advance(&mut self, ui: &Ui, font: &FontId, c: char, cell: f32) -> f32 {
        if c.is_ascii_graphic() || c == ' ' {
            return cell;
        }
        if matches!(c, '\u{2009}' | '\u{202f}') {
            return cell / 2.0;
        }
        *self.advances.entry(c).or_insert_with(|| {
            let (advance, found) =
                ui.fonts_mut(|fonts| (fonts.glyph_width(font, c), fonts.has_glyph(font, c)));
            if !found {
                fonts::note_missing(ui.ctx(), c);
            }
            advance
        })
    }

    /// Scrolls by `points` (down when positive): the top moves a row for each row's height
    /// passed, and stops at either end.
    fn scroll(&mut self, measure: &Measure, frame: &Frame, points: f32) {
        if frame.row <= 0.0 || points == 0.0 || !points.is_finite() {
            return;
        }
        let offset = self.offset + points;
        let rows = (offset / frame.row).floor();
        self.offset = rows.mul_add(-frame.row, offset);
        let steps = whole_rows(rows);
        for _ in 0..steps.unsigned_abs() {
            let moved = if steps > 0 {
                frame.wrap.next(measure, self.top)
            } else {
                frame.wrap.previous(measure, self.top)
            };
            let Some(top) = moved else {
                self.offset = 0.0;
                break;
            };
            self.top = top;
        }
    }

    /// Scrolls sideways by `cells` (right when positive), when unwrapped.
    fn scroll_sideways(&mut self, measure: &Measure, frame: &Frame, cells: f32) {
        if self.wrap || cells == 0.0 || !cells.is_finite() {
            return;
        }
        let max = measure
            .widest()
            .saturating_sub(frame.visible_cells.saturating_sub(2));
        let step = fit_cells(cells.abs().max(1.0), 1.0);
        self.sideways = if cells > 0.0 {
            self.sideways.saturating_add(step).min(max)
        } else {
            self.sideways.saturating_sub(step)
        };
    }

    /// Keeps the last row from scrolling above the bottom.
    fn clamp(&mut self, measure: &Measure) {
        let Some((rows, _)) = &self.rows else {
            return;
        };
        let last_top = rows.total().saturating_sub(u64::from(self.screen_rows));
        if rows.row_of(measure, self.top) > last_top {
            let top = rows.at(measure, last_top);
            self.top = top;
            self.offset = 0.0;
        }
    }

    /// The keys that scroll a focused grid.
    fn keys(&mut self, ui: &Ui, measure: &Measure, frame: &Frame) {
        let page = f32::from(u16::try_from(self.screen_rows.saturating_sub(1).max(1)).unwrap_or(1));
        let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
        let command = |key| ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, key));
        if pressed(Key::ArrowDown) {
            self.scroll(measure, frame, frame.row);
        } else if pressed(Key::ArrowUp) {
            self.scroll(measure, frame, -frame.row);
        } else if pressed(Key::PageDown) || pressed(Key::Space) {
            self.scroll(measure, frame, frame.row * page);
        } else if pressed(Key::PageUp) {
            self.scroll(measure, frame, -frame.row * page);
        } else if command(Key::Home) {
            self.scroll_to_line(0);
        } else if command(Key::End) {
            self.scroll_to_line(measure.lines().saturating_sub(1));
        } else if pressed(Key::ArrowRight) {
            self.scroll_sideways(measure, frame, 4.0);
        } else if pressed(Key::ArrowLeft) {
            self.scroll_sideways(measure, frame, -4.0);
        } else if pressed(Key::Home) {
            self.sideways = 0;
        } else if pressed(Key::End) {
            self.scroll_sideways(measure, frame, f32::from(u16::MAX));
        }
    }

    /// The vertical scroll bar: its thumb is where the top row is among all rows.
    fn scroll_bar(&mut self, ui: &Ui, measure: &Measure, outer: Rect, palette: Palette) {
        let Some((rows, size)) = self.rows.take() else {
            return;
        };
        let track = Rect::from_min_max(pos2(outer.right() - BAR, outer.top()), outer.max);
        let screen = u64::from(self.screen_rows);
        let scrollable = rows.total().saturating_sub(screen);
        let thumb = (ratio(screen, rows.total().max(1)) * track.height())
            .max(24.0)
            .min(track.height());
        let at = ratio(rows.row_of(measure, self.top), scrollable.max(1));
        let thumb_rect = Rect::from_min_size(
            pos2(
                track.left() + 2.0,
                at.mul_add(track.height() - thumb, track.top()),
            ),
            vec2(BAR - 4.0, thumb),
        );
        if scrollable == 0 {
            self.rows = Some((rows, size));
            return;
        }
        let response = ui.interact(track, self.id.with("scroll bar"), Sense::click_and_drag());
        if (response.dragged() || response.clicked())
            && let Some(pointer) = response.interact_pointer_pos()
            && track.height() > thumb
        {
            let fraction = ((pointer.y - track.top() - thumb / 2.0) / (track.height() - thumb))
                .clamp(0.0, 1.0);
            let row = part(scrollable, fraction);
            self.top = rows.at(measure, row);
            self.offset = 0.0;
        }
        bar_thumb(ui, &response, thumb_rect, palette);
        self.rows = Some((rows, size));
    }

    /// The sideways scroll bar of an unwrapped grid.
    fn sideways_bar(
        &mut self,
        ui: &Ui,
        measure: &Measure,
        frame: &Frame,
        track: Rect,
        palette: Palette,
    ) {
        let widest = u64::from(measure.widest());
        let visible = u64::from(frame.visible_cells);
        if widest <= visible {
            return;
        }
        let scrollable = widest.saturating_sub(visible);
        let thumb = (ratio(visible, widest) * track.width())
            .max(24.0)
            .min(track.width());
        let at = ratio(u64::from(self.sideways), scrollable);
        let thumb_rect = Rect::from_min_size(
            pos2(
                at.mul_add(track.width() - thumb, track.left()),
                track.top() + 2.0,
            ),
            vec2(thumb, BAR - 4.0),
        );
        let response = ui.interact(track, self.id.with("sideways bar"), Sense::click_and_drag());
        if (response.dragged() || response.clicked())
            && let Some(pointer) = response.interact_pointer_pos()
            && track.width() > thumb
        {
            let fraction = ((pointer.x - track.left() - thumb / 2.0) / (track.width() - thumb))
                .clamp(0.0, 1.0);
            self.sideways = u32::try_from(part(scrollable, fraction)).unwrap_or(u32::MAX);
        }
        bar_thumb(ui, &response, thumb_rect, palette);
    }
}

/// Each pane's whole rectangle and its text's, side by side across `outer` (less the scroll
/// bar) down to `bottom`: its gutter's cells, a cell and a half, then its text.
fn pane_rects(outer: Rect, bottom: f32, panes: &[&dyn GridSource], cell: f32) -> Vec<(Rect, Rect)> {
    let count = f32::from(u16::try_from(panes.len()).unwrap_or(1));
    let pane_width = SEPARATOR.mul_add(-(count - 1.0), outer.width() - BAR) / count;
    (0..panes.len())
        .zip(panes)
        .map(|(index, pane)| {
            let index = f32::from(u16::try_from(index).unwrap_or(0));
            let left = (pane_width + SEPARATOR).mul_add(index, outer.left());
            let gutter = f32::from(u16::try_from(pane.gutter_cells()).unwrap_or(u16::MAX))
                .mul_add(cell, cell * 1.5);
            (
                Rect::from_min_max(pos2(left, outer.top()), pos2(left + pane_width, bottom)),
                Rect::from_min_max(
                    pos2(left + gutter, outer.top()),
                    pos2(left + pane_width, bottom),
                ),
            )
        })
        .collect()
}

/// A scroll bar's thumb, brighter while hovered or dragged.
fn bar_thumb(ui: &Ui, response: &Response, rect: Rect, palette: Palette) {
    let color = if response.dragged() || response.hovered() {
        palette.muted
    } else {
        palette.border
    };
    ui.painter().rect_filled(rect, CornerRadius::same(4), color);
}

/// Characters of one colour and one advance, spaced so each takes its cells.
#[derive(Debug)]
struct Run {
    font: FontId,
    cell: f32,
    text: String,
    /// The run's colour, advance, and cells per character.
    class: Option<(Color32, f32, u32)>,
    /// The previous section's spacing, owed after its last glyph.
    carry: f32,
}

impl Run {
    fn new(font: &FontId, cell: f32) -> Self {
        Self {
            font: font.clone(),
            cell,
            text: String::new(),
            class: None,
            carry: 0.0,
        }
    }

    /// Adds `c` (advance `advance`, taking `cells` cells); a new section starts when its colour
    /// or advance differs.
    fn push(&mut self, job: &mut LayoutJob, c: char, advance: f32, cells: u32, color: Color32) {
        let class = (color, advance, cells);
        if self.class != Some(class) {
            self.flush(job);
            self.class = Some(class);
        }
        self.text.push(c);
    }

    /// Adds a combining mark to the character before it.
    fn join(&mut self, mark: char) {
        if !self.text.is_empty() {
            self.text.push(mark);
        }
    }

    /// Appends the characters so far to `job` as a section.
    fn flush(&mut self, job: &mut LayoutJob) {
        let Some((color, advance, cells)) = self.class.take() else {
            return;
        };
        if self.text.is_empty() {
            return;
        }
        let spacing = f32::from(u16::try_from(cells).unwrap_or(1)).mul_add(self.cell, -advance);
        // egui puts a section's spacing before its first glyph too, unless it starts the row.
        let leading = if job.sections.is_empty() {
            self.carry
        } else {
            self.carry - spacing
        };
        job.append(
            &self.text,
            leading,
            TextFormat {
                font_id: self.font.clone(),
                color,
                extra_letter_spacing: spacing,
                ..TextFormat::default()
            },
        );
        self.carry = spacing;
        self.text.clear();
    }
}

/// How many whole cells `size` wide fit in `length`.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to u32's range first (NaN becomes 0); float-to-int `as` saturates anyway"
)]
fn fit_cells(length: f32, size: f32) -> u32 {
    if size <= 0.0 {
        return 0;
    }
    // The largest `f32` below `u32::MAX`.
    (length / size).floor().clamp(0.0, 4_294_967_040.0) as u32
}

/// `rows` (a whole number) as a count of rows to move, at most a billion either way.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "clamped to a range `i64` holds exactly; float-to-int `as` saturates anyway"
)]
const fn whole_rows(rows: f32) -> i64 {
    rows.clamp(-1.0e9, 1.0e9) as i64
}

/// `part / whole`, for placing a scroll bar's thumb (a pixel's precision is enough).
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "a scroll bar's precision is a pixel"
)]
fn ratio(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    ((part as f64) / (whole as f64)).clamp(0.0, 1.0) as f32
}

/// `fraction` (0 to 1) of `whole`.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a fraction of a count, clamped to it; float-to-int `as` saturates"
)]
fn part(whole: u64, fraction: f32) -> u64 {
    ((whole as f64) * f64::from(fraction.clamp(0.0, 1.0))).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_fit_whole() {
        assert_eq!(fit_cells(100.0, 8.0), 12);
        assert_eq!(fit_cells(4.0, 8.0), 0);
        assert_eq!(fit_cells(100.0, 0.0), 0);
        assert_eq!(fit_cells(f32::NAN, 1.0), 0);
        assert_eq!(fit_cells(f32::INFINITY, 1.0), 4_294_967_040);
    }

    #[test]
    fn thumbs_follow_the_fraction_scrolled() {
        assert!((ratio(1, 4) - 0.25).abs() < f32::EPSILON);
        assert!(ratio(5, 4) <= 1.0);
        assert!(ratio(1, 0).abs() < f32::EPSILON);
        assert_eq!(part(1_000_000, 0.5), 500_000);
        assert_eq!(part(10, 2.0), 10);
    }
}
