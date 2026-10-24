//! Code as a conversation shows it: boxes of monospace lines under a header saying what they
//! are, with Copy; coloured by their language in the background (plain until then); an edit
//! as a diff of its lines; and Claude's Markdown, its code blocks drawn as such boxes.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontId, Frame, Margin, RichText, Stroke, Ui, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use parking_lot::Mutex;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::grid::Span;
use crate::theme::{CODE, Palette, RADIUS_BOX, SMALL, icon};
use crate::widgets::muted;

/// The most lines a box shows (Copy takes them all).
const LINES: usize = 200;
/// How many snippets' colours and diffs are kept: past it, all are forgotten.
const KEPT: usize = 256;
/// How long Copy says it copied.
const COPIED: Duration = Duration::from_millis(1500);

/// A snippet's colours: each line's spans (`None`: its language isn't known).
type Colored = Option<Vec<Vec<Span>>>;

/// Snippets' colours, worked out in jobs and kept by what they are (language, text, theme);
/// and edits' diffs, by their two sides.
#[derive(Default)]
pub(crate) struct Highlights {
    done: Arc<Mutex<HashMap<u64, Arc<Colored>>>>,
    asked: HashSet<u64>,
    diffs: HashMap<u64, Arc<[Line]>>,
}

impl Highlights {
    /// `code`'s colours in `language`, once a job has worked them out (asked the first time;
    /// the window woken as they land).
    fn colors(
        &mut self,
        ctx: &egui::Context,
        language: &str,
        code: &str,
        palette: &Palette,
    ) -> Option<Arc<Colored>> {
        if language.is_empty() {
            return None;
        }
        let key = key(&[language, code], palette.dark);
        if let Some(done) = self.done.lock().get(&key) {
            return Some(Arc::clone(done));
        }
        if self.asked.len() >= KEPT {
            self.asked.clear();
            self.done.lock().clear();
        }
        if self.asked.insert(key) {
            let done = Arc::clone(&self.done);
            let (language, code) = (language.to_owned(), code.to_owned());
            let (dark, default, ctx) = (palette.dark, palette.text, ctx.clone());
            crate::jobs::detached(async move {
                let colored = crate::syntax::highlight_snippet(&language, &code, dark, default);
                done.lock().insert(key, Arc::new(colored));
                ctx.request_repaint();
            });
        }
        None
    }

    /// The lines of an edit from `old` to `new`, each kept, removed, or added.
    fn diff(&mut self, old: &str, new: &str) -> Arc<[Line]> {
        let key = key(&[old, new], false);
        if let Some(lines) = self.diffs.get(&key) {
            return Arc::clone(lines);
        }
        if self.diffs.len() >= KEPT {
            self.diffs.clear();
        }
        let lines: Arc<[Line]> = diff_lines(old, new).into();
        self.diffs.insert(key, Arc::clone(&lines));
        lines
    }
}

/// How many lines an edit from `old` to `new` adds and removes.
pub(super) fn counts(highlights: &mut Highlights, old: &str, new: &str) -> (usize, usize) {
    highlights
        .diff(old, new)
        .iter()
        .fold((0, 0), |(added, removed), line| match line {
            Line::Added(_) => (added.saturating_add(1), removed),
            Line::Removed(_) => (added, removed.saturating_add(1)),
            Line::Kept(_) => (added, removed),
        })
}

/// What texts are, as a key.
fn key(texts: &[&str], dark: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    texts.hash(&mut hasher);
    dark.hash(&mut hasher);
    hasher.finish()
}

/// A line of an edit: kept, removed, or added, and its place on its side (the new side for a
/// kept line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Kept(usize),
    Removed(usize),
    Added(usize),
}

/// An edit's lines in order: around each run of lines kept, those removed, then those added.
fn diff_lines(old: &str, new: &str) -> Vec<Line> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (old_kept, new_kept) = egit_text::kept(&old_lines, &new_lines);
    let mut lines = Vec::with_capacity(old_lines.len().saturating_add(new_lines.len()));
    let (mut at_old, mut at_new) = (0, 0);
    loop {
        while old_kept.get(at_old) == Some(&false) {
            lines.push(Line::Removed(at_old));
            at_old = at_old.saturating_add(1);
        }
        while new_kept.get(at_new) == Some(&false) {
            lines.push(Line::Added(at_new));
            at_new = at_new.saturating_add(1);
        }
        if at_old >= old_lines.len() && at_new >= new_lines.len() {
            break;
        }
        // Both at a line kept (the same line on each side).
        lines.push(Line::Kept(at_new));
        at_old = at_old.saturating_add(1);
        at_new = at_new.saturating_add(1);
    }
    lines
}

