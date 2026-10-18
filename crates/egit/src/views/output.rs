//! The output panel at a repository tab's bottom: what its tasks and commits printed as they ran,
//! hooks' output above all (`output.rs`). A slim bar shows what ran or runs (its hooks, for how
//! long, how it ended) once anything has; it opens into the lines, as a terminal shows them:
//! each command (`$ git commit --file=-`), its output in the colours its tools printed, how it
//! ended. Only the lines in view are laid out; the view follows new lines while at the bottom.
//! Its bar: show or hide it, the runs kept, Cancel while one runs, Copy, Clear.

use std::time::Instant;

use egit_git::{Color, Line, LineKind};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, ComboBox, FontId, Id, Label, Layout, RichText, Ui};

use crate::output::{Ended, Output, Run};
use crate::repo::RepoId;
use crate::theme::{Palette, icon};
use crate::widgets;

/// The bar's height: a control's, with a little room.
const BAR: f32 = crate::theme::CONTROL + 8.0;

/// Draws the panel (when anything ran) at the bottom of `ui`, before what fills the rest.
pub(super) fn show(ui: &mut Ui, output: &mut Output, id: RepoId) {
    if output.latest().is_none() {
        return;
    }
    let palette = Palette::of(ui.ctx());
    let frame = egui::Frame::new()
        .fill(palette.panel)
        .inner_margin(egui::Margin::symmetric(12, 0));
    // The lines first (the lowest), the bar above them; dragged shut, it closes.
    let mut open = output.open;
    egui::Panel::bottom(Id::new(("output-lines", id)))
        .resizable(true)
        .drag_to_open(false)
        .default_size(220.0)
        .size_range(80.0..=900.0)
        .frame(
            egui::Frame::new()
                .fill(palette.background)
                .inner_margin(egui::Margin::symmetric(12, 4)),
        )
        .show_collapsible(ui, &mut open, |ui| lines(ui, output, palette));
    if open != output.open {
        output.toggle();
    }
    egui::Panel::bottom(Id::new(("output-bar", id)))
        .resizable(false)
        .exact_size(BAR)
        .frame(frame)
        .show(ui, |ui| bar(ui, output, palette));
}

/// The bar: the way open or closed, the run shown and how it stands, the runs, its actions.
fn bar(ui: &mut Ui, output: &mut Output, palette: Palette) {
    crate::help::note_topic(ui, ui.max_rect(), "output-panel");
    ui.horizontal_centered(|ui| {
        let (glyph, name) = if output.open {
            (icon::CARET_DOWN, "Hide the output")
        } else {
            (icon::CARET_UP, "Show the output")
        };
        if widgets::icon_tool(ui, true, glyph, name)
            .on_hover_text(format!("{name} ({})", crate::commands::keys_text("⌘J")))
            .clicked()
        {
            output.toggle();
        }
        ui.label(
            RichText::new(format!("{}  Output", icon::TERMINAL_WINDOW))
                .color(palette.muted)
                .size(crate::theme::SMALL),
        );
        let now = Instant::now();
        if let Some(run) = output.shown() {
            status(ui, run, now, palette);
            if run.ended.is_none() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            actions(ui, output, now);
        });
    });
}

/// How `run` stands: running (its hooks), done, failed, cancelled, and for how long.
fn status(ui: &mut Ui, run: &Run, now: Instant, palette: Palette) {
    let seconds = run.elapsed(now).as_secs_f32();
    let (glyph, color, said) = match run.ended.map(|(_, ended)| ended) {
        None if run.hooks.is_empty() => (None, palette.text, run.title.clone()),
        None => (
            None,
            palette.text,
            format!("{} · running {}", run.title, run.hooks.join(", ")),
        ),
        Some(Ended::Done) => (
            Some(icon::CHECK_CIRCLE),
            palette.ok,
            format!("{} · done in {seconds:.1} s", run.title),
        ),
        Some(Ended::Stopped) => (
            Some(icon::WARNING_CIRCLE),
            palette.warn,
            format!("{} · stopped after {seconds:.1} s", run.title),
        ),
        Some(Ended::Failed) => (
            Some(icon::X_CIRCLE),
            palette.danger,
            format!("{} · failed after {seconds:.1} s", run.title),
        ),
        Some(Ended::Cancelled) => (
            Some(icon::PROHIBIT),
            palette.muted,
            format!("{} · cancelled", run.title),
        ),
    };
    match glyph {
        Some(glyph) => {
            ui.label(RichText::new(glyph).color(color));
        }
        None => {
            ui.spinner();
        }
    }
    ui.add(Label::new(RichText::new(said).color(color)).truncate());
    // While it runs, its seconds in a place of their own width: ticking, they move nothing.
    if run.ended.is_none() {
        widgets::seconds(ui, run.elapsed(now), palette.muted);
    }
}

