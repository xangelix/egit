//! Git's and ssh's questions, asked in the window: passwords, passphrases, user names, decisions
//! about hosts, and security keys waiting for a touch.
//!
//! The askpass server (`egit_git::askpass`) brings each question; they are answered one at a
//! time, in a dialog over the window. A question whose asker went away (git was cancelled, the
//! key was touched) is dropped. Answers are wiped from memory once handed over, and so is the
//! text field's own copy (egui keeps one for undo).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use egit_git::SshSharing;
use egit_git::askpass::{AskpassServer, Prompt, PromptKind};
use egui::{Id, Key, Modal, RichText};
use zeroize::Zeroizing;

use crate::jobs;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// How often a shown question checks that its asker is still waiting.
const ABANDON_CHECK: Duration = Duration::from_millis(250);

/// The questions waiting for an answer.
pub(crate) struct Prompts {
    /// Keeps the socket open; `None` when questions can't be relayed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "held for its socket: dropping it closes the server"
        )
    )]
    server: Option<AskpassServer>,
    /// ssh's connections shared while egit runs, so a key's passphrase is asked once.
    ssh: Option<SshSharing>,
    incoming: Option<Receiver<Prompt>>,
    queue: VecDeque<Prompt>,
    /// What is typed for the question shown.
    answer: Zeroizing<String>,
    /// The question the answer field belongs to.
    answering: Option<u64>,
    /// Why questions can't be relayed, when they can't.
    pub(crate) unavailable: Option<String>,
}

impl Prompts {
    /// Relays questions for egit's own executable, waking `ctx` when one arrives, and shares
    /// ssh's connections, so each is asked once. Returns the environment git needs for both
    /// (empty when neither could start).
    pub(crate) fn start(ctx: &egui::Context) -> (Self, Vec<(OsString, OsString)>) {
        let (mut prompts, mut environment) = match std::env::current_exe() {
            Ok(executable) => Self::start_with(ctx, &executable, None),
            Err(error) => (
                Self::unavailable(format!("egit can't find itself: {error}")),
                Vec::new(),
            ),
        };
        prompts.ssh = SshSharing::start();
        if let Some(ssh) = &prompts.ssh {
            environment.extend(ssh.environment());
        }
        (prompts, environment)
    }

    /// Closes ssh's shared connections (egit is ending).
    pub(crate) fn close_connections(&mut self) {
        self.ssh = None;
    }

    /// As [`Self::start`], for `executable`, under `base` when given.
    pub(crate) fn start_with(
        ctx: &egui::Context,
        executable: &Path,
        base: Option<&Path>,
    ) -> (Self, Vec<(OsString, OsString)>) {
        let waker = ctx.clone();
        let notify = move || waker.request_repaint();
        let started = match base {
            Some(base) => AskpassServer::start_in(base, executable, notify),
            None => AskpassServer::start(executable, notify),
        };
        match started {
            Ok((server, incoming)) => {
                let environment = server.environment();
                (
                    Self {
                        server: Some(server),
                        ssh: None,
                        incoming: Some(incoming),
                        queue: VecDeque::new(),
                        answer: Zeroizing::new(String::new()),
                        answering: None,
                        unavailable: None,
                    },
                    environment,
                )
            }
            Err(error) => {
                tracing::warn!(%error, "git's questions can't be shown");
                (Self::unavailable(error.to_string()), Vec::new())
            }
        }
    }

    fn unavailable(reason: String) -> Self {
        Self {
            server: None,
            ssh: None,
            incoming: None,
            queue: VecDeque::new(),
            answer: Zeroizing::new(String::new()),
            answering: None,
            unavailable: Some(reason),
        }
    }

    /// No questions are relayed (tests that don't need them).
    #[cfg(test)]
    pub(crate) fn none() -> Self {
        Self::unavailable("not started".to_owned())
    }

    /// The environment git needs to ask through this window.
    #[cfg(test)]
    pub(crate) fn environment(&self) -> Vec<(OsString, OsString)> {
        self.server
            .as_ref()
            .map_or_else(Vec::new, AskpassServer::environment)
    }

