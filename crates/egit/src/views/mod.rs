//! What the tabs show, and the status bar.

mod file;
pub(crate) mod history;
pub(crate) mod home;
pub(crate) mod profile;
pub(crate) mod prompts;
pub(crate) mod repo;
pub(crate) mod status_bar;

use egui::{ScrollArea, Ui};

/// The widest a page's content grows; wider windows centre it.
pub(crate) const PAGE_WIDTH: f32 = 880.0;

/// A tab's page: it scrolls, with its content centred and at most [`PAGE_WIDTH`] wide.
pub(crate) fn page(ui: &mut Ui, id: impl egui::AsIdSalt, add_contents: impl FnOnce(&mut Ui)) {
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