/// How a box is toned: as code, or as what failed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Plain,
    Failed,
}

/// A box of code: its header (`label`, and Copy taking all of `text`), then its lines,
/// coloured in `language` once known, at most [`LINES`] (how many more said).
pub(super) fn code_box(
    ui: &mut Ui,
    (label, language, tone): (&str, &str, Tone),
    text: &str,
    highlights: &mut Highlights,
) {
    let palette = Palette::of(ui.ctx());
    let shown = first_lines(text, LINES);
    let colors = highlights.colors(ui.ctx(), language, shown, &palette);
    framed(ui, tone, |ui| {
        header(ui, label, tone, Some(text));
        body(ui, |ui| {
            let mut job = LayoutJob::default();
            let font = FontId::monospace(CODE);
            let colored = colors.as_deref().and_then(Option::as_ref);
            for (index, line) in shown.lines().enumerate() {
                if index > 0 {
                    job.append("\n", 0.0, format(&font, palette.text));
                }
                let spans = colored.and_then(|lines| lines.get(index));
                append_line(
                    &mut job,
                    line,
                    spans.map(Vec::as_slice),
                    (&font, palette.text),
                );
            }
            wrapped(ui, job);
            let more = text.lines().count().saturating_sub(LINES);
            if more > 0 {
                let said = format!(
                    "… {} more lines",
                    crate::format::count(u64::try_from(more).unwrap_or(u64::MAX))
                );
                ui.label(muted(ui, said).size(SMALL));
            }
        });
    });
}

/// An edit as a diff: each line marked removed (−) or added (+) on its colour, the lines kept
/// between them plain; coloured in `language` once known.
pub(super) fn diff_box(
    ui: &mut Ui,
    (old, new): (&str, &str),
    language: &str,
    highlights: &mut Highlights,
) {
    let palette = Palette::of(ui.ctx());
    let lines = highlights.diff(old, new);
    let old_colors = highlights.colors(ui.ctx(), language, old, &palette);
    let new_colors = highlights.colors(ui.ctx(), language, new, &palette);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    framed(ui, Tone::Plain, |ui| {
        header(ui, "Changes", Tone::Plain, Some(new));
        body(ui, |ui| {
            let font = FontId::monospace(CODE);
            let mut job = LayoutJob::default();
            let mut fills = Vec::with_capacity(lines.len().min(LINES));
            for (index, line) in lines.iter().take(LINES).enumerate() {
                if index > 0 {
                    job.append("\n", 0.0, format(&font, palette.text));
                }
                let (mark, text, colors, fill) = match *line {
                    Line::Kept(at) => (" ", new_lines.get(at), &new_colors, None),
                    Line::Removed(at) => {
                        ("−", old_lines.get(at), &old_colors, Some(palette.removed))
                    }
                    Line::Added(at) => ("+", new_lines.get(at), &new_colors, Some(palette.added)),
                };
                let at = match *line {
                    Line::Kept(at) | Line::Removed(at) | Line::Added(at) => at,
                };
                let mark_color = match line {
                    Line::Kept(_) => palette.muted,
                    Line::Removed(_) => palette.danger,
                    Line::Added(_) => palette.ok,
                };
                job.append(mark, 0.0, format(&font, mark_color));
                job.append(" ", 0.0, format(&font, palette.text));
                let spans = colors
                    .as_deref()
                    .and_then(Option::as_ref)
                    .and_then(|lines| lines.get(at));
                append_line(
                    &mut job,
                    text.copied().unwrap_or_default(),
                    spans.map(Vec::as_slice),
                    (&font, palette.text),
                );
                fills.push(fill);
            }
            filled(ui, job, &fills);
            let more = lines.len().saturating_sub(LINES);
            if more > 0 {
                let said = format!(
                    "… {} more lines",
                    crate::format::count(u64::try_from(more).unwrap_or(u64::MAX))
                );
                ui.label(muted(ui, said).size(SMALL));
            }
        });
    });
}

/// Claude's Markdown: its top-level code blocks as code boxes (coloured by their fence's
/// language, but while `streaming`), the rest as Markdown.
pub(super) fn markdown(
    ui: &mut Ui,
    text: &str,
    (cache, highlights): (&mut CommonMarkCache, &mut Highlights),
    streaming: bool,
) {
    let parts = parts(text);
    let count = parts.len();
    for (index, part) in parts.into_iter().enumerate() {
        match part {
            Part::Markdown(markdown) => {
                CommonMarkViewer::new().show(ui, cache, markdown);
            }
            Part::Code { language, code } => {
                let label = if language.is_empty() {
                    "Code"
                } else {
                    language
                };
                let language = if streaming { "" } else { language };
                code_box(ui, (label, language, Tone::Plain), &code, highlights);
                if index.saturating_add(1) < count {
                    ui.add_space(4.0);
                }
            }
        }
    }
}

