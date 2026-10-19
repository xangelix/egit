//! The Agents panel at the right of a repository's tab: Claude Code working in its work tree.
//! A head with the conversations (new, open, earlier), Claude's setup until it is done, then
//! the conversation shown (only the items in view laid out), what waits for an answer (a
//! tool's permission, Claude's question, its plan), and the prompt with the mode, model, and
//! Send or Stop. The top bar's Agents shows and hides it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use egit_agents::protocol::{Answer, Mode, PermissionRequest};
use egit_agents::{Item, ItemKind, Level, ToolCall, TurnEnd};
use egui::{Color32, Frame, Id, Key, Margin, Modifiers, RichText, Ui, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde_json::Value;

use crate::agents::{AgentsSetup, Conversation, RepoAgents};
use crate::app::Action;
use crate::stack::{Entry, Stack};
use crate::theme::{CODE, Palette, RADIUS_BOX, Weighted as _, icon};
use crate::widgets::{self, Head, Look, muted};

/// The panel's width at first, and its least and most.
pub(crate) const WIDTH: f32 = 460.0;
pub(crate) const WIDTHS: std::ops::RangeInclusive<f32> = 320.0..=1400.0;
/// The most lines of a tool's output shown.
const OUTPUT_LINES: usize = 200;
/// Room between items.
const GAP: f32 = 10.0;
/// The prompt field's tallest.
const PROMPT_HEIGHT: f32 = 220.0;

/// What a conversation's view keeps: the prompt being written, its items' heights, what is
/// folded open, Markdown's cache, and answers being made.
#[derive(Default)]
pub(crate) struct ConversationView {
    /// The prompt being written.
    pub(crate) draft: String,
    stack: Stack,
    /// Tool calls (by id) and thinking (by item) folded open.
    open: HashSet<String>,
    cache: CommonMarkCache,
    /// Why a refusal refuses, as typed.
    refusal: String,
    /// A question's choices, by the question.
    chosen: HashMap<String, Vec<String>>,
    /// A question's own answer typed, by the question.
    others: HashMap<String, String>,
}

impl ConversationView {
    /// Follows the conversation to its end.
    pub(crate) const fn follow(&mut self) {
        self.stack.scroll_to_end();
    }
}

/// The panel.
pub(crate) fn panel(
    ui: &mut Ui,
    (setup, agents): (&mut AgentsSetup, &mut RepoAgents),
    work_tree: &Path,
    actions: &mut Vec<Action>,
) {
    setup.look();
    head(ui, setup, agents, work_tree);
    ui.separator();
    if !setup.is_ready() {
        set_up(ui, setup, actions);
        return;
    }
    let launch = setup.launch(&egit_agents::accounts::Signer::Default);
    let sent = egui::Panel::bottom(Id::new(("agents-prompt", work_tree)))
        .frame(Frame::new().inner_margin(Margin::symmetric(0, 8)))
        .show(ui, |ui| {
            let conversation = agents.current()?;
            waiting(ui, conversation);
            prompt(ui, setup, conversation).then(|| std::mem::take(&mut conversation.view.draft))
        })
        .inner;
    if let Some(text) = sent {
        agents.send(launch, work_tree, text.trim());
    }
    let Some(conversation) = agents.current() else {
        return;
    };
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| transcript(ui, conversation, work_tree));
}

/// The head: the title, the conversations' menu, and hiding the panel.
fn head(ui: &mut Ui, setup: &mut AgentsSetup, agents: &mut RepoAgents, work_tree: &Path) {
    widgets::panel_head(ui, Head::Control, |ui| {
        widgets::panel_title(ui, "Agents");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::icon_tool(ui, true, icon::SIDEBAR_SIMPLE, "Hide the agents")
                .on_hover_text("Hide the agents")
                .clicked()
            {
                agents.shown = false;
            }
            if !setup.is_ready() {
                return;
            }
            let title = agents
                .current()
                .map(|conversation| conversation.title.clone())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "New conversation".to_owned());
            let launch = setup.launch(&egit_agents::accounts::Signer::Default);
            let title = elided(&title, 32);
            let (response, _) = widgets::tool_menu_of(
                ui,
                (icon::CHATS, &title),
                Look::Full,
                "agents-conversations",
                |ui| conversations_menu(ui, agents, launch.as_ref(), work_tree),
            );
            if response.clicked() {
                agents.list_past(launch, work_tree);
            }
        });
    });
}

