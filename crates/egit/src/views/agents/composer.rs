//! The composer under a conversation: what waits for an answer, the images attached, Claude
//! Code's slash commands offered as one is typed, the prompt (Enter sends; Shift+Enter starts
//! a new line), attaching images, the mode and the model, Send or Stop; and who it runs as,
//! with what it has cost (or how to sign in, when nobody is).

use std::path::Path;

use egit_agents::Prompt;
use egit_agents::protocol::{Command, Image, Mode};
use egui::{
    CornerRadius, Frame, Id, Key, Margin, Modifiers, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};

use super::{Around, NARROW, waiting};
use crate::agents::{AccountKind, AgentAccount, AgentsSetup, Conversation, RepoAgents};
use crate::app::Action;
use crate::desktop::FileQuestion;
use crate::theme::{CODE, Palette, RADIUS_CONTROL, RADIUS_INNER, SMALL, Weight, icon};
use crate::views::actions;
use crate::widgets::{self, Look, muted};

/// The prompt field's tallest.
const PROMPT_HEIGHT: f32 = 220.0;
/// The most slash commands offered at once.
const OFFERED: usize = 8;
/// A slash command's row's height.
const OFFER: f32 = 26.0;

/// The slash commands offered as one is typed: the one picked, and the draft they were
/// dismissed for (offered again once it changes).
#[derive(Default)]
pub(super) struct Completion {
    picked: usize,
    dismissed: Option<String>,
}

/// The composer; returns a prompt sent, with its images.
pub(super) fn show(
    ui: &mut Ui,
    (setup, agents): (&mut AgentsSetup, &mut RepoAgents),
    around: Around<'_>,
    actions: &mut Vec<Action>,
) -> Option<(Prompt, Vec<Image>)> {
    let narrow = ui.available_width() < NARROW;
    let conversation = agents.current()?;
    waiting::show(ui, conversation, around.work_tree);
    let sent = prompt(ui, conversation, (around, narrow), actions);
    ui.add_space(6.0);
    who(ui, setup, agents, around.accounts);
    sent
}

/// The images, the slash commands offered, the prompt, and the controls under it; returns
/// the prompt sent (Enter, or Send), with its images.
fn prompt(
    ui: &mut Ui,
    conversation: &mut Conversation,
    (around, narrow): (Around<'_>, bool),
    actions: &mut Vec<Action>,
) -> Option<(Prompt, Vec<Image>)> {
    let id = Id::new(("agent-prompt", around.work_tree));
    let focused = ui.memory(|memory| memory.has_focus(id));
    let commands = conversation
        .info
        .as_ref()
        .map(|info| info.commands.as_slice())
        .unwrap_or_default();
    let offered = offered(
        &conversation.view.draft,
        commands,
        &conversation.view.completion,
    );
    let view = &mut conversation.view;
    let (completed, entered) = keys(ui, (focused, &view.draft), (&offered, &mut view.completion));
    let completed = completed.or_else(|| offers(ui, &offered, conversation.view.completion.picked));
    if let Some(name) = &completed {
        conversation.view.draft = format!("/{name} ");
        conversation.view.completion = Completion::default();
        cursor_to_end(ui.ctx(), id, &conversation.view.draft);
    }
    attachments(ui, conversation);
    let busy = conversation.is_busy();
    let label = label_row(ui, busy);
    let field = widgets::bounded_text_area(
        ui,
        widgets::text_area(&mut conversation.view.draft)
            .id(id)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("What should Claude do?"),
        PROMPT_HEIGHT,
        "agent-prompt",
    )
    .labelled_by(label);
    if completed.is_some() {
        field.request_focus();
    }
    let attaching = conversation.view.is_attaching();
    let ready = !attaching
        && (!conversation.view.draft.trim().is_empty()
            || !conversation.view.attachments.is_empty());
    let mut sent = entered && ready;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let look = if narrow { Look::Compact } else { Look::Full };
        if widgets::icon_tool(ui, true, icon::PAPERCLIP, "Attach images…")
            .on_hover_text("Attach images (PNG, JPEG, GIF, WebP), or drop them on the window")
            .clicked()
        {
            actions.push(Action::AskFiles {
                question: FileQuestion::AttachImages { repo: around.repo },
                title: "Attach images",
                name: String::new(),
            });
        }
        mode_menu(ui, conversation, look);
        model_menu(ui, conversation, look);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if busy {
                // A prompt sent meanwhile (Enter) waits its turn.
                if widgets::secondary(ui, true, &format!("{}  Stop", icon::STOP))
                    .on_hover_text("Stop the turn under way, keeping what was done")
                    .clicked()
                {
                    conversation.interrupt();
                }
            } else if widgets::primary(ui, ready, &format!("{}  Send", icon::PAPER_PLANE_RIGHT))
                .clicked()
            {
                sent = true;
            }
        });
    });
    if !sent || !ready {
        return None;
    }
    field.request_focus();
    let view = &mut conversation.view;
    let text = std::mem::take(&mut view.draft).trim().to_owned();
    let attached = std::mem::take(&mut view.attachments);
    let prompt = Prompt {
        text,
        attachments: attached
            .iter()
            .map(|attached| attached.name.clone())
            .collect(),
    };
    let images = attached
        .iter()
        .map(|attached| (*attached.image).clone())
        .collect();
    Some((prompt, images))
}

