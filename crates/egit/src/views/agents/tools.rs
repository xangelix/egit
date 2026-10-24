//! A tool Claude used, as a line: its icon, what it did, what to (a path from the work tree,
//! a command, a search), a note (an edit's lines, a plan's progress), and how it stands
//! (running, waiting for you, failed). Opened, what it was given and gave back, as suits it: a
//! command and its output, an edit as a diff, a file written, a plan's steps, a subagent's
//! instructions. A file it touched opens from its line: its changes, or the file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egit_agents::ToolCall;
use egit_agents::protocol::PermissionRequest;
use egui::{Align2, Color32, FontId, Rect, Response, RichText, Sense, Ui, pos2, vec2};
use egui_commonmark::CommonMarkCache;
use serde_json::Value;

use super::code::{self, Highlights, Tone};
use crate::app::Action;
use crate::repo::RepoId;
use crate::theme::{BODY, CODE, Palette, RADIUS_INNER, SMALL, Weight, Weighted as _, icon};
use crate::widgets::{self, muted};

/// A line's height.
const LINE: f32 = 26.0;
/// How far what is under a line steps in: under its words, past its caret and icon.
pub(super) const STEP: f32 = 20.0;

/// What drawing a conversation's items works with.
pub(super) struct Drawing<'a> {
    /// What is folded open, and what a click asked to open or fold this frame.
    pub(super) open: &'a HashSet<String>,
    pub(super) toggled: Option<String>,
    pub(super) markdown: &'a mut CommonMarkCache,
    pub(super) highlights: &'a mut Highlights,
    pub(super) work_tree: &'a Path,
    /// The permission requests waiting (their tools' lines say so).
    pub(super) waiting: &'a [PermissionRequest],
    pub(super) repo: RepoId,
    pub(super) actions: &'a mut Vec<Action>,
    pub(super) palette: Palette,
}

/// How a line's tool stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Running,
    Waiting,
    Done,
    Failed,
}

/// What a line's detail is, for how it is cut when it doesn't fit: a path loses its start, the
/// rest their end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Detail {
    Path,
    Code,
    Words,
}

/// A line, as drawn.
pub(super) struct Line<'a> {
    /// Whether it is open (`None`: nothing under it to open).
    pub(super) open: Option<bool>,
    pub(super) glyph: &'a str,
    pub(super) verb: &'a str,
    pub(super) detail: (&'a str, Detail),
    /// Said after the detail, small (an edit's lines, a plan's progress).
    pub(super) note: Option<(String, Color32)>,
    pub(super) state: State,
}

/// Draws a line; returns its response (clicked: opened or folded), and whether its link (a
/// file to show, `link` naming it) was clicked.
pub(super) fn line(ui: &mut Ui, shown: &Line<'_>, link: Option<&str>) -> (Response, bool) {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    let link_width = if link.is_some() { LINE + 2.0 } else { 0.0 };
    // Only a line with something under it clicks: the rest reads as a label.
    let (sense, kind) = if shown.open.is_some() {
        (Sense::click(), egui::WidgetType::Button)
    } else {
        (Sense::hover(), egui::WidgetType::Label)
    };
    let (rect, response) =
        ui.allocate_exact_size(vec2((width - link_width).max(LINE), LINE), sense);
    let name = match shown.detail.0 {
        "" => shown.verb.to_owned(),
        detail => format!("{} {detail}", shown.verb),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(kind, true, name.as_str()));
    crate::help::note_topic(ui, rect, "agents-tools");
    if shown.open.is_some() {
        widgets::row_fill(ui, rect, f32::from(RADIUS_INNER), false, response.hovered());
    }
    paint_line(ui, rect, shown, &palette);
    let linked = link.is_some_and(|link| {
        let at = Rect::from_min_size(pos2(rect.right() + 2.0, rect.top()), vec2(LINE, LINE));
        link_button(ui, at, response.id.with("link"), link)
    });
    let response = if shown.open.is_some() {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    (response, linked)
}

/// The button at a line's right showing the file its tool touched (`name`: "Show src/a.rs");
/// returns whether it was clicked.
fn link_button(ui: &Ui, rect: Rect, id: egui::Id, name: &str) -> bool {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    crate::help::note_topic(ui, rect, "agents-tools");
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter()
            .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon::ARROW_SQUARE_OUT,
        FontId::proportional(BODY),
        visuals.text_color(),
    );
    response
        .on_hover_text(format!("{name}: its changes, or the file itself"))
        .clicked()
}

