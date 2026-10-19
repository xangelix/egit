//! A conversation's items on the reading background, only those in view laid out: prompts in
//! boxes of their own, Claude's words as Markdown (its code in boxes), thinking and tools as
//! lines folding open, a subagent's items stepped in under its call with its report last,
//! each turn's end, notices, and while Claude works, how long it has.

use std::collections::HashMap;
use std::time::Duration;

use egit_agents::{Item, ItemKind, Level, Notice, Prompt, TurnEnd};
use egui::{CornerRadius, Frame, Id, Margin, RichText, Stroke, Ui, vec2};

use super::code;
use super::tools::{self, Detail, Drawing, Line, STEP, State};
use super::{Around, ConversationView, INSET};
use crate::agents::Conversation;
use crate::app::Action;
use crate::stack::Entry;
use crate::theme::{Palette, RADIUS_BOX, SMALL, Weighted as _, icon};
use crate::widgets::{self, muted};

/// Room between items.
const GAP: f32 = 10.0;

/// A row of the transcript as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// An item, `depth` steps in (a subagent's, under its call).
    Item { index: usize, depth: u8 },
    /// An open subagent call's report, after its items.
    Report { index: usize, depth: u8 },
    /// Claude at work, or waiting for an answer: last.
    Working,
}

/// The rows shown and their entries, made again only when the transcript, what is open, or
/// whether Claude works changes.
#[derive(Default)]
pub(super) struct Rows {
    shown: Vec<Row>,
    entries: Vec<Entry>,
    made_for: Option<(u64, u64, Working)>,
}

/// What the working row says: nothing (there is none), that Claude Code is starting, that
/// Claude works, or that it waits for an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Working {
    No,
    Starting,
    Busy,
    Waiting,
}

impl Rows {
    /// Makes the rows again if what they show changed.
    fn follow(
        &mut self,
        items: &[Item],
        view: (u64, u64, &std::collections::HashSet<String>),
        (working, line): (Working, f32),
    ) {
        let (version, folds, open) = view;
        if self.made_for == Some((version, folds, working)) {
            return;
        }
        self.made_for = Some((version, folds, working));
        self.shown = rows(items, open);
        if working != Working::No {
            self.shown.push(Row::Working);
        }
        self.entries = self
            .shown
            .iter()
            .map(|row| entry(*row, items, open, line))
            .collect();
    }
}

/// The rows: each top-level item in turn, an open call's items under it (theirs too) and its
/// report after them.
fn rows(items: &[Item], open: &std::collections::HashSet<String>) -> Vec<Row> {
    let calls: HashMap<&str, usize> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match &item.kind {
            ItemKind::Tool(call) => Some((call.id.as_str(), index)),
            _ => None,
        })
        .collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut top = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item
            .parent
            .as_deref()
            .filter(|parent| calls.contains_key(parent))
        {
            Some(parent) => children.entry(parent).or_default().push(index),
            // A subagent's item whose call isn't here shows as any other.
            None => top.push(index),
        }
    }
    let mut rows = Vec::with_capacity(items.len());
    for index in top {
        push_rows(&mut rows, (index, 0), items, (&children, open));
    }
    rows
}

/// Pushes item `index`'s row, and if it is an open call, its items' and its report's.
fn push_rows(
    rows: &mut Vec<Row>,
    (index, depth): (usize, u8),
    items: &[Item],
    (children, open): (
        &HashMap<&str, Vec<usize>>,
        &std::collections::HashSet<String>,
    ),
) {
    rows.push(Row::Item { index, depth });
    let Some(ItemKind::Tool(call)) = items.get(index).map(|item| &item.kind) else {
        return;
    };
    let Some(under) = children
        .get(call.id.as_str())
        .filter(|_| open.contains(&call.id))
    else {
        return;
    };
    let deeper = depth.saturating_add(1);
    for child in under {
        push_rows(rows, (*child, deeper), items, (children, open));
    }
    if call.result.is_some() {
        rows.push(Row::Report {
            index,
            depth: deeper,
        });
    }
}