/// The field's label, and what Enter does (while Claude works: that a prompt waits its turn),
/// as far as it fits.
fn label_row(ui: &mut Ui, busy: bool) -> Id {
    ui.horizontal(|ui| {
        let label = ui.label(muted(ui, "Ask Claude").size(SMALL));
        let keys = if busy {
            "Claude is working: what you send now waits its turn"
        } else {
            "Enter sends · Shift+Enter, a new line"
        };
        // As much as fits, from the right edge.
        let font = egui::FontId::proportional(SMALL);
        let room = ui.available_width() - 12.0;
        if room > 40.0 {
            let keys = super::fitted(ui, keys, &font, room);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(muted(ui, keys).size(SMALL)).selectable(false));
            });
        }
        label.id
    })
    .inner
}

/// The slash commands offered for `draft`: while it is a `/` and a word, those whose names
/// start with the word, then those holding it; none once dismissed for it.
fn offered<'c>(draft: &str, commands: &'c [Command], completion: &Completion) -> Vec<&'c Command> {
    let Some(word) = draft.strip_prefix('/') else {
        return Vec::new();
    };
    if word.contains(char::is_whitespace) || completion.dismissed.as_deref() == Some(draft) {
        return Vec::new();
    }
    let word = word.to_lowercase();
    let starting = commands
        .iter()
        .filter(|command| command.name.to_lowercase().starts_with(&word));
    let holding = commands.iter().filter(|command| {
        let name = command.name.to_lowercase();
        !name.starts_with(&word) && name.contains(&word)
    });
    let offered: Vec<&Command> = starting.chain(holding).take(OFFERED).collect();
    // The command typed whole, alone: nothing left to complete.
    if offered.len() == 1
        && offered
            .first()
            .is_some_and(|command| command.name.eq_ignore_ascii_case(&word))
    {
        return Vec::new();
    }
    offered
}

/// The keys the offered commands take before the field does (↑ ↓ pick, Tab or Enter
/// completes, Escape dismisses), and Enter sending without them; returns the command completed
/// and whether Enter sent.
fn keys(
    ui: &Ui,
    (focused, draft): (bool, &str),
    (offered, completion): (&[&Command], &mut Completion),
) -> (Option<String>, bool) {
    if !focused {
        return (None, false);
    }
    if offered.is_empty() {
        let entered = ui.input_mut(|input| {
            input.consume_key(Modifiers::NONE, Key::Enter)
                || input.consume_key(Modifiers::COMMAND, Key::Enter)
        });
        return (None, entered);
    }
    let last = offered.len().saturating_sub(1);
    completion.picked = completion.picked.min(last);
    let (up, down, complete, dismiss) = ui.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::Tab)
                || input.consume_key(Modifiers::NONE, Key::Enter),
            input.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if up {
        completion.picked = completion.picked.checked_sub(1).unwrap_or(last);
    }
    if down {
        completion.picked = if completion.picked >= last {
            0
        } else {
            completion.picked.saturating_add(1)
        };
    }
    if dismiss {
        completion.dismissed = Some(draft.to_owned());
    }
    let picked = complete
        .then(|| {
            offered
                .get(completion.picked)
                .map(|command| command.name.clone())
        })
        .flatten();
    (picked, false)
}

