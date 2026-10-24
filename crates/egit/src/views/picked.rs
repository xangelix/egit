//! Commits picked together in the commit table, for what acts on several at once: ⇧-click
//! picks every row from the last one clicked to this one, ⌘-click (Ctrl elsewhere) adds one or
//! takes it away, ⇧↑ and ⇧↓ stretch the range, Escape lets them go. They are kept by their
//! names, so a history read again keeps them (where they are still in it); while several are
//! picked, the details show them, and a right click on one offers what can be done to all:
//! signing them, comparing two, saving or copying them as patches (their changes as one, or
//! one each), copying a changelog of them.

use std::collections::HashSet;

use egit_git::{MAX_SIGNED, ObjectId};
use egui::{Label, Modifiers, RichText, Ui};

use super::actions::Menus;
use super::history::Shared;
use crate::history::{History, Summaries};
use crate::theme::icon;
use crate::widgets::{self, muted};

/// The most rows a range picks: as many as a signing makes again.
const MAX_PICKED: u64 = 10_000;

/// The most picked commits the details list (the newest first).
const LISTED: usize = 200;

/// How a row was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Click {
    /// Alone: it is the selection, and picking starts over from it.
    Plain,
    /// With ⇧: every row from the last one clicked alone.
    Range,
    /// With ⌘ (Ctrl): added, or taken away.
    Toggle,
}

impl Click {
    /// The click the keys held make.
    pub(super) const fn of(modifiers: Modifiers) -> Self {
        if modifiers.shift {
            Self::Range
        } else if modifiers.command {
            Self::Toggle
        } else {
            Self::Plain
        }
    }
}

/// The commits picked, the newest (the highest row) first.
#[derive(Debug, Default)]
pub(super) struct Picked {
    /// Each one's row, as last seen, and name, by row.
    rows: Vec<(u64, ObjectId)>,
    set: HashSet<ObjectId>,
    /// Where a range runs from: the last row clicked alone or with ⌘.
    anchor: Option<ObjectId>,
}

impl Picked {
    /// Whether several are picked: one alone is just the selection.
    pub(super) fn is_several(&self) -> bool {
        self.set.len() > 1
    }

    /// How many are picked.
    pub(super) fn len(&self) -> usize {
        self.set.len()
    }

    /// Whether `id` is one of them.
    pub(super) fn contains(&self, id: &ObjectId) -> bool {
        self.set.contains(id)
    }

    /// Their names, the newest first.
    pub(super) fn ids(&self) -> Vec<ObjectId> {
        self.rows.iter().map(|(_, id)| *id).collect()
    }

    /// Lets them go (the anchor stays).
    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.set.clear();
    }

    /// Picks just these two, as a ⌘-click on each would; returns whether the history shows
    /// both.
    pub(super) fn pick_two(
        &mut self,
        history: &History,
        (one, other): (ObjectId, ObjectId),
    ) -> bool {
        self.clear();
        for id in [one, other] {
            if let Some(row) = history.row_of(&id, 0) {
                self.insert(row, id);
            }
        }
        if self.is_several() {
            self.anchor = Some(other);
            true
        } else {
            self.clear();
            false
        }
    }

    fn insert(&mut self, row: u64, id: ObjectId) {
        if self.set.insert(id) {
            let at = self.rows.partition_point(|(other, _)| *other < row);
            self.rows.insert(at, (row, id));
        }
    }

    /// Takes in `click` on `row`; `selected` is the row selected before it, which a ⌘-click
    /// picks too (it was the one picked so far).
    pub(super) fn click(
        &mut self,
        history: &History,
        row: u64,
        selected: Option<(u64, ObjectId)>,
        click: Click,
    ) {
        let Some(id) = history.id(row) else {
            return;
        };
        match click {
            Click::Plain => {
                self.clear();
                self.anchor = Some(id);
            }
            Click::Toggle => {
                if self.set.is_empty()
                    && let Some((at, was)) = selected
                    && was != id
                {
                    self.insert(at, was);
                }
                if self.set.remove(&id) {
                    self.rows.retain(|(_, other)| *other != id);
                } else {
                    self.insert(row, id);
                }
                self.anchor = Some(id);
            }
            Click::Range => {
                let from = self
                    .anchor
                    .and_then(|anchor| history.row_of(&anchor, row))
                    .or_else(|| selected.map(|(at, _)| at))
                    .unwrap_or(row);
                let (low, high) = (from.min(row), from.max(row));
                let high = high.min(low.saturating_add(MAX_PICKED - 1));
                self.clear();
                for at in low..=high {
                    if let Some(id) = history.id(at) {
                        self.insert(at, id);
                    }
                }
            }
        }
        if !self.is_several() {
            self.clear();
        }
    }

    /// After the history was read again: each found again near its old row, those gone (made
    /// again by a rewrite, say) let go.
    pub(super) fn follow(&mut self, history: &History) {
        let rows = std::mem::take(&mut self.rows);
        self.set.clear();
        for (row, id) in rows {
            if let Some(row) = history.row_of(&id, row) {
                self.insert(row, id);
            }
        }
        if !self.is_several() {
            self.clear();
        }
    }
}