/// A piece of Markdown: text as it is, or a top-level code block's language and code.
#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Markdown(&'a str),
    Code { language: &'a str, code: String },
}

/// `text` split at its top-level code blocks (fenced or indented; one still open, as it
/// streams, runs to the end).
fn parts(text: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut last = 0;
    let mut code: Option<(&str, String)> = None;
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) if depth == 0 => {
                let before = text.get(last..range.start).unwrap_or_default();
                if !before.trim().is_empty() {
                    parts.push(Part::Markdown(before));
                }
                let language = match kind {
                    CodeBlockKind::Fenced(_) => {
                        fence_language(text.get(range.clone()).unwrap_or_default())
                    }
                    CodeBlockKind::Indented => "",
                };
                code = Some((language, String::new()));
                last = range.end;
                depth = depth.saturating_add(1);
            }
            Event::Start(_) => depth = depth.saturating_add(1),
            Event::End(TagEnd::CodeBlock) if depth == 1 && code.is_some() => {
                depth = 0;
                if let Some((language, mut body)) = code.take() {
                    if body.ends_with('\n') {
                        body.pop();
                    }
                    parts.push(Part::Code {
                        language,
                        code: body,
                    });
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(said) if depth == 1 => {
                if let Some((_, body)) = &mut code {
                    body.push_str(&said);
                }
            }
            _ => {}
        }
    }
    let rest = text.get(last..).unwrap_or_default();
    if !rest.trim().is_empty() {
        parts.push(Part::Markdown(rest));
    }
    parts
}

/// A fenced block's language: its info string's first word (` ```rust,ignore` is `rust`).
fn fence_language(block: &str) -> &str {
    let opening = block.lines().next().unwrap_or_default().trim_start();
    opening
        .trim_start_matches(['`', '~'])
        .split(|character: char| character.is_whitespace() || character == ',' || character == '{')
        .next()
        .unwrap_or_default()
}

/// The box around code: the panel's tone, an edge (the danger colour's for what failed).
fn framed(ui: &mut Ui, tone: Tone, add: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    let (fill, edge) = match tone {
        Tone::Plain => (palette.panel, palette.border),
        Tone::Failed => (palette.removed, palette.danger.gamma_multiply(0.5)),
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(RADIUS_BOX))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

/// A box's header: what it holds (small, muted; the danger colour's for what failed), and
/// Copy at its right (saying it copied, a moment), a hairline under them.
fn header(ui: &mut Ui, label: &str, tone: Tone, copy: Option<&str>) {
    let palette = Palette::of(ui.ctx());
    let shown = Frame::new()
        .inner_margin(Margin {
            left: 10,
            right: 4,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(crate::widgets::TARGET);
                let color = match tone {
                    Tone::Plain => palette.muted,
                    Tone::Failed => palette.danger,
                };
                ui.add(
                    egui::Label::new(RichText::new(label).size(SMALL).color(color))
                        .truncate()
                        .selectable(false),
                );
                let Some(text) = copy else {
                    return;
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    copy_button(ui, text);
                });
            });
        });
    let rect = shown.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border),
    );
}

/// Copy: `text` to the clipboard, saying so for [`COPIED`].
fn copy_button(ui: &mut Ui, text: &str) {
    let id = ui.next_auto_id();
    let copied_at: Option<Instant> = ui.data(|data| data.get_temp(id));
    let copied = copied_at.is_some_and(|at| at.elapsed() < COPIED);
    let words = if copied {
        format!("{}  Copied", icon::CHECK)
    } else {
        format!("{}  Copy", icon::COPY)
    };
    let response = crate::widgets::quiet(ui, |ui| {
        ui.add(
            egui::Button::new(RichText::new(words).size(SMALL))
                .frame_when_inactive(false)
                .min_size(vec2(crate::widgets::TARGET, crate::widgets::TARGET)),
        )
    });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Copy"));
    crate::help::note(ui, response.rect, "Copy");
    if response.clicked() {
        ui.ctx().copy_text(text.to_owned());
        ui.data_mut(|data| data.insert_temp(id, Instant::now()));
        ui.ctx().request_repaint_after(COPIED);
    }
}