/// The conversations' menu: a new one, those open, the earlier ones kept for the work tree.
fn conversations_menu(
    ui: &mut Ui,
    agents: &mut RepoAgents,
    launch: Option<&egit_agents::Launch>,
    work_tree: &Path,
) {
    super::actions::menu_rows(ui);
    if super::actions::item(ui, (icon::PLUS, "New conversation"), Ok(())) {
        agents.begin();
        ui.close();
    }
    super::actions::menu_header(ui, "Open");
    let mut shown = None;
    let mut closed = None;
    for (index, conversation) in agents.conversations.iter().enumerate() {
        let title = if conversation.title.is_empty() {
            "New conversation".to_owned()
        } else {
            elided(&conversation.title, 48)
        };
        let glyph = if index == agents.current {
            icon::CHECK
        } else {
            icon::CHAT_CIRCLE_DOTS
        };
        ui.horizontal(|ui| {
            if super::actions::item_of(ui, (glyph, &title), Ok(()), "agents-conversations") {
                shown = Some(index);
            }
            if widgets::icon_tool(ui, true, icon::X, "Close the conversation")
                .on_hover_text("Close the conversation (it is kept: open it again from Earlier)")
                .clicked()
            {
                closed = Some(index);
            }
        });
    }
    if let Some(index) = shown {
        agents.current = index;
        ui.close();
    }
    if let Some(index) = closed {
        agents.close(index);
    }
    super::actions::menu_header(ui, "Earlier");
    let past = match agents.past.read_as_ref() {
        Some(Ok(past)) => std::sync::Arc::clone(past),
        Some(Err(problem)) => {
            ui.label(muted(ui, problem.detail.clone()));
            return;
        }
        None => {
            widgets::working(ui, "Reading…");
            return;
        }
    };
    if past.is_empty() {
        ui.label(muted(ui, "None kept for this folder"));
    }
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            let now = crate::format::now();
            for session in past.iter() {
                let seconds = session.last_modified.checked_div(1000).unwrap_or(0);
                let modified = u64::try_from(seconds).unwrap_or(0);
                let title = format!(
                    "{} · {}",
                    elided(&session.summary, 48),
                    crate::format::ago(modified, now)
                );
                let glyph = icon::CLOCK_COUNTER_CLOCKWISE;
                if super::actions::item_of(ui, (glyph, &title), Ok(()), "agents-conversations") {
                    agents.resume(launch.cloned(), work_tree, session);
                    ui.close();
                }
            }
        });
}

/// Claude's setup, until it is done: Node, then the SDK.
fn set_up(ui: &mut Ui, setup: &mut AgentsSetup, actions: &mut Vec<Action>) {
    ui.add_space(8.0);
    let about = "Agents run Claude Code here, as it runs in a terminal (its tools, settings, \
                 CLAUDE.md files, hooks, and sessions), through Anthropic's Claude Agent SDK.";
    ui.add(egui::Label::new(about).wrap());
    ui.add_space(8.0);
    match setup.found.read_as_ref() {
        None => widgets::working(ui, "Looking for Node.js…"),
        Some(Err(problem)) => {
            let said = format!(
                "The SDK needs Node.js {} or newer, and npm: {}.",
                egit_agents::install::NODE_MINIMUM,
                problem.detail
            );
            ui.add(egui::Label::new(muted(ui, said)).wrap());
            ui.horizontal(|ui| {
                if widgets::secondary(ui, true, "Look again").clicked() {
                    setup.look_again();
                }
                if widgets::link(ui, "nodejs.org").clicked() {
                    actions.push(Action::OpenUrl("https://nodejs.org/".to_owned()));
                }
            });
        }
        Some(Ok(found)) => {
            let found = format!("Node.js {} found.", found.node.version);
            ui.label(muted(ui, found));
            install(ui, setup);
        }
    }
}

