//! What waits for an answer, over the prompt: the oldest request (how many more said). A tool's
//! permission says what it would do in the tool's own terms (run this command, edit this
//! file), shows it (the command, the edit as a diff), and offers Allow once, Allow for this
//! conversation, and Refuse… (which asks why, for Claude to read). Claude's questions offer
//! their choices, described, or an answer of one's own; its plan, approving it (accepting
//! edits from then, or asking for each) or keeping on planning, saying what to change.

use egit_agents::ToolCall;
use egit_agents::protocol::{Answer, Mode, PermissionRequest};
use egui::text::{LayoutJob, TextFormat};
use egui::{CornerRadius, FontId, Frame, Key, Margin, Modifiers, RichText, Stroke, Ui};
use serde_json::Value;

use super::code::{self, Tone};
use super::{ConversationView, shown_path};
use crate::agents::Conversation;
use crate::theme::{BODY, Palette, RADIUS_BOX, SMALL, Weighted as _, icon};
use crate::widgets::{self, muted};

/// The tallest a request's content shows before it scrolls.
const CONTENT_HEIGHT: f32 = 240.0;

/// What the user chose for a request.
enum Chosen {
    /// Answered as made.
    Answer(Answer),
    /// A plan approved, Claude Code's mode then `Mode`.
    Approve(Answer, Mode),
}

/// The oldest request waiting, in a card over the prompt; answered, it goes.
pub(super) fn show(ui: &mut Ui, conversation: &mut Conversation, work_tree: &std::path::Path) {
    let Some(asked) = conversation.waiting.first().cloned() else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let more = conversation.waiting.len().saturating_sub(1);
    let chosen = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.accent))
        .corner_radius(CornerRadius::same(RADIUS_BOX))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            let view = &mut conversation.view;
            let chosen = match asked.tool.as_str() {
                "AskUserQuestion" => question(ui, &asked, view),
                "ExitPlanMode" => plan(ui, &asked, view),
                _ => permission(ui, &asked, view, work_tree),
            };
            if more > 0 {
                let said = if more == 1 {
                    "1 more waiting after this".to_owned()
                } else {
                    format!("{more} more waiting after this")
                };
                ui.label(muted(ui, said).size(SMALL));
            }
            chosen
        })
        .inner;
    ui.add_space(8.0);
    let Some(chosen) = chosen else {
        return;
    };
    conversation.view.refusing = None;
    conversation.view.reason.clear();
    match chosen {
        Chosen::Answer(answer) => conversation.answer(answer),
        Chosen::Approve(answer, then) => {
            // The mode first: what follows the plan runs in it.
            conversation.set_mode(then);
            conversation.answer(answer);
        }
    }
}

/// A card's headline: an icon in the accent, and words (wrapping beside it).
fn headline(ui: &mut Ui, glyph: &str, words: &str) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(glyph).color(palette.accent).semibold());
        ui.add(egui::Label::new(RichText::new(words).semibold()).wrap());
    });
}