/// What the picked commits' panel asked for.
#[derive(Debug)]
pub(super) enum PickedEvent {
    /// Ask whether to sign them.
    Sign(Vec<ObjectId>),
    /// Compare two: what changed from the older (`from`) to the newer (`to`).
    Compare { from: ObjectId, to: ObjectId },
    /// Export them (the newest first).
    Export(Vec<ObjectId>, Exporting),
    /// Let them go.
    Clear,
}

/// How picked commits are exported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Exporting {
    /// Their changes as one patch, saved where the user says.
    SaveOne,
    /// Their changes as one patch, copied.
    CopyOne,
    /// One patch each, saved into the folder the user says.
    SaveEach,
    /// One patch each, copied.
    CopyEach,
    /// A changelog of their subjects, copied.
    Changelog,
}

/// The two picked, the older first, if exactly two are.
pub(super) const fn two(ids: &[ObjectId]) -> Result<(ObjectId, ObjectId), &'static str> {
    match ids {
        [newer, older] => Ok((*older, *newer)),
        _ => Err("Pick exactly two commits to compare"),
    }
}

/// Why the picked commits can't be signed now, if they can't.
pub(super) const fn signing_allowed(menus: &Menus<'_>, count: usize) -> Result<(), &'static str> {
    if menus.branch.is_none() {
        Err("Switch to a branch first: signing makes its commits again")
    } else if count > MAX_SIGNED {
        Err("Too many: pick fewer")
    } else {
        menus.can_change()
    }
}

/// The details' place while several commits are picked: how many, what can be done to them
/// all, and which they are.
pub(super) fn panel(
    ui: &mut Ui,
    shared: &Shared<'_>,
    summaries: &Summaries,
    picked: &Picked,
    menus: &Menus<'_>,
) -> Option<PickedEvent> {
    let mut event = None;
    widgets::panel_head(ui, widgets::Head::Title, |ui| {
        widgets::panel_title(ui, format!("{} commits selected", picked.len()));
    });
    let (range, one) = clicks();
    ui.add(
        Label::new(muted(
            ui,
            format!("{range} picks a range, {one} adds or takes away one; Escape lets them go."),
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    let ids = picked.ids();
    // Wrapped in the panel's width: a row of them all would widen the panel past the table.
    ui.horizontal_wrapped(|ui| {
        let allowed = signing_allowed(menus, ids.len());
        let sign = widgets::primary(ui, allowed.is_ok(), "Sign these commits…");
        let sign = match allowed {
            Err(why) => sign.on_disabled_hover_text(why),
            Ok(()) => sign,
        };
        if sign.clicked() {
            event = Some(PickedEvent::Sign(ids.clone()));
        }
        if let Ok((from, to)) = two(&ids)
            && widgets::secondary(ui, true, "Compare these two").clicked()
        {
            event = Some(PickedEvent::Compare { from, to });
        }
        let (_, exported) =
            widgets::tool_menu(ui, icon::EXPORT, "Export", widgets::Look::Full, |ui| {
                export_items(ui, &ids, menus.idle())
            });
        if let Some(Some(exported)) = exported {
            event = Some(exported);
        }
        if widgets::secondary(ui, true, "Copy their names").clicked() {
            ui.ctx().copy_text(names(&ids));
        }
        if widgets::secondary(ui, true, "Clear the selection").clicked() {
            event = Some(PickedEvent::Clear);
        }
    });
    ui.add_space(8.0);
    widgets::section_title(
        ui,
        "Selected",
        Some(ids.len()),
        &format!("{} commits selected", ids.len()),
    );
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("picked-commits")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for id in ids.iter().take(LISTED) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(id.abbreviated(super::history::ID_DIGITS).to_string())
                            .monospace()
                            .color(shared.palette.muted),
                    );
                    let subject = summaries
                        .peek(id)
                        .map_or_else(String::new, |summary| summary.subject.to_string());
                    ui.add(Label::new(subject).truncate());
                });
            }
            if ids.len() > LISTED {
                ui.label(muted(
                    ui,
                    format!("and {} more", ids.len().saturating_sub(LISTED)),
                ));
            }
        });
    event
}