/// The commands offered, over the field: each its name, what it takes, and what it does, the
/// one picked marked; returns the one clicked.
fn offers(ui: &mut Ui, offered: &[&Command], picked: usize) -> Option<String> {
    if offered.is_empty() {
        return None;
    }
    let palette = Palette::of(ui.ctx());
    let mut clicked = None;
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, command) in offered.iter().enumerate() {
                let width = ui.available_width();
                let (rect, response) = ui.allocate_exact_size(vec2(width, OFFER), Sense::click());
                let name = format!("/{}", command.name);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name.as_str())
                });
                crate::help::note_topic(ui, rect, "agents-commands");
                widgets::row_fill(
                    ui,
                    rect,
                    f32::from(RADIUS_INNER),
                    index == picked,
                    response.hovered(),
                );
                paint_offer(ui, rect, command, &palette);
                if response.clicked() {
                    clicked = Some(command.name.clone());
                }
                if !command.description.is_empty() {
                    response.on_hover_text(command.description.as_str());
                }
            }
        });
    ui.add_space(6.0);
    clicked
}

/// Paints an offered command: its name, what it takes, then what it does, as far as it fits.
fn paint_offer(ui: &Ui, rect: Rect, command: &Command, palette: &Palette) {
    let painter = ui.painter();
    let middle = rect.center().y;
    let mut x = rect.left() + 8.0;
    let name = painter.layout_no_wrap(
        format!("/{}", command.name),
        egui::FontId::new(CODE, egui::FontFamily::Monospace),
        palette.text,
    );
    let name_width = name.size().x;
    painter.galley(pos2(x, middle - name.size().y / 2.0), name, palette.text);
    x += name_width + 6.0;
    if !command.argument_hint.is_empty() {
        let hint = painter.layout_no_wrap(
            command.argument_hint.clone(),
            egui::FontId::monospace(CODE),
            palette.muted,
        );
        let hint_width = hint.size().x;
        painter.galley(pos2(x, middle - hint.size().y / 2.0), hint, palette.muted);
        x += hint_width + 6.0;
    }
    let room = rect.right() - 8.0 - x;
    if command.description.is_empty() || room < 40.0 {
        return;
    }
    let font = egui::FontId::new(SMALL, Weight::Regular.family());
    let said = super::fitted(ui, &command.description, &font, room);
    let galley = painter.layout_no_wrap(said, font, palette.muted);
    painter.galley(
        pos2(x + 4.0, middle - galley.size().y / 2.0),
        galley,
        palette.muted,
    );
}

/// Moves the field `id`'s cursor to the end of `text`.
fn cursor_to_end(ctx: &egui::Context, id: Id, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let end = egui::text::CCursor::new(text.chars().count());
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ctx, id);
    }
}

/// The images attached, each a chip (its picture, name, size, and × to take it off); those
/// being read; why the last couldn't be.
fn attachments(ui: &mut Ui, conversation: &mut Conversation) {
    let view = &mut conversation.view;
    let attaching = view.is_attaching();
    if let Some(problem) = view.attach_problem.clone() {
        let mut dismissed = false;
        widgets::problem_box(ui, &problem, |ui| {
            dismissed = widgets::secondary(ui, true, "Dismiss").clicked();
        });
        if dismissed {
            view.attach_problem = None;
        }
        ui.add_space(6.0);
    }
    if view.attachments.is_empty() && !attaching {
        return;
    }
    let palette = Palette::of(ui.ctx());
    let mut removed = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (index, attached) in view.attachments.iter_mut().enumerate() {
            if chip(ui, attached, &palette) {
                removed = Some(index);
            }
        }
        if attaching {
            widgets::working(ui, "Reading the images…");
        }
    });
    if let Some(index) = removed
        && index < view.attachments.len()
    {
        view.attachments.remove(index);
    }
    ui.add_space(6.0);
}