/// Paints a line's caret, icon, words, note, and state.
fn paint_line(ui: &Ui, rect: Rect, line: &Line<'_>, palette: &Palette) {
    let painter = ui.painter();
    let middle = rect.center().y;
    let mut x = rect.left() + 4.0;
    if let Some(open) = line.open {
        let caret = if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        };
        painter.text(
            pos2(x + 6.0, middle),
            Align2::CENTER_CENTER,
            caret,
            FontId::proportional(SMALL),
            palette.muted,
        );
    }
    x += 16.0;
    let glyph_color = match line.state {
        State::Failed => palette.danger,
        State::Waiting => palette.warn,
        State::Running | State::Done => palette.muted,
    };
    painter.text(
        pos2(x + 8.0, middle),
        Align2::CENTER_CENTER,
        line.glyph,
        FontId::proportional(BODY),
        glyph_color,
    );
    x += 24.0;
    // At the right: how it stands, then the note before it.
    let mut right = rect.right() - 6.0;
    match line.state {
        State::Running => {
            let spinner = Rect::from_center_size(pos2(right - 7.0, middle), vec2(12.0, 12.0));
            egui::Spinner::new()
                .size(12.0)
                .color(palette.muted)
                .paint_at(ui, spinner);
            right -= 20.0;
        }
        State::Failed | State::Waiting => {
            let (words, color) = if line.state == State::Failed {
                ("Failed", palette.danger)
            } else {
                ("Waiting for you", palette.warn)
            };
            let galley = painter.layout_no_wrap(
                words.to_owned(),
                FontId::new(SMALL, Weight::Medium.family()),
                color,
            );
            right -= galley.size().x;
            painter.galley(pos2(right, middle - galley.size().y / 2.0), galley, color);
            right -= 8.0;
        }
        State::Done => {}
    }
    let verb = painter.layout_no_wrap(
        line.verb.to_owned(),
        FontId::new(BODY, Weight::Medium.family()),
        palette.text,
    );
    // The note only where the detail keeps room enough to read.
    if let Some((note, color)) = &line.note {
        let galley = painter.layout_no_wrap(note.clone(), FontId::proportional(SMALL), *color);
        let detail_room = right - galley.size().x - 8.0 - (x + verb.size().x + 8.0);
        if line.detail.0.is_empty() || detail_room >= 120.0 {
            right -= galley.size().x;
            painter.galley(pos2(right, middle - galley.size().y / 2.0), galley, *color);
            right -= 8.0;
        }
    }
    let verb_width = verb.size().x.min((right - x).max(0.0));
    painter
        .with_clip_rect(Rect::from_x_y_ranges(x..=right, rect.y_range()))
        .galley(pos2(x, middle - verb.size().y / 2.0), verb, palette.text);
    x += verb_width + 8.0;
    let (detail, kind) = line.detail;
    let room = right - x;
    if detail.is_empty() || room < 24.0 {
        return;
    }
    let font = match kind {
        Detail::Path | Detail::Code => FontId::monospace(CODE),
        Detail::Words => FontId::proportional(BODY),
    };
    let shown = match kind {
        Detail::Path => crate::tables::elided_start(ui, detail, &font, room).into_owned(),
        Detail::Code | Detail::Words => super::fitted(ui, detail, &font, room),
    };
    let galley = painter.layout_no_wrap(shown, font, palette.muted);
    painter.galley(
        pos2(x, middle - galley.size().y / 2.0),
        galley,
        palette.muted,
    );
}