/// The bar's right: Clear, Copy, the runs, Cancel (laid out from the right).
fn actions(ui: &mut Ui, output: &mut Output, now: Instant) {
    if widgets::icon_tool(ui, true, icon::TRASH, "Clear the output")
        .on_hover_text("Forget the runs that ended")
        .clicked()
    {
        output.clear();
    }
    let shown_text = output
        .shown()
        .map(|run| run.transcript.read(egit_git::Log::text));
    if let Some(text) = shown_text
        && widgets::icon_tool(ui, true, icon::COPY, "Copy the output")
            .on_hover_text("Every line of the run shown")
            .clicked()
    {
        ui.ctx().copy_text(text);
    }
    let picked = output.shown().map(|run| run.id);
    let label = |run: &Run| {
        let ago = now.saturating_duration_since(run.started).as_secs();
        format!("{} · {} · {}", run.title, how(run), ago_text(ago))
    };
    let mut chosen = picked;
    // What it shows stays put as time passes: how long ago only in the list.
    let selected = output
        .shown()
        .map(|run| format!("{} · {}", run.title, how(run)))
        .unwrap_or_default();
    let combo = ComboBox::from_id_salt("output-runs")
        .width(220.0)
        .truncate()
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for run in output.runs() {
                ui.selectable_value(&mut chosen, Some(run.id), label(run));
            }
        })
        .response;
    crate::help::note_topic(ui, combo.rect, "output-panel");
    if chosen != picked {
        output.picked = chosen;
        output.open = true;
    }
    if let Some(cancel) = output.shown().and_then(|run| run.cancel.clone()) {
        let stop = widgets::secondary(ui, !cancel.is_cancelled(), "Cancel");
        crate::help::note_topic(ui, stop.rect, "output-cancel");
        if stop.clicked() {
            cancel.cancel();
        }
    }
}

/// How `run` stands, in a word.
fn how(run: &Run) -> &'static str {
    match run.ended.map(|(_, ended)| ended) {
        None => "running",
        Some(Ended::Done) => "done",
        Some(Ended::Stopped) => "stopped",
        Some(Ended::Failed) => "failed",
        Some(Ended::Cancelled) => "cancelled",
    }
}

/// "12 s ago", "3 min ago", "2 h ago".
fn ago_text(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds.div_euclid(60)),
        _ => format!("{} h ago", seconds.div_euclid(3600)),
    }
}

/// The shown run's lines, those in view laid out, following new ones while at the bottom.
fn lines(ui: &mut Ui, output: &Output, palette: Palette) {
    let Some(run) = output.shown() else {
        return;
    };
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let count = run.transcript.read(egit_git::Log::len);
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::both()
        .id_salt(("output-lines", run.id))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show_rows(ui, height, count, |ui, rows| {
            run.transcript.read(|log| {
                for at in rows {
                    if let Some(line) = log.line(at) {
                        let job = layout(line, &font, palette);
                        ui.add(Label::new(job).extend().selectable(false));
                    }
                }
            });
        });
}

/// A line laid out: a command after `$ `, an end in its verdict's colour, the rest in the
/// colours its tools printed (the text's own elsewhere).
fn layout(line: &Line, font: &FontId, palette: Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |color: Color32| TextFormat::simple(font.clone(), color);
    match line.kind {
        LineKind::Command => {
            job.append("$ ", 0.0, format(palette.muted));
            job.append(&line.text, 0.0, format(palette.accent));
            return job;
        }
        LineKind::Ended { success } => {
            let color = if success { palette.ok } else { palette.danger };
            job.append(&line.text, 0.0, format(color));
            return job;
        }
        LineKind::Out | LineKind::Err => {}
    }
    let mut at = 0_usize;
    for span in &line.spans {
        let (start, end) = (
            usize::try_from(span.start).unwrap_or(usize::MAX),
            usize::try_from(span.end).unwrap_or(usize::MAX),
        );
        if let Some(plain) = line.text.get(at..start) {
            job.append(plain, 0.0, format(palette.text));
        }
        if let Some(styled) = line.text.get(start..end) {
            let color = span
                .style
                .color
                .map_or(palette.text, |color| terminal_color(color, palette));
            let color = if span.style.dim {
                color.gamma_multiply(0.7)
            } else {
                color
            };
            job.append(styled, 0.0, format(color));
        }
        at = end;
    }
    if let Some(rest) = line.text.get(at..) {
        job.append(rest, 0.0, format(palette.text));
    }
    job
}

/// A terminal's colour on egit's palette: the sixteen as the theme draws them (readable on its
/// background), the 256's cube and greys and exact colours as they are.
fn terminal_color(color: Color, palette: Palette) -> Color32 {
    match color {
        Color::Indexed(index @ 0..=15) => {
            let base = index & 7;
            let color = match base {
                1 => palette.danger,
                2 => palette.ok,
                3 => palette.warn,
                4 => palette.accent,
                5 => palette.lanes.get(5).copied().unwrap_or(palette.accent),
                6 => palette.lanes.get(2).copied().unwrap_or(palette.accent),
                // Black and white: the theme's quiet and strong text.
                0 => palette.muted,
                _ => palette.text,
            };
            if index > 7 && base != 0 && base != 7 {
                color.lerp_to_gamma(palette.text, 0.25)
            } else {
                color
            }
        }
        Color::Indexed(index @ 16..=231) => {
            let cube = index.saturating_sub(16);
            let level = |n: u8| {
                if n == 0 {
                    0
                } else {
                    55_u8.saturating_add(n.saturating_mul(40))
                }
            };
            let digit = |place: u8| cube.checked_div(place).and_then(|n| n.checked_rem(6));
            Color32::from_rgb(
                level(digit(36).unwrap_or(0)),
                level(digit(6).unwrap_or(0)),
                level(digit(1).unwrap_or(0)),
            )
        }
        Color::Indexed(index) => {
            Color32::from_gray(8_u8.saturating_add(index.saturating_sub(232).saturating_mul(10)))
        }
        Color::Rgb(red, green, blue) => Color32::from_rgb(red, green, blue),
    }
}