/// An image's chip; returns whether its × was clicked.
fn chip(ui: &mut Ui, attached: &mut crate::attachments::Attachment, palette: &Palette) -> bool {
    let texture = match (&attached.texture, attached.thumbnail.take()) {
        (Some(texture), _) => Some(texture.clone()),
        (None, Some(thumbnail)) => {
            let made = ui.ctx().load_texture(
                format!("agent-attachment-{}", attached.name),
                (*thumbnail).clone(),
                egui::TextureOptions::LINEAR,
            );
            attached.texture = Some(made.clone());
            Some(made)
        }
        (None, None) => None,
    };
    let mut removed = false;
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin {
            left: 4,
            right: 0,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                if let Some(texture) = &texture {
                    let size = texture.size_vec2();
                    let scale = 28.0 / size.x.max(size.y).max(1.0);
                    let fitted = vec2(size.x * scale, size.y * scale);
                    let shown = Rect::from_center_size(rect.center(), fitted);
                    ui.painter().image(
                        texture.id(),
                        shown,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(super::elided(&attached.name, 24)).size(SMALL));
                    let size = crate::format::size(attached.bytes);
                    let [width, height] = attached.size;
                    let said = if attached.scaled {
                        format!("{size} · scaled to {width}×{height}")
                    } else {
                        size
                    };
                    ui.label(muted(ui, said).size(SMALL));
                });
                let name = format!("Remove {}", attached.name);
                removed = super::icon_button_of(ui, icon::X, &name, "agents-attachments")
                    .on_hover_text("Take it off the prompt")
                    .clicked();
            });
        });
    removed
}

/// The permission mode's menu: each mode, what it does on hover.
fn mode_menu(ui: &mut Ui, conversation: &mut Conversation, look: Look) {
    let mode = conversation.mode();
    let mut chosen = None;
    widgets::tool_menu(ui, icon::SHIELD_CHECK, mode.name(), look, |ui| {
        actions::menu_rows(ui);
        actions::menu_header(ui, "Permission mode");
        for offered in Mode::OFFERED {
            let glyph = if offered == mode { icon::CHECK } else { "" };
            let danger = offered == Mode::BypassPermissions;
            if actions::described_item(
                ui,
                (glyph, offered.name()),
                Ok(()),
                (danger, about(offered)),
            ) {
                chosen = Some(offered);
            }
        }
    });
    if let Some(mode) = chosen {
        conversation.set_mode(mode);
    }
}

/// What a mode does.
const fn about(mode: Mode) -> &'static str {
    match mode {
        Mode::Default => {
            "Asks before running commands, editing files, or using tools no rule allows"
        }
        Mode::AcceptEdits => "Edits files without asking; asks before the rest",
        Mode::Plan => "Reads and plans, changing nothing, until you approve its plan",
        Mode::Auto => "Claude Code's own classifier decides what needs asking",
        Mode::BypassPermissions => {
            "Asks nothing: commands run too. Only where nothing could matter"
        }
        Mode::DontAsk => "Refuses whatever no rule allows, asking nothing",
    }
}

/// The model's menu, once Claude Code has said which there are (the default's name until
/// then).
fn model_menu(ui: &mut Ui, conversation: &mut Conversation, look: Look) {
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
    widgets::tool_menu_of(ui, (icon::SPARKLE, &name), look, "agents-models", |ui| {
        actions::menu_rows(ui);
        actions::menu_header(ui, "Model");
        for model in &models {
            let glyph = if current.as_deref() == Some(model.value.as_str()) {
                icon::CHECK
            } else {
                ""
            };
            if actions::item_of(ui, (glyph, &model.display_name), Ok(()), "agents-models") {
                chosen = Some(model.value.clone());
            }
        }
    });
    if let Some(model) = chosen {
        conversation.set_model(Some(model));
    }
}