/// A tool call: its line, and what it was given and gave back, opened (a plan's steps always).
pub(super) fn tool(ui: &mut Ui, call: &ToolCall, drawing: &mut Drawing<'_>) {
    let is_open = drawing.open.contains(&call.id);
    let summary = Summary::of(
        call,
        drawing.work_tree,
        drawing.highlights,
        &drawing.palette,
    );
    let state = state_of(call, drawing.waiting);
    let openable = has_details(call);
    let file = file_of(call, drawing.work_tree);
    let link = file.as_ref().map(|(_, shown)| format!("Show {shown}"));
    let (response, linked) = line(
        ui,
        &Line {
            open: openable.then_some(is_open),
            glyph: tool_icon(&call.name),
            verb: &summary.verb,
            detail: (&summary.detail, summary.kind),
            note: summary.note.clone(),
            state,
        },
        link.as_deref(),
    );
    if openable && response.clicked() {
        drawing.toggled = Some(call.id.clone());
    }
    response.on_hover_text(hover_of(call, &summary));
    if linked && let Some((path, _)) = file {
        drawing
            .actions
            .push(Action::ShowWorkFile(drawing.repo, path));
    }
    if call.name == "TodoWrite" {
        under(ui, |ui| todos(ui, &call.input, &drawing.palette));
        return;
    }
    if !is_open {
        if let Some(result) = call.result.as_ref().filter(|result| result.error) {
            under(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(super::first_line(&result.text))
                            .color(drawing.palette.danger)
                            .size(SMALL),
                    )
                    .truncate(),
                );
            });
        }
        return;
    }
    under(ui, |ui| details(ui, call, drawing));
}

/// What a line says of a tool call.
pub(super) struct Summary {
    pub(super) verb: String,
    pub(super) detail: String,
    pub(super) kind: Detail,
    pub(super) note: Option<(String, Color32)>,
}