/// Installing the SDK: what it is, the button, its progress or failure.
fn install(ui: &mut Ui, setup: &mut AgentsSetup) {
    let what = format!(
        "Setting Claude up installs the Claude Agent SDK {} and Claude Code for this computer \
         (about {} MB, from npm) into egit's cache.",
        egit_agents::install::SDK_VERSION,
        egit_agents::install::INSTALLED_MEGABYTES
    );
    ui.add(egui::Label::new(what).wrap());
    ui.add_space(8.0);
    if setup.installing.is_pending() {
        let elapsed = setup.installing.since_started();
        ui.horizontal(|ui| {
            widgets::working(ui, &format!("Setting Claude up… {elapsed:.0} s"));
            if widgets::secondary(ui, true, "Cancel").clicked() {
                setup.cancel();
            }
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
        return;
    }
    if let Some(problem) = setup.install_problem.clone() {
        widgets::problem_box(ui, &problem, |_| {});
        ui.add_space(8.0);
    }
    if widgets::primary(ui, true, "Set up Claude").clicked() {
        setup.install();
    }
}

/// The conversation's items, those in view laid out; who it runs as above them.
fn transcript(ui: &mut Ui, conversation: &mut Conversation, work_tree: &Path) {
    let palette = Palette::of(ui.ctx());
    if conversation.history.is_pending() {
        widgets::working(ui, "Reading the conversation…");
        return;
    }
    let items = conversation.transcript.items();
    if items.is_empty() {
        ui.add_space(16.0);
        let name = work_tree
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        ui.label(RichText::new(format!("Ask Claude to work in {name}")).semibold());
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(muted(
                ui,
                "It reads this repository's CLAUDE.md and your settings, and asks before \
                 changing anything unless its mode says otherwise. Earlier conversations are in \
                 the menu above.",
            ))
            .wrap(),
        );
        return;
    }
    // Subagents' items show inside their call, while it is open.
    let shown: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.parent
                .as_ref()
                .is_none_or(|parent| conversation.view.open.contains(parent))
        })
        .map(|(index, _)| index)
        .collect();
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let entries: Vec<Entry> = shown
        .iter()
        .filter_map(|index| items.get(*index).map(|item| (index, item)))
        .map(|(index, item)| {
            let open = conversation.view.open.contains(&key_of(*index, item));
            Entry {
                version: item.changed.wrapping_mul(2).wrapping_add(u64::from(open)),
                guess: guess(item, line),
            }
        })
        .collect();
    let busy = conversation.transcript.busy();
    let view = &mut conversation.view;
    let ConversationView {
        stack, open, cache, ..
    } = view;
    stack.show(
        ui,
        Id::new(("agents-transcript", work_tree)),
        &entries,
        GAP,
        |ui, at| {
            let Some(index) = shown.get(at).copied() else {
                return;
            };
            let Some(item) = items.get(index) else {
                return;
            };
            let last = index.saturating_add(1) == items.len();
            item_view(ui, (index, item), (open, cache), (palette, busy && last));
        },
    );
}

/// A guess at an item's height before it is drawn.
fn guess(item: &Item, line: f32) -> f32 {
    let lines = |text: &str| {
        let wraps = text.len().checked_div(60).unwrap_or(0);
        let count = text.lines().count().saturating_add(wraps);
        f32::from(u16::try_from(count.max(1)).unwrap_or(u16::MAX))
    };
    match &item.kind {
        ItemKind::Prompt(text) => lines(text).mul_add(line, 20.0),
        ItemKind::Text(text) => lines(text) * line * 1.3,
        ItemKind::Thinking(_) | ItemKind::Tool(_) => 28.0,
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

/// One item.
fn item_view(
    ui: &mut Ui,
    (index, item): (usize, &Item),
    (open, cache): (&mut HashSet<String>, &mut CommonMarkCache),
    (palette, streaming): (Palette, bool),
) {
    if item.parent.is_some() {
        // A subagent's: stepped in under its call.
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| item_body(ui, (index, item), (open, cache), (palette, streaming)));
        });
    } else {
        item_body(ui, (index, item), (open, cache), (palette, streaming));
    }
}