/// A tool's permission: what it would do, why Claude Code asks, and Allow once, Allow for this
/// conversation (when Claude Code offers rules for it), or Refuse… with a reason.
fn permission(
    ui: &mut Ui,
    asked: &PermissionRequest,
    view: &mut ConversationView,
    work_tree: &std::path::Path,
) -> Option<Chosen> {
    let field = |name: &str| {
        asked
            .input
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    headline(ui, icon::SHIELD_CHECK, &wants(asked, work_tree));
    let description = field("description");
    if !description.is_empty() {
        ui.add(egui::Label::new(muted(ui, description)).wrap());
    }
    asked_content(ui, asked, view);
    if let Some(reason) = &asked.reason {
        ui.add(
            egui::Label::new(muted(ui, format!("Claude Code asks: {reason}")).size(SMALL)).wrap(),
        );
    }
    if let Some(path) = &asked.blocked_path {
        let said = format!("{path} is outside the folders it may use without asking.");
        ui.add(egui::Label::new(muted(ui, said).size(SMALL)).wrap());
    }
    if view.refusing.as_deref() == Some(asked.request.as_str()) {
        return refusing(
            ui,
            asked,
            view,
            ("Why not? Claude reads it and goes on without.", "Refuse"),
        );
    }
    let mut chosen = None;
    // Enter, with nothing typed in the prompt, allows it once: the keys answer as the mouse does.
    // Only while the prompt (or no text field) has the keys: Enter elsewhere is another field's.
    let prompt = egui::Id::new(("agent-prompt", work_tree));
    let keys_here = ui.memory(|memory| memory.has_focus(prompt)) || !ui.ctx().text_edit_focused();
    let entered = keys_here
        && view.draft.trim().is_empty()
        && !ui.ctx().memory(|memory| memory.top_modal_layer().is_some())
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    ui.horizontal_wrapped(|ui| {
        if widgets::primary(ui, true, "Allow once")
            .on_hover_text("Allow it this once (Enter, with nothing typed)")
            .clicked()
            || entered
        {
            chosen = Some(Chosen::Answer(allow(asked, false, None)));
        }
        if !asked.suggestions.is_empty()
            && widgets::secondary(ui, true, "Allow for this conversation")
                .on_hover_text("Claude Code's rule for it (this command, these edits), for the rest of this conversation")
                .clicked()
        {
            chosen = Some(Chosen::Answer(allow(asked, true, None)));
        }
        if widgets::secondary(ui, true, "Refuse…").clicked() {
            view.refusing = Some(asked.request.clone());
            view.reason.clear();
            view.focus_reason = true;
        }
    });
    chosen
}

/// What a tool asks to do, shown as suits it: a command, an edit as a diff, a file's content,
/// what it is about (a path, a search), or what it is given.
fn asked_content(ui: &mut Ui, asked: &PermissionRequest, view: &mut ConversationView) {
    let field = |name: &str| {
        asked
            .input
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let language = std::path::Path::new(field("file_path"))
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_owned();
    let highlights = &mut view.highlights;
    egui::ScrollArea::vertical()
        .id_salt(("permission", &asked.request))
        .max_height(CONTENT_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| match asked.tool.as_str() {
            "Bash" | "PowerShell" => {
                code::code_box(
                    ui,
                    ("Command", "sh", Tone::Plain),
                    field("command"),
                    highlights,
                );
            }
            "Edit" => code::diff_box(
                ui,
                (field("old_string"), field("new_string")),
                &language,
                highlights,
            ),
            "MultiEdit" => {
                for edit in asked
                    .input
                    .get("edits")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let part =
                        |name: &str| edit.get(name).and_then(Value::as_str).unwrap_or_default();
                    code::diff_box(
                        ui,
                        (part("old_string"), part("new_string")),
                        &language,
                        highlights,
                    );
                }
            }
            "Write" => {
                let label = ("The file's content", language.as_str(), Tone::Plain);
                code::code_box(ui, label, field("content"), highlights);
            }
            "Read" | "Glob" | "Grep" | "WebFetch" | "WebSearch" | "NotebookEdit" => {
                let call = ToolCall {
                    id: String::new(),
                    name: asked.tool.clone(),
                    input: asked.input.clone(),
                    result: None,
                };
                let (_, detail) = call.summary();
                if !detail.is_empty() {
                    ui.add(egui::Label::new(RichText::new(detail).monospace()).wrap());
                }
            }
            _ => {
                let shown = serde_json::to_string_pretty(&asked.input).unwrap_or_default();
                let label = ("What it is given", "json", Tone::Plain);
                code::code_box(ui, label, &shown, highlights);
            }
        });
}

/// What a tool would do, in its own terms.
fn wants(asked: &PermissionRequest, work_tree: &std::path::Path) -> String {
    let field = |name: &str| {
        asked
            .input
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let path = |name: &str| shown_path(field(name), work_tree).to_owned();
    match asked.tool.as_str() {
        "Bash" | "PowerShell" => "Claude wants to run a command".to_owned(),
        "Edit" | "MultiEdit" => format!("Claude wants to edit {}", path("file_path")),
        "Write" => format!("Claude wants to write {}", path("file_path")),
        "NotebookEdit" => format!(
            "Claude wants to edit the notebook {}",
            path("notebook_path")
        ),
        "Read" => format!("Claude wants to read {}", path("file_path")),
        "Glob" | "Grep" => "Claude wants to search the files".to_owned(),
        "WebFetch" => "Claude wants to fetch a page".to_owned(),
        "WebSearch" => "Claude wants to search the web".to_owned(),
        name => match name
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        {
            Some((server, tool)) => format!(
                "Claude wants to use {} from {server}",
                tool.replace('_', " ")
            ),
            None => format!("Claude wants to use {name}"),
        },
    }
}

/// A refusal being made: why (Enter refuses), then the refusal's button and Back.
fn refusing(
    ui: &mut Ui,
    asked: &PermissionRequest,
    view: &mut ConversationView,
    (asking, button): (&str, &str),
) -> Option<Chosen> {
    let label = ui.label(muted(ui, asking));
    let id = egui::Id::new(("refusal", &asked.request));
    let focused = ui.memory(|memory| memory.has_focus(id));
    let entered = focused && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
    let field = ui
        .add(
            widgets::line_field(&mut view.reason)
                .id(id)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    if std::mem::take(&mut view.focus_reason) {
        field.request_focus();
    }
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        let plan = asked.tool == "ExitPlanMode";
        let clicked = if plan {
            widgets::primary(ui, true, button).clicked()
        } else {
            widgets::danger(ui, true, button).clicked()
        };
        if clicked || entered {
            chosen = Some(Chosen::Answer(refuse(asked, &view.reason, plan)));
        }
        if widgets::secondary(ui, true, "Back").clicked() {
            view.refusing = None;
        }
    });
    chosen
}

/// Claude's questions: each one's choices (their descriptions under them; several where it
/// takes several), or an answer of one's own; Answer, or Don't answer.
fn question(ui: &mut Ui, asked: &PermissionRequest, view: &mut ConversationView) -> Option<Chosen> {
    let questions: Vec<&Value> = asked
        .input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let headline_words = if questions.len() > 1 {
        "Claude asks you a few questions"
    } else {
        "Claude asks"
    };
    headline(ui, icon::QUESTION, headline_words);
    let mut answers = serde_json::Map::new();
    egui::ScrollArea::vertical()
        .id_salt(("questions", &asked.request))
        .max_height(CONTENT_HEIGHT * 1.5)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for question in &questions {
                let (text, answer) = one_question(ui, question, view);
                answers.insert(text, Value::String(answer));
            }
        });
    let complete = answers
        .values()
        .all(|answer| answer.as_str().is_some_and(|answer| !answer.is_empty()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        if widgets::primary(ui, complete, "Answer")
            .on_disabled_hover_text("Choose an answer to each question, or write your own")
            .clicked()
        {
            let mut input = asked.input.clone();
            if let Some(object) = input.as_object_mut() {
                object.insert("answers".to_owned(), Value::Object(answers.clone()));
            }
            chosen = Some(Chosen::Answer(allow(asked, false, Some(input))));
        }
        if widgets::secondary(ui, true, "Don't answer").clicked() {
            chosen = Some(Chosen::Answer(refuse(
                asked,
                "The user chose not to answer.",
                false,
            )));
        }
    });
    chosen
}