impl Summary {
    /// What `call`'s line says: paths from the work tree; an edit's lines added and removed;
    /// a plan's progress; an MCP server's tool by its server.
    fn of(
        call: &ToolCall,
        work_tree: &Path,
        highlights: &mut Highlights,
        palette: &Palette,
    ) -> Self {
        let field = |name: &str| {
            call.input
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let (verb, detail) = call.summary();
        let (verb, mut detail) = (verb.to_owned(), detail);
        let mut kind = Detail::Words;
        let mut note = None;
        match call.name.as_str() {
            "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                detail = super::shown_path(&detail, work_tree).to_owned();
                kind = Detail::Path;
                note = edit_note(call, highlights, palette);
                if call.name == "Write" {
                    let lines = field("content").lines().count();
                    note = Some((lines_said(lines), palette.muted));
                }
            }
            "Bash" | "PowerShell" | "Grep" | "Glob" | "WebFetch" => kind = Detail::Code,
            "TodoWrite" => {
                let todos = todo_list(&call.input);
                let done = todos
                    .iter()
                    .filter(|(_, status, _)| *status == "completed")
                    .count();
                note = Some((format!("{done} of {} done", todos.len()), palette.muted));
            }
            "Agent" | "Task" => {
                let kind_of = field("subagent_type");
                if !kind_of.is_empty() {
                    note = Some((kind_of.to_owned(), palette.muted));
                }
            }
            "AskUserQuestion" => {
                call.input
                    .get("questions")
                    .and_then(Value::as_array)
                    .and_then(|questions| questions.first())
                    .and_then(|question| question.get("question"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .clone_into(&mut detail);
            }
            name => {
                // An MCP server's: `mcp__server__tool`.
                if let Some((server, tool)) = name
                    .strip_prefix("mcp__")
                    .and_then(|rest| rest.split_once("__"))
                {
                    return Self {
                        verb: tool.replace('_', " "),
                        detail: String::new(),
                        kind,
                        note: Some((server.to_owned(), palette.muted)),
                    };
                }
            }
        }
        Self {
            verb,
            detail,
            kind,
            note,
        }
    }
}

/// An edit's lines added and removed, "+3 −1" in their colours.
fn edit_note(
    call: &ToolCall,
    highlights: &mut Highlights,
    palette: &Palette,
) -> Option<(String, Color32)> {
    let field = |value: &Value, name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let edits: Vec<(String, String)> = match call.name.as_str() {
        "Edit" => vec![(
            field(&call.input, "old_string"),
            field(&call.input, "new_string"),
        )],
        "MultiEdit" => call
            .input
            .get("edits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|edit| (field(edit, "old_string"), field(edit, "new_string")))
            .collect(),
        _ => return None,
    };
    let (added, removed) = edits
        .iter()
        .fold((0_usize, 0_usize), |(added, removed), (old, new)| {
            let (more, fewer) = code::counts(highlights, old, new);
            (added.saturating_add(more), removed.saturating_add(fewer))
        });
    let color = if removed > added {
        palette.danger
    } else {
        palette.ok
    };
    Some((format!("+{added} −{removed}"), color))
}

/// "1 line", "12 lines".
fn lines_said(count: usize) -> String {
    if count == 1 {
        "1 line".to_owned()
    } else {
        format!(
            "{} lines",
            crate::format::count(u64::try_from(count).unwrap_or(u64::MAX))
        )
    }
}

/// How `call` stands: running, waiting for an answer (`waiting` holds its request), done, or
/// failed.
fn state_of(call: &ToolCall, waiting: &[PermissionRequest]) -> State {
    match &call.result {
        Some(result) if result.error => State::Failed,
        Some(_) => State::Done,
        None if waiting
            .iter()
            .any(|asked| asked.tool_use_id.as_deref() == Some(call.id.as_str())) =>
        {
            State::Waiting
        }
        None => State::Running,
    }
}

/// What the pointer resting on a line says: the tool, and the detail whole.
fn hover_of(call: &ToolCall, summary: &Summary) -> String {
    let field = |name: &str| {
        call.input
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let whole = match call.name.as_str() {
        "Bash" | "PowerShell" => field("command").to_owned(),
        _ => summary.detail.clone(),
    };
    if whole.is_empty() {
        call.name.clone()
    } else {
        format!("{}: {whole}", call.name)
    }
}

/// Whether a call has anything under its line to open.
fn has_details(call: &ToolCall) -> bool {
    !matches!(call.name.as_str(), "TodoWrite") && (call.result.is_some() || !call.input.is_null())
}

/// The file a call read or changed: its path, and as the user reads it.
fn file_of(call: &ToolCall, work_tree: &Path) -> Option<(PathBuf, String)> {
    let key = match call.name.as_str() {
        "Read" | "Write" | "Edit" | "MultiEdit" => "file_path",
        "NotebookEdit" => "notebook_path",
        _ => return None,
    };
    let path = call.input.get(key).and_then(Value::as_str)?;
    if path.is_empty() {
        return None;
    }
    let shown = super::shown_path(path, work_tree).to_owned();
    let path = Path::new(path);
    let absolute = if egit_git::is_absolute(path) {
        path.to_owned()
    } else {
        work_tree.join(path)
    };
    Some((absolute, shown))
}

/// What a call was given and gave back, as suits it.
fn details(ui: &mut Ui, call: &ToolCall, drawing: &mut Drawing<'_>) {
    let input = &call.input;
    let field = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or_default();
    match call.name.as_str() {
        "Bash" | "PowerShell" => {
            let description = field("description");
            if !description.is_empty() {
                ui.label(muted(ui, description));
            }
            let command = field("command");
            // One line shows on the line itself, unless cut there.
            if command.contains('\n') || command.chars().count() > 60 {
                code::code_box(
                    ui,
                    ("Command", "sh", Tone::Plain),
                    command,
                    drawing.highlights,
                );
            }
            result(ui, call, "Output", drawing);
        }
        "Edit" | "MultiEdit" | "Write" => file_details(ui, call, drawing),
        "Read" => result(ui, call, "Contents", drawing),
        "Grep" | "Glob" => result(ui, call, "Found", drawing),
        "WebFetch" | "WebSearch" => {
            let asked = field("prompt");
            if !asked.is_empty() {
                ui.label(muted(ui, asked));
            }
            prose_result(ui, call, drawing);
        }
        "Agent" | "Task" => {
            // Its items follow, then its report: only what it was asked here.
            ui.add(
                egui::Label::new(muted(ui, field("prompt")))
                    .wrap()
                    .selectable(true),
            );
        }
        "AskUserQuestion" => {
            for question in input
                .get("questions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let text = question
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(text).medium()).wrap());
            }
            prose_result(ui, call, drawing);
        }
        "ExitPlanMode" => {
            code::markdown(
                ui,
                field("plan"),
                (drawing.markdown, drawing.highlights),
                false,
            );
            if call.result.as_ref().is_some_and(|result| result.error) {
                result(ui, call, "Asked to keep planning", drawing);
            }
        }
        _ => {
            let shown = serde_json::to_string_pretty(input).unwrap_or_default();
            code::code_box(
                ui,
                ("Input", "json", Tone::Plain),
                &shown,
                drawing.highlights,
            );
            result(ui, call, "Result", drawing);
        }
    }
}

/// A file's edit as a diff (each of several), or a file written whole; what failed, if it did.
fn file_details(ui: &mut Ui, call: &ToolCall, drawing: &mut Drawing<'_>) {
    let input = &call.input;
    let field = |value: &'_ Value, name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let language = file_of(call, drawing.work_tree)
        .and_then(|(path, _)| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    match call.name.as_str() {
        "Write" => {
            let content = field(input, "content");
            let label = lines_said(content.lines().count());
            code::code_box(
                ui,
                (&label, &language, Tone::Plain),
                &content,
                drawing.highlights,
            );
        }
        "MultiEdit" => {
            for edit in input
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (old, new) = (field(edit, "old_string"), field(edit, "new_string"));
                code::diff_box(ui, (&old, &new), &language, drawing.highlights);
            }
        }
        _ => {
            let (old, new) = (field(input, "old_string"), field(input, "new_string"));
            code::diff_box(ui, (&old, &new), &language, drawing.highlights);
        }
    }
    if call.result.as_ref().is_some_and(|result| result.error) {
        result(ui, call, "Result", drawing);
    }
}

/// What a call gave back, in a box under `label` (as failed, when it did).
fn result(ui: &mut Ui, call: &ToolCall, label: &str, drawing: &mut Drawing<'_>) {
    let Some(result) = &call.result else {
        return;
    };
    if result.text.trim().is_empty() && !result.error {
        ui.label(muted(ui, "Nothing came back").size(SMALL));
        return;
    }
    let (label, tone) = if result.error {
        ("Failed", Tone::Failed)
    } else {
        (label, Tone::Plain)
    };
    code::code_box(ui, (label, "", tone), &result.text, drawing.highlights);
}

/// What a call gave back as prose (a page fetched, an answer): Markdown, or what failed.
fn prose_result(ui: &mut Ui, call: &ToolCall, drawing: &mut Drawing<'_>) {
    match &call.result {
        Some(result) if !result.error => {
            code::markdown(
                ui,
                &result.text,
                (drawing.markdown, drawing.highlights),
                false,
            );
        }
        Some(_) => result(ui, call, "Result", drawing),
        None => {}
    }
}

/// A plan's steps: each ticked done, under way (as it is being done), or to do.
fn todos(ui: &mut Ui, input: &Value, palette: &Palette) {
    for (content, status, active) in todo_list(input) {
        let (glyph, color, text) = match status {
            "completed" => (
                icon::CHECK_CIRCLE,
                palette.ok,
                RichText::new(content).color(palette.muted).strikethrough(),
            ),
            "in_progress" => (
                icon::ARROW_CIRCLE_RIGHT,
                palette.accent,
                RichText::new(if active.is_empty() { content } else { active }).medium(),
            ),
            _ => (icon::CIRCLE, palette.muted, RichText::new(content)),
        };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(glyph).color(color));
            ui.add(egui::Label::new(text).wrap());
        });
    }
}