fn item_body(
    ui: &mut Ui,
    (index, item): (usize, &Item),
    (open, cache): (&mut HashSet<String>, &mut CommonMarkCache),
    (palette, streaming): (Palette, bool),
) {
    match &item.kind {
        ItemKind::Prompt(text) => {
            Frame::new()
                .fill(palette.surface)
                .corner_radius(RADIUS_BOX)
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(text.as_str()).wrap().selectable(true));
                });
        }
        ItemKind::Text(text) => {
            CommonMarkViewer::new().show(ui, cache, text);
            if streaming {
                ui.ctx().request_repaint();
            }
        }
        ItemKind::Thinking(text) => {
            let key = key_of(index, item);
            let is_open = open.contains(&key);
            if fold(ui, is_open, icon::BRAIN, "Thinking", "").clicked() {
                toggle(open, key);
            }
            if is_open {
                ui.add(
                    egui::Label::new(muted(ui, text.as_str()))
                        .wrap()
                        .selectable(true),
                );
            }
        }
        ItemKind::Tool(call) => tool(ui, call, open, palette),
        ItemKind::Turn(end) => turn(ui, end, palette),
        ItemKind::Notice(notice) => {
            let (glyph, color) = match notice.level {
                Level::Info => (icon::INFO, palette.muted),
                Level::Warning => (icon::WARNING, palette.warn),
                Level::Error => (icon::WARNING_CIRCLE, palette.danger),
            };
            ui.add(
                egui::Label::new(RichText::new(format!("{glyph}  {}", notice.text)).color(color))
                    .wrap()
                    .selectable(true),
            );
        }
    }
}

/// Opens or folds `key`.
fn toggle(open: &mut HashSet<String>, key: String) {
    if !open.remove(&key) {
        open.insert(key);
    }
}

/// A line that opens and folds what is under it: its caret, icon, words, and what they are about
/// (a path, a command), the last losing its start first.
fn fold(ui: &mut Ui, is_open: bool, glyph: &str, verb: &str, detail: &str) -> egui::Response {
    let caret = if is_open {
        icon::CARET_DOWN
    } else {
        icon::CARET_RIGHT
    };
    let mut words = format!("{caret}  {glyph}  {verb}");
    if !detail.is_empty() {
        words.push_str("  ");
        words.push_str(detail);
    }
    let response = widgets::quiet(ui, |ui| {
        ui.add(
            egui::Button::new(RichText::new(words).medium())
                .frame_when_inactive(false)
                .truncate()
                .min_size(vec2(ui.available_width(), widgets::TARGET)),
        )
    });
    crate::help::note_topic(ui, response.rect, "agents-tools");
    response
}

/// A tool call: its line, and opened, what it was given and gave back.
fn tool(ui: &mut Ui, call: &ToolCall, open: &mut HashSet<String>, palette: Palette) {
    let (verb, detail) = call.summary();
    let glyph = tool_icon(&call.name);
    let is_open = open.contains(&call.id);
    let response = fold(ui, is_open, glyph, verb, &detail);
    let state = match &call.result {
        None => "running",
        Some(result) if result.error => "failed",
        Some(_) => "done",
    };
    let response = response.on_hover_text(format!("{} ({state})", call.name));
    if let Some(result) = call.result.as_ref().filter(|result| result.error) {
        ui.add(
            egui::Label::new(RichText::new(first_line(&result.text)).color(palette.danger))
                .truncate(),
        );
    }
    if response.clicked() {
        toggle(open, call.id.clone());
    }
    if !is_open {
        return;
    }
    tool_details(ui, call, palette);
}

/// What a tool was given, as fits it (a command, an edit as a diff, a plan's steps), and what it
/// gave back.
fn tool_details(ui: &mut Ui, call: &ToolCall, palette: Palette) {
    let input = &call.input;
    let field = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or_default();
    match call.name.as_str() {
        "Bash" | "PowerShell" => code(ui, field("command"), None),
        "Edit" => edit_diff(ui, field("old_string"), field("new_string"), palette),
        "MultiEdit" => {
            for edit in input
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let part = |name: &str| edit.get(name).and_then(Value::as_str).unwrap_or_default();
                edit_diff(ui, part("old_string"), part("new_string"), palette);
            }
        }
        "Write" => code(ui, field("content"), Some(palette.added)),
        "TodoWrite" => todos(ui, input, palette),
        "Read" | "Grep" | "Glob" | "WebFetch" | "WebSearch" | "Agent" | "Task" => {}
        _ => code(
            ui,
            &serde_json::to_string_pretty(input).unwrap_or_default(),
            None,
        ),
    }
    if let Some(result) = &call.result {
        let color = result.error.then_some(palette.removed);
        code(ui, &result.text, color);
    }
}