/// The clicks that pick, as this platform's keys name them.
pub(super) const fn clicks() -> (&'static str, &'static str) {
    if cfg!(target_os = "macos") {
        ("⇧-click", "⌘-click")
    } else {
        ("Shift-click", "Ctrl-click")
    }
}

/// Commits' full names, a line each, the newest first.
pub(super) fn names(ids: &[ObjectId]) -> String {
    let mut text = String::with_capacity(ids.len().saturating_mul(41));
    for id in ids {
        text.push_str(&id.to_string());
        text.push('\n');
    }
    text
}

/// What exporting `ids` offers (`allowed`: a task can start), in the menu and the panel's
/// Export: their changes as a single patch, or separate patches (each saved or copied), and a
/// changelog copied.
fn export_items(ui: &mut Ui, ids: &[ObjectId], allowed: Result<(), &str>) -> Option<PickedEvent> {
    use super::actions::{item, item_of, submenu};
    let mut chosen = None;
    let ask = |how| Some(PickedEvent::Export(ids.to_vec(), how));
    submenu(ui, (icon::FILE_TEXT, "As a single patch"), |ui| {
        if item_of(
            ui,
            (icon::FLOPPY_DISK, "Save to a file…"),
            allowed,
            "export-one-patch",
        ) {
            chosen = ask(Exporting::SaveOne);
        }
        if item_of(
            ui,
            (icon::COPY, "Copy to the clipboard"),
            allowed,
            "export-one-patch",
        ) {
            chosen = ask(Exporting::CopyOne);
        }
    });
    submenu(ui, (icon::FILES, "As separate patches"), |ui| {
        if item_of(
            ui,
            (icon::FLOPPY_DISK, "Save to a folder…"),
            allowed,
            "export-patches",
        ) {
            chosen = ask(Exporting::SaveEach);
        }
        if item_of(
            ui,
            (icon::COPY, "Copy to the clipboard"),
            allowed,
            "export-patches",
        ) {
            chosen = ask(Exporting::CopyEach);
        }
    });
    if item(ui, (icon::LIST_BULLETS, "Copy as a changelog"), allowed) {
        chosen = ask(Exporting::Changelog);
    }
    chosen
}

/// The menu of a right click on one of several commits picked: what can be done to them all.
pub(super) fn menu(ui: &mut Ui, menus: &Menus<'_>, ids: &[ObjectId]) -> Option<PickedEvent> {
    let mut event = None;
    super::actions::menu_header(ui, &format!("Bulk actions on {} commits", ids.len()));
    if super::actions::item(
        ui,
        (icon::SEAL_CHECK, "Sign these commits…"),
        signing_allowed(menus, ids.len()),
    ) {
        event = Some(PickedEvent::Sign(ids.to_vec()));
    }
    let compared = two(ids);
    if super::actions::item(
        ui,
        (icon::ARROWS_LEFT_RIGHT, "Compare these two"),
        compared.map(drop),
    ) && let Ok((from, to)) = compared
    {
        event = Some(PickedEvent::Compare { from, to });
    }
    ui.separator();
    if let Some(exported) = export_items(ui, ids, menus.idle()) {
        event = Some(exported);
    }
    ui.separator();
    if super::actions::item(ui, (icon::COPY, "Copy their names"), Ok(())) {
        ui.ctx().copy_text(names(ids));
    }
    if super::actions::item(ui, (icon::X, "Clear the selection"), Ok(())) {
        event = Some(PickedEvent::Clear);
    }
    event
}
