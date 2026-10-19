//! Claude's setup, until it is done: what agents are, then each step in turn (Node.js found,
//! the SDK installed, someone signed in), each marked done, under way, to do, or failed, the
//! one to do now with its button.

use egui::{Align, Layout, RichText, Ui, vec2};

use crate::agents::AgentsSetup;
use crate::app::Action;
use crate::theme::{Palette, Weighted as _, icon};
use crate::widgets::{self, muted};

/// Where a step stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stands {
    Done,
    Working,
    Now,
    Later,
    Failed,
}

/// The setup card.
pub(super) fn show(ui: &mut Ui, setup: &mut AgentsSetup, actions: &mut Vec<Action>) {
    let there = setup.elsewhere().map(str::to_owned);
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Set up Claude");
        ui.add_space(4.0);
        let about = "Agents run Claude Code beside your repository, as it runs in a terminal: \
                     its tools, your settings, CLAUDE.md files, hooks, and sessions, through \
                     Anthropic's Claude Agent SDK.";
        ui.add(egui::Label::new(muted(ui, about)).wrap());
        if let Some(host) = &there {
            let said = format!(
                "This repository is on {host}: Claude Code runs there, beside it, set up \
                 there as below (its Node, its cache folder, its login)."
            );
            ui.add(egui::Label::new(muted(ui, said)).wrap());
        }
        ui.add_space(12.0);
        let node = node_step(ui, setup, actions);
        ui.add_space(8.0);
        install_step(ui, setup, node);
        ui.add_space(8.0);
        step(
            ui,
            (Stands::Later, "Sign in"),
            "With Claude Code itself, once it is installed (or add an API key's account in \
             Preferences).",
            |_| {},
        );
    });
}

/// Node.js and npm: found (its version), being looked for, or missing (Look again,
/// nodejs.org). Returns whether it was found.
fn node_step(ui: &mut Ui, setup: &mut AgentsSetup, actions: &mut Vec<Action>) -> bool {
    let minimum = egit_agents::install::NODE_MINIMUM;
    let title = setup.elsewhere().map_or_else(
        || format!("Node.js {minimum} or newer, and npm"),
        |host| format!("Node.js {minimum} or newer, and npm, on {host}"),
    );
    match setup.found.read_as_ref() {
        None => {
            step(ui, (Stands::Working, &title), "Looking for them…", |_| {});
            false
        }
        Some(Err(problem)) => {
            let detail = format!("{}. Install them, then look again.", problem.detail);
            let mut again = false;
            step(ui, (Stands::Failed, &title), &detail, |ui| {
                ui.horizontal_wrapped(|ui| {
                    again = widgets::secondary(ui, true, "Look again").clicked();
                    if widgets::link(ui, "nodejs.org").clicked() {
                        actions.push(Action::OpenUrl("https://nodejs.org/".to_owned()));
                    }
                });
            });
            if again {
                setup.look_again();
            }
            false
        }
        Some(Ok(found)) => {
            let detail = format!("Node.js {} found.", found.node.version);
            step(ui, (Stands::Done, &title), &detail, |_| {});
            true
        }
    }
}

/// The SDK and Claude Code: to install (its size, the button), installing (its time, Cancel),
/// or failed (why, and the button again).
fn install_step(ui: &mut Ui, setup: &mut AgentsSetup, node: bool) {
    let title = "The Claude Agent SDK and Claude Code";
    let what = format!(
        "Version {}, about {} MB from npm, into egit's cache; no install scripts run.",
        egit_agents::install::SDK_VERSION,
        egit_agents::install::INSTALLED_MEGABYTES
    );
    if !node {
        step(ui, (Stands::Later, title), &what, |_| {});
        return;
    }
    if setup.installing.is_pending() {
        let elapsed = setup.installing.since_started();
        let mut cancel = false;
        step(ui, (Stands::Working, title), &what, |ui| {
            ui.horizontal(|ui| {
                ui.label(muted(ui, format!("Installing… {elapsed:.0} s")));
                cancel = widgets::secondary(ui, true, "Cancel").clicked();
            });
        });
        if cancel {
            setup.cancel();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
        return;
    }
    let problem = setup.install_problem.clone();
    let stands = if problem.is_some() {
        Stands::Failed
    } else {
        Stands::Now
    };
    let mut install = false;
    step(ui, (stands, title), &what, |ui| {
        if let Some(problem) = &problem {
            widgets::problem_box(ui, problem, |_| {});
            ui.add_space(6.0);
        }
        install = widgets::primary(ui, true, "Set up Claude").clicked();
    });
    if install {
        setup.install();
    }
}

/// A step: its mark (done, under way, to do now or later, failed), its title, what it is, and
/// `add` under them (its buttons).
fn step(ui: &mut Ui, (stands, title): (Stands, &str), detail: &str, add: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
        let (glyph, color) = match stands {
            Stands::Done => (icon::CHECK_CIRCLE, palette.ok),
            Stands::Working => ("", palette.accent),
            Stands::Now => (icon::ARROW_CIRCLE_RIGHT, palette.accent),
            Stands::Later => (icon::CIRCLE, palette.muted),
            Stands::Failed => (icon::WARNING_CIRCLE, palette.danger),
        };
        if stands == Stands::Working {
            egui::Spinner::new()
                .size(14.0)
                .color(color)
                .paint_at(ui, rect.shrink(3.0));
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(16.0),
                color,
            );
        }
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let title = RichText::new(title).medium();
                let title = if stands == Stands::Later {
                    title.color(palette.muted)
                } else {
                    title
                };
                ui.add(egui::Label::new(title).wrap());
                ui.add(egui::Label::new(muted(ui, detail).size(crate::theme::SMALL)).wrap());
                add(ui);
            },
        );
    });
}