/// A plan's steps, ticked as done.
fn todos(ui: &mut Ui, input: &Value, palette: Palette) {
    for todo in input
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = todo
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (glyph, color) = match todo.get("status").and_then(Value::as_str) {
            Some("completed") => (icon::CHECK_SQUARE, palette.ok),
            Some("in_progress") => (icon::CIRCLE_NOTCH, palette.accent),
            _ => (icon::SQUARE, palette.muted),
        };
        ui.add(egui::Label::new(RichText::new(format!("{glyph}  {text}")).color(color)).wrap());
    }
}

/// An edit: the lines it replaces over those it puts there.
fn edit_diff(ui: &mut Ui, old: &str, new: &str, palette: Palette) {
    if !old.is_empty() {
        code(ui, old, Some(palette.removed));
    }
    code(ui, new, Some(palette.added));
}

/// Text in a box of its own, monospace, at most [`OUTPUT_LINES`] lines (how many more said),
/// on `fill` when given.
fn code(ui: &mut Ui, text: &str, fill: Option<Color32>) {
    let palette = Palette::of(ui.ctx());
    let total = text.lines().count();
    let shown: String = text
        .lines()
        .take(OUTPUT_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    Frame::new()
        .fill(fill.unwrap_or(palette.surface_alt))
        .corner_radius(RADIUS_BOX)
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(RichText::new(shown).monospace().size(CODE))
                    .wrap()
                    .selectable(true),
            );
            if total > OUTPUT_LINES {
                let more = total.saturating_sub(OUTPUT_LINES);
                ui.label(muted(ui, format!("… {more} more lines")));
            }
        });
}

/// A turn's end: how long it took and what it cost, or why it failed.
fn turn(ui: &mut Ui, end: &TurnEnd, palette: Palette) {
    if let Some(error) = &end.error {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{}  {error}", icon::WARNING_CIRCLE)).color(palette.danger),
            )
            .wrap(),
        );
        return;
    }
    let seconds = f64::from(u32::try_from(end.duration_ms).unwrap_or(u32::MAX)) / 1000.0;
    let tokens = end.input_tokens.saturating_add(end.output_tokens);
    ui.label(
        muted(
            ui,
            format!(
                "{}  Done in {seconds:.1} s · {} tokens · ≈ ${:.2}",
                icon::CHECK,
                crate::format::count(tokens),
                end.cost_usd
            ),
        )
        .size(crate::theme::SMALL),
    )
    .on_hover_text(
        "The cost as Claude Code estimates it: a subscription's plan isn't billed by it",
    );
}

/// What waits for an answer: the oldest permission request (how many more said).
fn waiting(ui: &mut Ui, conversation: &mut Conversation) {
    let Some(asked) = conversation.waiting.first().cloned() else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let more = conversation.waiting.len().saturating_sub(1);
    let answer = Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.accent))
        .corner_radius(RADIUS_BOX)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let answer = match asked.tool.as_str() {
                "AskUserQuestion" => question(ui, &asked, &mut conversation.view),
                "ExitPlanMode" => plan(ui, &asked, &mut conversation.view),
                _ => permission(ui, &asked, &mut conversation.view),
            };
            if more > 0 {
                ui.label(muted(ui, format!("{more} more waiting")));
            }
            answer
        })
        .inner;
    ui.add_space(8.0);
    if let Some(answer) = answer {
        conversation.view.refusal.clear();
        conversation.answer(answer);
    }
}