/// One question: its words, its choices, its own answer's field; returns the question and the
/// answer made (the choices joined, or the words written).
fn one_question(ui: &mut Ui, question: &Value, view: &mut ConversationView) -> (String, String) {
    let palette = Palette::of(ui.ctx());
    let text = question
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let several = question
        .get("multiSelect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ui.add(egui::Label::new(RichText::new(text.as_str()).medium()).wrap());
    let chosen = view.chosen.entry(text.clone()).or_default();
    for option in question
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |name: &str| option.get(name).and_then(Value::as_str).unwrap_or_default();
        let (label, description) = (field("label"), field("description"));
        let mut words = LayoutJob::default();
        words.append(
            label,
            0.0,
            TextFormat {
                font_id: FontId::proportional(BODY),
                color: palette.text,
                ..TextFormat::default()
            },
        );
        if !description.is_empty() {
            words.append("\n", 0.0, TextFormat::default());
            words.append(
                description,
                0.0,
                TextFormat {
                    font_id: FontId::proportional(SMALL),
                    color: palette.muted,
                    ..TextFormat::default()
                },
            );
        }
        let mut picked = chosen.iter().any(|chosen| chosen == label);
        if several {
            widgets::checkbox_of(ui, &mut picked, words, "agents-questions");
        } else if widgets::radio_of(ui, picked, words, "agents-questions").clicked() {
            picked = true;
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
    let label = ui.label(muted(ui, "Or in your own words").size(SMALL));
    ui.add(widgets::line_field(other).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    let answer = if other.trim().is_empty() {
        chosen.join(", ")
    } else {
        other.trim().to_owned()
    };
    ui.add_space(4.0);
    (text, answer)
}

/// Claude's plan: Approve and accept edits, Approve (asking for each edit), or Keep
/// planning… saying what to change.
fn plan(ui: &mut Ui, asked: &PermissionRequest, view: &mut ConversationView) -> Option<Chosen> {
    headline(ui, icon::MAP_TRIFOLD, "Claude's plan");
    let text = asked
        .input
        .get("plan")
        .and_then(Value::as_str)
        .unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt(("plan", &asked.request))
        .max_height(CONTENT_HEIGHT * 1.5)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            code::markdown(ui, text, (&mut view.markdown, &mut view.highlights), false);
        });
    if view.refusing.as_deref() == Some(asked.request.as_str()) {
        return refusing(
            ui,
            asked,
            view,
            ("What should change? Claude plans again.", "Keep planning"),
        );
    }
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        if widgets::primary(ui, true, "Approve and accept edits")
            .on_hover_text("Claude starts, editing files without asking (Accept edits)")
            .clicked()
        {
            chosen = Some(Chosen::Approve(
                allow(asked, false, None),
                Mode::AcceptEdits,
            ));
        }
        if widgets::secondary(ui, true, "Approve")
            .on_hover_text("Claude starts, asking before each edit (Ask first)")
            .clicked()
        {
            chosen = Some(Chosen::Approve(allow(asked, false, None), Mode::Default));
        }
        if widgets::secondary(ui, true, "Keep planning…").clicked() {
            view.refusing = Some(asked.request.clone());
            view.reason.clear();
            view.focus_reason = true;
        }
    });
    chosen
}

/// Allows a request (`always`: Claude Code's suggested rules too; `input`: what it runs with
/// instead).
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

/// Refuses a request, saying `why` (or that the user refused it; a plan, that they want
/// changes).
fn refuse(asked: &PermissionRequest, why: &str, plan: bool) -> Answer {
    let message = match why.trim() {
        "" if plan => "Keep planning: the user wants changes.".to_owned(),
        "" => "The user refused this.".to_owned(),
        why if plan => format!("Keep planning. The user wants changes: {why}"),
        why => why.to_owned(),
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