/// A row's entry: what it shows (its version: measured again when it changes) and a guess at
/// its height.
fn entry(row: Row, items: &[Item], open: &std::collections::HashSet<String>, line: f32) -> Entry {
    let mix = |a: u64, b: u64| a.rotate_left(23) ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    match row {
        Row::Item { index, depth } | Row::Report { index, depth } => {
            let Some(item) = items.get(index) else {
                return Entry {
                    version: 0,
                    guess: line,
                };
            };
            let report = matches!(row, Row::Report { .. });
            let is_open = open.contains(&key_of(index, item));
            let place = u64::try_from(index).unwrap_or(u64::MAX);
            let version = mix(
                mix(mix(place, item.changed), u64::from(depth)),
                u64::from(is_open) | (u64::from(report) << 1),
            );
            Entry {
                version,
                guess: if report {
                    3.0 * line
                } else {
                    guess(item, line)
                },
            }
        }
        Row::Working => Entry {
            version: u64::MAX,
            guess: line,
        },
    }
}

/// A guess at an item's height before it is drawn.
fn guess(item: &Item, line: f32) -> f32 {
    let lines = |text: &str| {
        let wraps = text.len().checked_div(60).unwrap_or(0);
        let count = text.lines().count().saturating_add(wraps);
        f32::from(u16::try_from(count.max(1)).unwrap_or(u16::MAX))
    };
    match &item.kind {
        ItemKind::Prompt(prompt) => lines(&prompt.text).mul_add(line, 24.0),
        ItemKind::Text(text) => lines(text) * line * 1.3,
        ItemKind::Thinking(_) | ItemKind::Tool(_) => 26.0,
        ItemKind::Turn(_) | ItemKind::Notice(_) => line,
    }
}

/// What folds an item open: its tool call's id, or its place.
fn key_of(index: usize, item: &Item) -> String {
    match &item.kind {
        ItemKind::Tool(call) => call.id.clone(),
        _ => format!("item-{index}"),
    }
}

/// The conversation's items, those in view laid out.
pub(super) fn show(
    ui: &mut Ui,
    conversation: &mut Conversation,
    around: Around<'_>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    if conversation.history.is_pending() {
        Frame::new()
            .inner_margin(Margin::same(INSET))
            .show(ui, |ui| widgets::working(ui, "Reading the conversation…"));
        return;
    }
    let working = if !conversation.transcript.busy() {
        Working::No
    } else if !conversation.waiting.is_empty() {
        Working::Waiting
    } else if conversation.is_starting() {
        Working::Starting
    } else {
        Working::Busy
    };
    let elapsed = conversation.turn_time();
    let Conversation {
        transcript,
        view,
        waiting,
        ..
    } = conversation;
    let items = transcript.items();
    if items.is_empty() {
        Frame::new()
            .inner_margin(Margin::same(INSET))
            .show(ui, |ui| welcome(ui, around));
        return;
    }
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let ConversationView {
        stack,
        open,
        folds,
        rows,
        markdown,
        highlights,
        ..
    } = view;
    rows.follow(items, (transcript.version(), *folds, open), (working, line));
    let busy = transcript.busy();
    let last = items.len().saturating_sub(1);
    let mut drawing = Drawing {
        open,
        toggled: None,
        markdown,
        highlights,
        work_tree: around.work_tree,
        waiting,
        repo: around.repo,
        actions,
        palette,
    };
    let Rows { shown, entries, .. } = rows;
    stack.show(
        ui,
        Id::new(("agents-transcript", around.work_tree)),
        entries,
        (GAP, vec2(f32::from(INSET), f32::from(INSET))),
        |ui, at| {
            let Some(row) = shown.get(at).copied() else {
                return;
            };
            match row {
                Row::Item { index, depth } => {
                    let Some(item) = items.get(index) else {
                        return;
                    };
                    stepped(ui, depth, |ui| {
                        item_view(ui, (index, item), &mut drawing, busy && index == last);
                    });
                }
                Row::Report { index, depth } => {
                    if let Some(item) = items.get(index) {
                        stepped(ui, depth, |ui| report(ui, item, &mut drawing));
                    }
                }
                Row::Working => working_row(ui, working, elapsed, &palette),
            }
        },
    );
    if let Some(key) = drawing.toggled.take() {
        view.toggle(&key);
    }
}