/// A tool's permission: what it would do, and Allow, Allow for this conversation, or Refuse.
fn permission(
    ui: &mut Ui,
    asked: &PermissionRequest,
    view: &mut ConversationView,
) -> Option<Answer> {
    let palette = Palette::of(ui.ctx());
    let call = ToolCall {
        id: String::new(),
        name: asked.tool.clone(),
        input: asked.input.clone(),
        result: None,
    };
    let (verb, detail) = call.summary();
    ui.label(RichText::new(format!("Claude wants to {}", verb.to_lowercase())).semibold());
    // A command shows whole below; anything else, what it is about (a path, a search).
    let shown_below = matches!(asked.tool.as_str(), "Bash" | "PowerShell");
    if !detail.is_empty() && !shown_below {
        ui.add(egui::Label::new(RichText::new(detail).monospace()).wrap());
    }
    if let Some(reason) = &asked.reason {
        ui.add(egui::Label::new(muted(ui, reason.as_str())).wrap());
    }
    if let Some(path) = &asked.blocked_path {
        ui.add(egui::Label::new(muted(ui, format!("Outside the folders allowed: {path}"))).wrap());
    }
    egui::ScrollArea::vertical()
        .id_salt(("permission", &asked.request))
        .max_height(200.0)
        .show(ui, |ui| tool_details(ui, &call, palette));
    ui.add_space(6.0);
    let label = ui.label(muted(ui, "Why not (Claude reads it), when refusing"));
    ui.add(widgets::line_field(&mut view.refusal))
        .labelled_by(label.id);
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, true, "Allow once").clicked() {
            answer = Some(allow(asked, false, None));
        }
        if !asked.suggestions.is_empty()
            && widgets::secondary(ui, true, "Allow for this conversation")
                .on_hover_text("Claude Code's rules for it, for the rest of this conversation")
                .clicked()
        {
            answer = Some(allow(asked, true, None));
        }
        if widgets::danger(ui, true, "Refuse").clicked() {
            answer = Some(refuse(asked, &view.refusal));
        }
    });
    answer
}

/// Claude's questions: each one's choices (or several), or an answer of one's own; Answer or
/// Refuse.
fn question(ui: &mut Ui, asked: &PermissionRequest, view: &mut ConversationView) -> Option<Answer> {
    let questions: Vec<&Value> = asked
        .input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    ui.label(RichText::new("Claude asks").semibold());
    let mut answers = serde_json::Map::new();
    for question in &questions {
        let text = question
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let several = question
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text.as_str()).medium()).wrap());
        let chosen = view.chosen.entry(text.clone()).or_default();
        for option in question
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let label = option
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let description = option
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut picked = chosen.iter().any(|chosen| chosen == label);
            let response = if several {
                widgets::checkbox_of(ui, &mut picked, label, "agents-questions")
            } else {
                let response = widgets::radio_of(ui, picked, label, "agents-questions");
                if response.clicked() {
                    picked = true;
                }
                response
            };
            if !description.is_empty() {
                response.on_hover_text(description);
            }
            if picked && !chosen.iter().any(|chosen| chosen == label) {
                if !several {
                    chosen.clear();
                }
                chosen.push(label.to_owned());
            } else if !picked {
                chosen.retain(|chosen| chosen != label);
            }
        }
        let other = view.others.entry(text.clone()).or_default();
        let label = ui.label(muted(ui, "Or in your own words"));
        ui.add(widgets::line_field(other)).labelled_by(label.id);
        let answer = if other.trim().is_empty() {
            chosen.join(", ")
        } else {
            other.trim().to_owned()
        };
        answers.insert(text, Value::String(answer));
    }
    let complete = answers
        .values()
        .all(|answer| answer.as_str().is_some_and(|answer| !answer.is_empty()));
    let mut answered = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, complete, "Answer").clicked() {
            let mut input = asked.input.clone();
            if let Some(object) = input.as_object_mut() {
                object.insert("answers".to_owned(), Value::Object(answers.clone()));
            }
            answered = Some(allow(asked, false, Some(input)));
        }
        if widgets::secondary(ui, true, "Don't answer").clicked() {
            answered = Some(refuse(asked, "The user chose not to answer."));
        }
    });
    answered
}

/// Claude's plan: Approve it, or keep planning (saying what to change).
fn plan(ui: &mut Ui, asked: &PermissionRequest, view: &mut ConversationView) -> Option<Answer> {
    ui.label(RichText::new("Claude's plan").semibold());
    let text = asked
        .input
        .get("plan")
        .and_then(Value::as_str)
        .unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt(("plan", &asked.request))
        .max_height(320.0)
        .show(ui, |ui| {
            CommonMarkViewer::new().show(ui, &mut view.cache, text)
        });
    let label = ui.label(muted(ui, "What to change, to keep planning"));
    ui.add(widgets::line_field(&mut view.refusal))
        .labelled_by(label.id);
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, true, "Approve the plan").clicked() {
            answer = Some(allow(asked, false, None));
        }
        if widgets::secondary(ui, true, "Keep planning").clicked() {
            let why = if view.refusal.trim().is_empty() {
                "Keep planning: the user wants changes."
            } else {
                view.refusal.as_str()
            };
            answer = Some(refuse(asked, why));
        }
    });
    answer
}