    /// How many questions are waiting.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.queue.len()
    }

    /// Takes the questions that arrived and drops those whose asker went away.
    pub(crate) fn poll(&mut self) {
        if let Some(incoming) = &self.incoming {
            self.queue.extend(incoming.try_iter());
        }
        // Dropping a prompt refuses it; one abandoned has nobody left to refuse.
        self.queue.retain(|prompt| !prompt.is_abandoned());
        let front = self.queue.front().map(Prompt::id);
        if front != self.answering {
            self.answer.clear();
            self.answering = front;
        }
    }

    /// Shows the first question, if any.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.queue.front() else {
            return;
        };
        // The asker may leave while the question is shown.
        ctx.request_repaint_after(ABANDON_CHECK);
        let palette = Palette::of(ctx);
        let kind = prompt.kind();
        let field_id = Id::new(("askpass-answer", prompt.id()));
        let mut decision = None;
        let frame = egui::Frame::new()
            .fill(palette.surface)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
            .inner_margin(egui::Margin::same(20));
        let modal = Modal::new(Id::new("askpass")).frame(frame).show(ctx, |ui| {
            ui.set_width(480.0);
            let (glyph, title) = title(kind);
            widgets::panel_title(ui, format!("{glyph}  {title}"));
            if let Some(context) = prompt.context() {
                ui.label(muted(ui, format!("While {}", lowercase_first(context))));
            }
            ui.add_space(8.0);
            let question = ui.add(
                egui::Label::new(prompt.text().trim_end())
                    .selectable(true)
                    .wrap(),
            );
            let topic = match kind {
                PromptKind::Visible => "prompt-user",
                PromptKind::Secret => "prompt-secret",
                PromptKind::YesNo => "prompt-host-key",
                PromptKind::Confirm => "prompt-confirm",
                PromptKind::Notice => "prompt-notice",
            };
            crate::help::note_topic(ui, question.rect, topic);
            ui.add_space(8.0);
            match kind {
                PromptKind::Visible | PromptKind::Secret => {
                    let field = ui
                        .add(
                            crate::widgets::line_field(&mut *self.answer)
                                .id(field_id)
                                .password(kind == PromptKind::Secret)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(question.id);
                    if !field.has_focus() && !field.lost_focus() {
                        field.request_focus();
                    }
                    let entered =
                        field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                    ui.add_space(8.0);
                    widgets::buttons_row(ui, |ui| {
                        if widgets::primary(ui, true, "OK").clicked() || entered {
                            decision = Some(Decision::Answer);
                        }
                        if widgets::secondary(ui, true, "Cancel").clicked() {
                            decision = Some(Decision::Refuse);
                        }
                    });
                }
                PromptKind::YesNo => widgets::buttons_row(ui, |ui| {
                    if widgets::primary(ui, true, "Yes").clicked() {
                        decision = Some(Decision::Say("yes"));
                    }
                    if widgets::secondary(ui, true, "No").clicked() {
                        decision = Some(Decision::Say("no"));
                    }
                }),
                PromptKind::Confirm => widgets::buttons_row(ui, |ui| {
                    if widgets::primary(ui, true, "Allow").clicked() {
                        decision = Some(Decision::Say(""));
                    }
                    if widgets::secondary(ui, true, "Deny").clicked() {
                        decision = Some(Decision::Refuse);
                    }
                }),
                PromptKind::Notice => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Waiting…").color(palette.muted));
                    });
                    widgets::buttons_row(ui, |ui| {
                        if widgets::secondary(ui, true, "Dismiss").clicked() {
                            decision = Some(Decision::Refuse);
                        }
                    });
                }
            }
        });
        if decision.is_none() && modal.should_close() {
            decision = Some(Decision::Refuse);
        }
        if let Some(decision) = decision {
            self.decide(ctx, field_id, decision);
        }
    }

    /// Hands the first question its answer, off the interface thread, and forgets the answer.
    fn decide(&mut self, ctx: &egui::Context, field_id: Id, decision: Decision) {
        let Some(prompt) = self.queue.pop_front() else {
            return;
        };
        let answer = match decision {
            Decision::Answer => Some(Zeroizing::new(self.answer.as_bytes().to_vec())),
            Decision::Say(text) => Some(Zeroizing::new(text.as_bytes().to_vec())),
            Decision::Refuse => None,
        };
        self.answer.clear();
        self.answering = None;
        // egui keeps the field's text for undo; that copy goes too.
        ctx.data_mut(|data| data.remove::<egui::text_edit::TextEditState>(field_id));
        jobs::detached(move || {
            let handed = match answer {
                Some(answer) => prompt.answer(&answer),
                None => prompt.refuse(),
            };
            if let Err(error) = handed {
                tracing::debug!(%error, "the asker left before the answer");
            }
        });
    }
}

/// What the user decided about a question.
#[derive(Clone, Copy, Debug)]
enum Decision {
    /// Hand over what was typed.
    Answer,
    /// Hand over this text.
    Say(&'static str),
    /// Refuse.
    Refuse,
}

/// The dialog's icon and title for a kind of question.
const fn title(kind: PromptKind) -> (&'static str, &'static str) {
    match kind {
        PromptKind::Visible => (icon::USER, "A user name is needed"),
        PromptKind::Secret => (icon::LOCK_KEY, "A password or passphrase is needed"),
        PromptKind::YesNo => (icon::QUESTION, "Your decision is needed"),
        PromptKind::Confirm => (icon::KEY, "Allow the use of a key?"),
        PromptKind::Notice => (icon::FINGERPRINT, "Waiting for your security key"),
    }
}

/// `text` with its first letter in lower case, to follow "While".
fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_follows_while() {
        assert_eq!(lowercase_first("Fetching origin"), "fetching origin");
        assert_eq!(lowercase_first(""), "");
        assert_eq!(lowercase_first("Élan"), "élan");
    }
}