/// A plan's steps: what each is, how it stands, and its words while under way.
fn todo_list(input: &Value) -> Vec<(&str, &str, &str)> {
    input
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|todo| {
            let field = |name: &str| todo.get(name).and_then(Value::as_str).unwrap_or_default();
            (field("content"), field("status"), field("activeForm"))
        })
        .collect()
}

/// Draws `add` stepped in under a line, to its words.
pub(super) fn under(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.add_space(STEP);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
    });
}

/// A tool's icon.
pub(super) fn tool_icon(name: &str) -> &'static str {
    match name {
        "Read" => icon::FILE_TEXT,
        "Edit" | "MultiEdit" | "NotebookEdit" => icon::PENCIL_SIMPLE,
        "Write" => icon::FILE_PLUS,
        "Bash" | "PowerShell" => icon::TERMINAL,
        "Grep" | "Glob" => icon::MAGNIFYING_GLASS,
        "WebFetch" | "WebSearch" => icon::GLOBE,
        "TodoWrite" => icon::LIST_CHECKS,
        "Agent" | "Task" => icon::ROBOT,
        "AskUserQuestion" => icon::QUESTION,
        "ExitPlanMode" => icon::MAP_TRIFOLD,
        name if name.starts_with("mcp__") => icon::PLUG,
        _ => icon::WRENCH,
    }
}
