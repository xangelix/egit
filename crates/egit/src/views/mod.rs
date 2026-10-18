//! What the tabs show, and the status bar.

mod actions;
mod blame;
pub(crate) mod clone;
mod dialogs;
mod file;
mod file_history;
pub(crate) mod history;
pub(crate) mod home;
mod lfs;
mod merge;
pub(crate) mod palette;
mod patch;
pub(crate) mod preferences;
pub(crate) mod profile;
pub(crate) mod prompts;
mod pull_form;
mod pulls;
mod rebase;
mod reflog;
pub(crate) mod repo;
mod search;
mod sidebar;
pub(crate) mod status_bar;
mod submodules;
mod tasks;
mod work;

use egui::{ScrollArea, Ui};

/// The widest a page's content grows; wider windows centre it.
pub(crate) const PAGE_WIDTH: f32 = 880.0;

/// A tab's page: it scrolls, with its content centred and at most [`PAGE_WIDTH`] wide.
pub(crate) fn page(ui: &mut Ui, id: impl egui::AsIdSalt, add_contents: impl FnOnce(&mut Ui)) {
    // On the chrome's tone, so the cards stand off it.
    let palette = crate::theme::Palette::of(ui.ctx());
    ui.painter()
        .rect_filled(ui.available_rect_before_wrap(), 0.0, palette.panel);
    ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            let width = available.min(PAGE_WIDTH);
            let margin = ((available - width) / 2.0).max(16.0);
            ui.horizontal(|ui| {
                ui.add_space(margin);
                ui.vertical(|ui| {
                    ui.set_width((available - 2.0 * margin).max(0.0));
                    ui.add_space(24.0);
                    add_contents(ui);
                    ui.add_space(24.0);
                });
            });
        });
}