fn allow(asked: &PermissionRequest, always: bool, input: Option<Value>) -> Answer {
    Answer {
        request: asked.request.clone(),
        allow: true,
        always,
        input,
        message: None,
        interrupt: false,
    }
}

fn refuse(asked: &PermissionRequest, why: &str) -> Answer {
    let message = if why.trim().is_empty() {
        "The user refused this.".to_owned()
    } else {
        why.trim().to_owned()
    };
    Answer {
        request: asked.request.clone(),
        allow: false,
        always: false,
        input: None,
        message: Some(message),
        interrupt: false,
    }
}

/// The prompt, and under it the mode, model, what it cost, and Send or Stop. Returns whether
/// the prompt was sent (Enter, or Send).
fn prompt(ui: &mut Ui, setup: &mut AgentsSetup, conversation: &mut Conversation) -> bool {
    let id = Id::new(("agent-prompt", std::ptr::from_ref(conversation).addr()));
    let focused = ui.memory(|memory| memory.has_focus(id));
    // Enter sends (Shift+Enter starts a new line), before the field takes it.
    let entered = focused
        && ui.input_mut(|input| {
            input.consume_key(Modifiers::NONE, Key::Enter)
                || input.consume_key(Modifiers::COMMAND, Key::Enter)
        });
    let label = ui.label(muted(ui, "Ask Claude"));
    let field = widgets::bounded_text_area(
        ui,
        widgets::text_area(&mut conversation.view.draft)
            .id(id)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("What should it do? (Enter sends; Shift+Enter, a new line)"),
        PROMPT_HEIGHT,
        "agent-prompt",
    )
    .labelled_by(label.id);
    let has_text = !conversation.view.draft.trim().is_empty();
    let mut sent = entered && has_text;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        mode_menu(ui, conversation);
        model_menu(ui, conversation);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // While Claude works, Stop; a prompt sent meanwhile (Enter) waits its turn.
            if conversation.is_busy() {
                if widgets::secondary(ui, true, &format!("{}  Stop", icon::STOP)).clicked() {
                    conversation.interrupt();
                }
                ui.spinner();
            } else if widgets::primary(ui, has_text, &format!("{}  Send", icon::PAPER_PLANE_RIGHT))
                .clicked()
            {
                sent = true;
            }
        });
    });
    who(ui, setup, conversation);
    if sent {
        field.request_focus();
    }
    sent && has_text
}

/// The permission mode's menu.
fn mode_menu(ui: &mut Ui, conversation: &mut Conversation) {
    let mode = conversation.mode();
    let mut chosen = None;
    widgets::tool_menu(ui, icon::SHIELD_CHECK, mode.name(), Look::Full, |ui| {
        super::actions::menu_rows(ui);
        for offered in Mode::OFFERED {
            let glyph = if offered == mode { icon::CHECK } else { "" };
            let picked = if offered == Mode::BypassPermissions {
                super::actions::danger_item(ui, (glyph, offered.name()), Ok(()))
            } else {
                super::actions::item(ui, (glyph, offered.name()), Ok(()))
            };
            if picked {
                chosen = Some(offered);
                ui.close();
            }
        }
    });
    if let Some(mode) = chosen {
        conversation.set_mode(mode);
    }
}