/// Who the conversation runs as (its account's menu, chosen before it begins), who is signed
/// in there, and what it has cost; how to sign in when nobody is.
fn who(ui: &mut Ui, setup: &mut AgentsSetup, agents: &mut RepoAgents, accounts: &[AgentAccount]) {
    let Some(conversation) = agents.current() else {
        return;
    };
    let account = conversation.account.clone();
    let begun = conversation.has_begun();
    let cost = conversation.transcript.cost_usd();
    let told = conversation.info.as_ref().and_then(|info| {
        let email = info.account.email.clone()?;
        Some(match &info.account.subscription_type {
            Some(plan) => format!("{email} · {plan}"),
            None => email,
        })
    });
    let folder = match account.as_ref().map(|account| &account.kind) {
        None => Some(None),
        Some(AccountKind::Folder(folder)) => Some(Some(folder.to_path())),
        Some(AccountKind::ApiKey) => None,
    };
    let mut chosen = None;
    let mut sign_in = None;
    ui.horizontal(|ui| {
        chosen = account_menu(ui, account.as_ref(), accounts, begun);
        let said = match (&told, &folder) {
            (Some(told), _) => told.clone(),
            (None, None) => "an API key, from git's credential helper".to_owned(),
            (None, Some(folder)) => match setup.who(folder.as_deref()) {
                Some(Ok(status)) if status.logged_in => status
                    .email
                    .clone()
                    .unwrap_or_else(|| status.auth_method.clone()),
                Some(Ok(_)) => {
                    sign_in = Some(folder.clone());
                    "nobody signed in".to_owned()
                }
                Some(Err(problem)) => problem.title.clone(),
                None => String::new(),
            },
        };
        let said = if cost > 0.0 {
            format!("{said} · ≈ ${cost:.2}")
        } else {
            said
        };
        ui.add(egui::Label::new(muted(ui, said).size(SMALL)).truncate())
            .on_hover_text(
                "Who it runs as, and what its turns cost as Claude Code estimates it (a plan's \
                 usage isn't billed by it)",
            );
    });
    match chosen {
        Some(Chosen::Own) => agents.choose_account(None),
        Some(Chosen::Account(account)) => agents.choose_account(Some(account)),
        None => {}
    }
    if let Some(folder) = sign_in {
        signing_in(ui, setup, folder.as_deref());
    }
}

/// The accounts' menu: Claude Code's own, then the others (chosen before a conversation
/// begins); returns the one chosen.
fn account_menu(
    ui: &mut Ui,
    account: Option<&AgentAccount>,
    accounts: &[AgentAccount],
    begun: bool,
) -> Option<Chosen> {
    let name = account.map_or(OWN, |account| account.name.as_str());
    let allowed = if begun {
        Err("It has begun: start a new conversation to run as another")
    } else {
        Ok(())
    };
    let mut chosen = None;
    widgets::tool_menu_of(
        ui,
        (icon::USER_CIRCLE, name),
        Look::Full,
        "agents-accounts",
        |ui| {
            actions::menu_rows(ui);
            actions::menu_header(ui, "Runs as");
            let mark = |this: bool| if this { icon::CHECK } else { "" };
            if actions::item_of(
                ui,
                (mark(account.is_none()), OWN),
                allowed,
                "agents-accounts",
            ) {
                chosen = Some(Chosen::Own);
            }
            for other in accounts {
                let this = account.is_some_and(|account| account.name == other.name);
                if actions::item_of(ui, (mark(this), &other.name), allowed, "agents-accounts") {
                    chosen = Some(Chosen::Account(other.clone()));
                }
            }
            if accounts.is_empty() {
                ui.label(muted(ui, "Others are added in Preferences.").size(SMALL));
            }
        },
    );
    chosen
}

/// An account chosen from the menu.
enum Chosen {
    /// Claude Code's own.
    Own,
    Account(AgentAccount),
}

/// What Claude Code's own account is called.
const OWN: &str = "Claude Code's own";

/// Nobody is signed in to `folder` (`None`: Claude Code's own): the command signing in with
/// Claude Code itself, to copy, and Look again.
fn signing_in(ui: &mut Ui, setup: &mut AgentsSetup, folder: Option<&Path>) {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.warn.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, palette.warn.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{}  Nobody is signed in. Sign in with Claude Code itself, in a terminal:",
                        icon::WARNING
                    ))
                    .color(palette.warn),
                )
                .wrap(),
            );
            let Some(claude) = setup
                .install
                .as_ref()
                .and_then(egit_agents::install::Install::claude)
            else {
                return;
            };
            let command = egit_agents::accounts::login_command(&claude, folder);
            ui.add(
                egui::Label::new(RichText::new(command.as_str()).monospace().size(CODE))
                    .wrap()
                    .selectable(true),
            );
            ui.horizontal_wrapped(|ui| {
                if widgets::secondary(ui, true, "Copy the command").clicked() {
                    ui.ctx().copy_text(command.clone());
                }
                if widgets::secondary(ui, true, "Look again").clicked() {
                    setup.ask_who_again(folder);
                }
            });
        });
}