/// A box's lines, inset from its edges.
fn body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// `job` as a label wrapped to the room there is (anywhere in a long word): read and selected
/// as text.
fn wrapped(ui: &mut Ui, mut job: LayoutJob) {
    job.wrap.break_anywhere = true;
    ui.add(egui::Label::new(job).wrap().selectable(true));
}

/// `job` as a label wrapped to the room there is, each of its lines' rows on its fill (across
/// the box), if it has one: laid out first to know where its rows fall (egui keeps the layout
/// for the label's own).
fn filled(ui: &mut Ui, job: LayoutJob, fills: &[Option<Color32>]) {
    let width = ui.available_width();
    let mut measured = job.clone();
    measured.wrap.max_width = width;
    measured.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(measured);
    let top_left = ui.cursor().min;
    // Across the box, to its edges.
    let across = egui::Rangef::new(top_left.x, top_left.x + width).expand(10.0);
    let mut line = 0_usize;
    for row in &galley.rows {
        if let Some(Some(fill)) = fills.get(line) {
            let rows = row.rect().translate(top_left.to_vec2()).y_range();
            ui.painter()
                .rect_filled(egui::Rect::from_x_y_ranges(across, rows), 0.0, *fill);
        }
        if row.ends_with_newline {
            line = line.saturating_add(1);
        }
    }
    wrapped(ui, job);
}

/// Appends `line`, its `spans` coloured (the rest `default`).
fn append_line(
    job: &mut LayoutJob,
    line: &str,
    spans: Option<&[Span]>,
    (font, default): (&FontId, Color32),
) {
    let Some(spans) = spans.filter(|spans| !spans.is_empty()) else {
        job.append(line, 0.0, format(font, default));
        return;
    };
    let mut at = 0;
    for span in spans {
        let start = usize::try_from(span.bytes.start)
            .unwrap_or(usize::MAX)
            .min(line.len());
        let end = usize::try_from(span.bytes.end)
            .unwrap_or(usize::MAX)
            .min(line.len());
        if start > at
            && let Some(plain) = line.get(at..start)
        {
            job.append(plain, 0.0, format(font, default));
        }
        if let Some(colored) = line.get(start.max(at)..end) {
            job.append(colored, 0.0, format(font, span.color));
        }
        at = at.max(end);
    }
    if let Some(rest) = line.get(at..).filter(|rest| !rest.is_empty()) {
        job.append(rest, 0.0, format(font, default));
    }
}

/// Monospace text in `color`.
fn format(font: &FontId, color: Color32) -> TextFormat {
    TextFormat {
        font_id: font.clone(),
        color,
        ..TextFormat::default()
    }
}

/// `text`'s first `most` lines.
fn first_lines(text: &str, most: usize) -> &str {
    let end = text
        .match_indices('\n')
        .nth(most.saturating_sub(1))
        .map_or(text.len(), |(at, _)| at);
    text.get(..end).unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_blocks_are_split_from_markdown() {
        let text = "Look:\n\n```rust,ignore\nfn a() {}\n```\n\nThen\n\n    indented\n\n- a list\n  ```\n  nested\n  ```\n";
        let split = parts(text);
        assert_eq!(split.len(), 5, "{split:?}");
        assert_eq!(split[0], Part::Markdown("Look:\n\n"));
        assert_eq!(
            split[1],
            Part::Code {
                language: "rust",
                code: "fn a() {}".to_owned()
            }
        );
        assert!(matches!(split[2], Part::Markdown(text) if text.contains("Then")));
        assert_eq!(
            split[3],
            Part::Code {
                language: "",
                code: "indented".to_owned()
            }
        );
        // A list's code block stays in the list's Markdown.
        assert!(matches!(split[4], Part::Markdown(text) if text.contains("nested")));
        // Still streaming: the open block runs to the end.
        let open = parts("Here:\n```py\nprint(1)\n");
        assert_eq!(
            open[1],
            Part::Code {
                language: "py",
                code: "print(1)".to_owned()
            }
        );
    }

    #[test]
    fn edits_read_as_lines_removed_then_added() {
        let lines = diff_lines("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(
            lines,
            [
                Line::Kept(0),
                Line::Removed(1),
                Line::Added(1),
                Line::Kept(2),
                Line::Added(3)
            ]
        );
        assert_eq!(diff_lines("", "x"), [Line::Added(0)]);
        assert_eq!(first_lines("1\n2\n3", 2), "1\n2");
        assert_eq!(first_lines("1\n2", 5), "1\n2");
    }
}