/// The model's menu, once Claude Code has said which there are.
fn model_menu(ui: &mut Ui, conversation: &mut Conversation) {
    let current = conversation
        .model
        .clone()
        .or_else(|| conversation.transcript.model().map(str::to_owned));
    let models = conversation
        .info
        .as_ref()
        .map(|info| info.models.clone())
        .unwrap_or_default();
    let name = current
        .as_deref()
        .and_then(|current| {
            models
                .iter()
                .find(|model| model.value == current)
                .map(|model| model.display_name.clone())
        })
        .or_else(|| current.clone())
        .unwrap_or_else(|| "Default model".to_owned());
    if models.is_empty() {
        let label = ui.label(muted(ui, name));
        crate::help::note_topic(ui, label.rect, "agents-models");
        return;
    }
    let mut chosen = None;
    widgets::tool_menu_of(
        ui,
        (icon::SPARKLE, &name),
        Look::Full,
        "agents-models",
        |ui| {
            super::actions::menu_rows(ui);
            for model in &models {
                let glyph = if current.as_deref() == Some(model.value.as_str()) {
                    icon::CHECK
                } else {
                    ""
                };
                if super::actions::item_of(
                    ui,
                    (glyph, &model.display_name),
                    Ok(()),
                    "agents-models",
                ) {
                    chosen = Some(model.value.clone());
                    ui.close();
                }
            }
        },
    );
    if let Some(model) = chosen {
        conversation.set_model(Some(model));
    }
}

/// Who the conversation runs as (Claude Code's account, or how to sign in when nobody is), and
/// what its turns cost.
fn who(ui: &mut Ui, setup: &mut AgentsSetup, conversation: &Conversation) {
    let cost = conversation.transcript.cost_usd();
    let cost = if cost > 0.0 {
        format!(" · ≈ ${cost:.2}")
    } else {
        String::new()
    };
    if let Some(email) = conversation
        .info
        .as_ref()
        .and_then(|info| info.account.email.clone())
    {
        let plan = conversation
            .info
            .as_ref()
            .and_then(|info| info.account.subscription_type.clone())
            .map_or_else(String::new, |plan| format!(" ({plan})"));
        ui.label(
            muted(ui, format!("Claude Code as {email}{plan}{cost}")).size(crate::theme::SMALL),
        )
        .on_hover_text("The cost as Claude Code estimates it: a plan's usage isn't billed by it");
        return;
    }
    let claude = setup
        .install
        .as_ref()
        .and_then(egit_agents::install::Install::claude);
    match setup.signed_in.read_as_ref() {
        Some(Ok(status)) if status.logged_in => {
            let who = status
                .email
                .clone()
                .unwrap_or_else(|| status.auth_method.clone());
            ui.label(muted(ui, format!("Claude Code as {who}")).size(crate::theme::SMALL));
        }
        Some(Ok(_)) => {
            let palette = Palette::of(ui.ctx());
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{}  Nobody is signed in to Claude Code here. Sign in with Claude Code \
                         itself, in a terminal:",
                        icon::WARNING
                    ))
                    .color(palette.warn),
                )
                .wrap(),
            );
            if let Some(claude) = claude {
                let command = egit_agents::accounts::login_command(&claude, None);
                code(ui, &command, None);
                widgets::buttons_row(ui, |ui| {
                    if widgets::secondary(ui, true, "Copy the command").clicked() {
                        ui.ctx().copy_text(command.clone());
                    }
                    if widgets::secondary(ui, true, "Look again").clicked() {
                        setup.ask_who();
                    }
                });
            }
        }
        Some(Err(problem)) => {
            ui.label(muted(ui, problem.title.clone()).size(crate::theme::SMALL));
        }
        None => {}
    }
}

/// A tool's icon.
fn tool_icon(name: &str) -> &'static str {
    match name {
        "Read" => icon::FILE_TEXT,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => icon::PENCIL_SIMPLE,
        "Bash" | "PowerShell" => icon::TERMINAL,
        "Grep" | "Glob" => icon::MAGNIFYING_GLASS,
        "WebFetch" | "WebSearch" => icon::GLOBE,
        "TodoWrite" => icon::LIST_CHECKS,
        "Agent" | "Task" => icon::ROBOT,
        "AskUserQuestion" => icon::QUESTION,
        "ExitPlanMode" => icon::MAP_TRIFOLD,
        _ => icon::WRENCH,
    }
}

/// `text`'s first line.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// `text` at most `most` characters, an ellipsis ending it when cut.
fn elided(text: &str, most: usize) -> String {
    let first = first_line(text);
    if first.chars().count() <= most {
        return first.to_owned();
    }
    let mut cut: String = first.chars().take(most.saturating_sub(1)).collect();
    cut.push('…');
    cut
}
