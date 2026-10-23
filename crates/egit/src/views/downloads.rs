//! A tab's downloads, under its commit table (whatever shows in its place): each artifact's
//! file and what it is of, how far it has come (of how much), and Cancel; once saved, where it
//! went, Show in folder, and Dismiss; failed, why. Each in a `Ui` of its own id, as they come
//! and go.

use egui::{Align, Id, Layout, RichText, Ui};

use super::history::Shared;
use crate::app::{Action, DownloadAsk};
use crate::desktop::Reveal;
use crate::downloads::{Download, Stage};
use crate::format;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// The help topic explaining downloads.
const TOPIC: &str = "pipeline-artifacts";

/// Whether the tab has downloads to show.
pub(super) fn any(shared: &Shared<'_>) -> bool {
    !shared.forge.downloads.list().is_empty()
}

/// The tab's downloads, the oldest first.
pub(super) fn notices(ui: &mut Ui, shared: &Shared<'_>, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    for download in shared.forge.downloads.list() {
        ui.scope_builder(
            egui::UiBuilder::new()
                .id(Id::new(("download", shared.id, download.id)))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                if let Some(asked) = notice(ui, shared, palette, download) {
                    actions.push(asked);
                }
            },
        );
    }
}

/// One download's notice; what it asked.
fn notice(
    ui: &mut Ui,
    shared: &Shared<'_>,
    palette: Palette,
    download: &Download,
) -> Option<Action> {
    let wanted = &download.wanted;
    let file = &wanted.artifact.file_name;
    let ask = |asked| Some(Action::Download(shared.id, asked));
    match download.stage() {
        Stage::Running => {
            let done = format::size(download.done());
            let words = wanted.artifact.size.map_or_else(
                || format!("Downloading {file} of {}: {done}", wanted.of),
                |size| {
                    format!(
                        "Downloading {file} of {}: {done} of {}",
                        wanted.of,
                        format::size(size)
                    )
                },
            );
            let mut asked = None;
            ui.horizontal(|ui| {
                ui.spinner();
                let said = ui.add(egui::Label::new(words).truncate());
                crate::help::note_topic(ui, said.rect, TOPIC);
                if widgets::secondary(ui, true, "Cancel").clicked() {
                    asked = ask(DownloadAsk::Cancel(download.id));
                }
            });
            asked
        }
        Stage::Saved => {
            let to = format::display_path(&download.to, shared.home);
            let mut asked = None;
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::CHECK_CIRCLE).color(palette.ok));
                let said = ui.add(egui::Label::new(format!("Saved {file} to {to}")).truncate());
                crate::help::note_topic(ui, said.rect, TOPIC);
                if widgets::secondary(ui, true, "Show in folder").clicked() {
                    asked = Some(Action::Reveal(download.to.clone(), Reveal::InFolder));
                }
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    asked = ask(DownloadAsk::Dismiss(download.id));
                }
            });
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