/// What a conversation shows before its first prompt.
fn welcome(ui: &mut Ui, around: Around<'_>) {
    let name = around
        .work_tree
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    ui.add_space(8.0);
    ui.label(
        RichText::new(icon::ROBOT)
            .size(28.0)
            .color(Palette::of(ui.ctx()).muted),
    );
    ui.add_space(4.0);
    let title = around.elsewhere.map_or_else(
        || format!("Ask Claude to work in {name}"),
        |host| format!("Ask Claude to work in {name}, on {host}"),
    );
    ui.add(egui::Label::new(RichText::new(title).semibold()).wrap());
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(muted(
            ui,
            "It reads this repository's CLAUDE.md and your settings, and asks before changing \
             anything unless its mode says otherwise. What it changes shows in the work tree, \
             to review and commit.",
        ))
        .wrap(),
    );
    ui.add_space(12.0);
    for (glyph, tip) in [
        (
            icon::FILE_TEXT,
            "Name files by their path; it reads them itself.",
        ),
        (
            icon::IMAGE,
            "Attach screenshots: drop them on the window, or the paperclip.",
        ),
        (icon::TERMINAL, "Type / for Claude Code's commands."),
        (icon::CHATS, "Earlier conversations are in the menu above."),
    ] {
        ui.horizontal_top(|ui| {
            ui.label(muted(ui, glyph));
            ui.add(egui::Label::new(muted(ui, tip)).wrap());
        });
    }
}

/// Draws `add` `depth` steps in, a guide down each step's side, joined across the gap above.
fn stepped(ui: &mut Ui, depth: u8, add: impl FnOnce(&mut Ui)) {
    if depth == 0 {
        add(ui);
        return;
    }
    let indent = f32::from(depth) * STEP;
    let shown = ui
        .horizontal_top(|ui| {
            ui.add_space(indent);
            ui.vertical(add);
        })
        .response;
    let palette = Palette::of(ui.ctx());
    for step in 0..depth {
        let x = shown.rect.left() + f32::from(step).mul_add(STEP, 10.0);
        ui.painter().vline(
            x,
            (shown.rect.top() - GAP)..=shown.rect.bottom(),
            Stroke::new(1.0, palette.border),
        );
    }
}

/// One item.
fn item_view(
    ui: &mut Ui,
    (index, item): (usize, &Item),
    drawing: &mut Drawing<'_>,
    streaming: bool,
) {
    match &item.kind {
        ItemKind::Prompt(prompt) => prompt_box(ui, prompt, &drawing.palette),
        ItemKind::Text(text) => {
            code::markdown(ui, text, (drawing.markdown, drawing.highlights), streaming);
            if streaming {
                ui.ctx().request_repaint();
            }
        }
        ItemKind::Thinking(text) => thinking(ui, (index, item), text, drawing),
        ItemKind::Tool(call) => tools::tool(ui, call, drawing),
        ItemKind::Turn(end) => turn(ui, end, &drawing.palette),
        ItemKind::Notice(notice) => notice_view(ui, notice, &drawing.palette),
    }
}

/// What the user asked: in a box of its own, the images that went with it under it.
fn prompt_box(ui: &mut Ui, prompt: &Prompt, palette: &Palette) {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(RADIUS_BOX))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !prompt.text.is_empty() {
                ui.add(
                    egui::Label::new(prompt.text.as_str())
                        .wrap()
                        .selectable(true),
                );
            }
            if !prompt.attachments.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for name in &prompt.attachments {
                        ui.label(muted(ui, format!("{}  {name}", icon::IMAGE)).size(SMALL));
                    }
                });
            }
        });
}

/// Claude's thinking: a line saying its first words, opening onto all of it.
fn thinking(ui: &mut Ui, (index, item): (usize, &Item), text: &str, drawing: &mut Drawing<'_>) {
    let key = key_of(index, item);
    let is_open = drawing.open.contains(&key);
    let preview = super::first_line(text.trim_start());
    let (response, _) = tools::line(
        ui,
        &Line {
            open: Some(is_open),
            glyph: icon::BRAIN,
            verb: "Thinking",
            detail: (if is_open { "" } else { preview }, Detail::Words),
            note: None,
            state: State::Done,
        },
        None,
    );
    if response.clicked() {
        drawing.toggled = Some(key);
    }
    if is_open {
        tools::under(ui, |ui| {
            ui.add(
                egui::Label::new(muted(ui, text).italics())
                    .wrap()
                    .selectable(true),
            );
        });
    }
}

