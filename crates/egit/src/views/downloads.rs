//! A tab's downloads, under its commit table (whatever shows in its place): each artifact's
//! file and what it is of, how far it has come (a bar of fixed width, of how much), and Cancel at
//! the row's end, where it stays; once saved, where it went, Open, Show in folder, and Dismiss;
//! failed, why. Each in a `Ui` of its own id, as they come and go.

use egui::{Align, Id, Layout, Ui};

use super::history::Shared;
use crate::app::{Action, DownloadAsk};
use crate::desktop::Reveal;
use crate::downloads::{Download, Stage};
use crate::format;
use crate::widgets::{self, muted};

/// The help topic explaining downloads.
const TOPIC: &str = "pipeline-artifacts";

/// The progress bar's width.
const PROGRESS: f32 = 200.0;

/// Whether the tab has downloads to show.
pub(super) fn any(shared: &Shared<'_>) -> bool {
    !shared.forge.downloads.list().is_empty()
}

/// The tab's downloads, the oldest first.
pub(super) fn notices(ui: &mut Ui, shared: &Shared<'_>, actions: &mut Vec<Action>) {
    for download in shared.forge.downloads.list() {
        ui.scope_builder(
            egui::UiBuilder::new()
                .id(Id::new(("download", shared.id, download.id)))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                if let Some(asked) = notice(ui, shared, download) {
                    actions.push(asked);
                }
            },
        );
    }
}

/// One download's notice; what it asked.
fn notice(ui: &mut Ui, shared: &Shared<'_>, download: &Download) -> Option<Action> {
    let wanted = &download.wanted;
    let file = &wanted.artifact.file_name;
    let ask = |asked| Some(Action::Download(shared.id, asked));
    match download.stage() {
        Stage::Running => {
            let done = download.done();
            let (fraction, said) = wanted.artifact.size.filter(|size| *size > 0).map_or_else(
                || (None, format::size(done)),
                |size| {
                    (
                        Some(crate::grid::ratio(done, size)),
                        format!("{} of {}", format::size(done), format::size(size)),
                    )
                },
            );
            let mut asked = None;
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::secondary(ui, true, "Cancel").clicked() {
                        asked = ask(DownloadAsk::Cancel(download.id));
                    }
                    // A bar of its own width, its words inside: what comes moves nothing.
                    let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0))
                        .desired_width(PROGRESS)
                        .text(said)
                        .animate(fraction.is_none());
                    ui.add(bar);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let words = format!("Downloading {file} of {}", wanted.of);
                        let said = ui.add(egui::Label::new(words).truncate());
                        crate::help::note_topic(ui, said.rect, TOPIC);
                    });
                });
            });
            asked
        }
        Stage::Saved => {
            let to = format::display_path(&download.to, shared.home);
            let mut asked = None;
            // In a browser, its downloads have it: nothing of theirs to show from here.
            let words = if crate::desktop::IN_BROWSER {
                format!("{file} went to the browser's downloads")
            } else {
                format!("Saved {file} to {to}")
            };
            let dismissed = widgets::notice(ui, widgets::Tone::Done, words, true, |ui| {
                if crate::desktop::IN_BROWSER {
                    return;
                }
                if widgets::secondary(ui, true, "Show in folder").clicked() {
                    asked = Some(Action::Reveal(download.to.clone(), Reveal::InFolder));
                }
                if widgets::secondary(ui, true, "Open").clicked() {
                    asked = Some(Action::Reveal(download.to.clone(), Reveal::Open));
                }
            });
            crate::help::note_topic(ui, ui.min_rect(), TOPIC);
            if dismissed {
                asked = ask(DownloadAsk::Dismiss(download.id));
            }
            asked
        }
        Stage::Failed(problem) => {
            let mut dismissed = false;
            widgets::problem_box(ui, &problem, |ui| {
                ui.label(muted(ui, format!("{file} of {}", wanted.of)));
                dismissed = widgets::secondary(ui, true, "Dismiss").clicked();
            });
            if dismissed {
                ask(DownloadAsk::Dismiss(download.id))
            } else {
                None
            }
        }
    }
}