/// An open subagent call's report, after its items.
fn report(ui: &mut Ui, item: &Item, drawing: &mut Drawing<'_>) {
    let ItemKind::Tool(call) = &item.kind else {
        return;
    };
    let Some(result) = &call.result else {
        return;
    };
    let palette = drawing.palette;
    ui.label(
        RichText::new(format!("{}  Its report", icon::ARROW_ELBOW_DOWN_RIGHT))
            .size(SMALL)
            .color(palette.muted),
    );
    if result.error {
        ui.add(egui::Label::new(RichText::new(result.text.as_str()).color(palette.danger)).wrap());
    } else {
        code::markdown(
            ui,
            &result.text,
            (drawing.markdown, drawing.highlights),
            false,
        );
    }
}

/// A turn's end: how long it took, its tokens and what Claude Code estimates it cost, a
/// hairline after them; or why it failed.
fn turn(ui: &mut Ui, end: &TurnEnd, palette: &Palette) {
    if let Some(error) = &end.error {
        notice_view(
            ui,
            &Notice {
                level: Level::Error,
                text: error.clone(),
            },
            palette,
        );
        return;
    }
    let seconds = f64::from(u32::try_from(end.duration_ms).unwrap_or(u32::MAX)) / 1000.0;
    let tokens = end.input_tokens.saturating_add(end.output_tokens);
    let said = format!(
        "{}  {seconds:.1} s · {} tokens · ≈ ${:.2}",
        icon::CHECK,
        crate::format::count(tokens),
        end.cost_usd
    );
    ui.horizontal(|ui| {
        let label = ui.label(muted(ui, said).size(SMALL)).on_hover_text(
            "How long the turn took, the tokens it read and wrote, and what Claude Code \
                 estimates it cost (a plan's usage isn't billed by it)",
        );
        let y = label.rect.center().y;
        let from = label.rect.right() + 8.0;
        let to = ui.max_rect().right();
        if to > from {
            ui.painter()
                .hline(from..=to, y, Stroke::new(1.0, palette.border));
        }
    });
}

/// A notice: its icon and words in its level's colour; a warning's or failure's on a tint.
fn notice_view(ui: &mut Ui, notice: &Notice, palette: &Palette) {
    let (glyph, color) = match notice.level {
        Level::Info => (icon::INFO, palette.muted),
        Level::Warning => (icon::WARNING, palette.warn),
        Level::Error => (icon::WARNING_CIRCLE, palette.danger),
    };
    let text = RichText::new(format!("{glyph}  {}", notice.text)).color(color);
    if notice.level == Level::Info {
        ui.add(egui::Label::new(text.size(SMALL)).wrap().selectable(true));
        return;
    }
    Frame::new()
        .fill(color.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.4)))
        .corner_radius(CornerRadius::same(RADIUS_BOX))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(text).wrap().selectable(true));
        });
}

/// Claude at work (and for how long), starting, or waiting for an answer below.
fn working_row(ui: &mut Ui, working: Working, elapsed: Option<Duration>, palette: &Palette) {
    ui.horizontal(|ui| {
        ui.set_height(crate::widgets::TARGET);
        match working {
            Working::Waiting => {
                ui.label(RichText::new(icon::HAND_PALM).color(palette.warn));
                ui.label(
                    RichText::new("Waiting for your answer, below")
                        .color(palette.warn)
                        .medium(),
                );
            }
            Working::Starting => {
                ui.add(egui::Spinner::new().size(14.0).color(palette.accent));
                ui.label(muted(ui, "Starting Claude Code…"));
            }
            Working::Busy | Working::No => {
                ui.add(egui::Spinner::new().size(14.0).color(palette.accent));
                // Last on its line: its words growing move nothing.
                let seconds = elapsed.map_or(0, |elapsed| elapsed.as_secs());
                let said = if seconds == 0 {
                    "Working…".to_owned()
                } else {
                    format!("Working… {seconds} s")
                };
                ui.label(muted(ui, said));
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
        }
    });
}
