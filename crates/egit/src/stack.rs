//! A tall list of items of different heights in a scroll area of its own, drawn only where in
//! view: a conversation with an agent, however long.
//!
//! Each item's height is guessed until it is drawn (or measured ahead, unseen, a little each
//! frame), and measured again whenever it changed since (its version). Items above the view
//! that grow or shrink keep the view where it is; at the bottom it follows what is added, as a
//! terminal does.

use std::ops::Add as _;
use std::time::{Duration, Instant};

use egui::{Id, Rect, ScrollArea, Ui, UiBuilder, pos2, vec2};

/// How long a frame may spend measuring items out of view.
const MEASURING: Duration = Duration::from_millis(2);

/// An item: its version (when it changes, its height is measured again) and a guess at its
/// height until then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) version: u64,
    pub(crate) guess: f32,
}

/// A list's heights, and where to scroll.
#[derive(Debug, Default)]
pub(crate) struct Stack {
    /// Each item's height (its gap after it included), and the version it was measured at.
    heights: Vec<(f32, Option<u64>)>,
    width: f32,
    scroll_to: Option<f32>,
    /// Scroll to the end, once drawn.
    to_end: bool,
}

impl Stack {
    /// Scrolls to the end (once drawn).
    pub(crate) const fn scroll_to_end(&mut self) {
        self.to_end = true;
    }

    /// Draws the `entries` in view, each by `draw` (its index), `gap` apart and `margin` in
    /// from the scrolled content's edges (its sides, and its top and bottom), in a scroll area
    /// of its own (`id`).
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        id: Id,
        entries: &[Entry],
        (gap, margin): (f32, egui::Vec2),
        mut draw: impl FnMut(&mut Ui, usize),
    ) {
        self.heights.truncate(entries.len());
        for entry in entries.get(self.heights.len()..).unwrap_or_default() {
            self.heights.push((entry.guess + gap, None));
        }
        let mut area = ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink([false, false])
            .stick_to_bottom(true);
        if std::mem::take(&mut self.to_end) {
            // As far as it goes: the area keeps the offset within its content.
            let total: f32 = self.heights.iter().map(|(height, _)| height).sum();
            self.scroll_to = Some(2.0f32.mul_add(margin.y, total));
        }
        if let Some(offset) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(offset);
        }
        let shown = area.show_viewport(ui, |ui, viewport| {
            let width = 2.0f32.mul_add(-margin.x, ui.available_width()).max(0.0);
            if (self.width - width).abs() > 0.5 {
                // Measured at another width: guesses now, until drawn again.
                for (_, measured) in &mut self.heights {
                    *measured = None;
                }
                self.width = width;
            }
            let total: f32 = self.heights.iter().map(|(height, _)| height).sum();
            ui.set_height(2.0f32.mul_add(margin.y, total));
            let place = Place {
                origin: ui.min_rect().min.add(margin),
                width,
                gap,
            };
            // The viewport in the items' terms: from their first's top.
            let viewport = viewport.translate(vec2(0.0, -margin.y));
            let (grew, above) = self.draw_in_view(ui, (entries, viewport, place), &mut draw);
            grew + self.measure_stale(ui, (entries, above, place), &mut draw)
        });
        // Items above the view that grew or shrank: the view stays (unless it follows the end).
        let at_end = shown.state.offset.y + shown.inner_rect.height() >= shown.content_size.y - 1.0;
        if shown.inner != 0.0 && !at_end && self.scroll_to.is_none() {
            self.scroll_to = Some((shown.state.offset.y + shown.inner).max(0.0));
            ui.ctx().request_repaint();
        }
    }

    /// Draws the entries `viewport` shows; returns how much those starting above it grew, and
    /// how many lie wholly above it.
    fn draw_in_view(
        &mut self,
        ui: &mut Ui,
        (entries, viewport, place): (&[Entry], Rect, Place),
        draw: &mut impl FnMut(&mut Ui, usize),
    ) -> (f32, usize) {
        let mut grew = 0.0;
        let mut top = 0.0;
        let mut above = 0;
        for (index, entry) in entries.iter().enumerate() {
            let Some(&(height, _)) = self.heights.get(index) else {
                break;
            };
            if top > viewport.max.y {
                break;
            }
            if top + height < viewport.min.y {
                above = index.saturating_add(1);
                top += height;
                continue;
            }
            let builder = UiBuilder::new().max_rect(place.at(top)).id_salt(index);
            let drawn = ui
                .scope_builder(builder, |ui| draw(ui, index))
                .response
                .rect
                .height()
                + place.gap;
            if let Some(slot) = self.heights.get_mut(index) {
                *slot = (drawn, Some(entry.version));
            }
            if top < viewport.min.y {
                grew += drawn - height;
            }
            top += drawn;
        }
        (grew, above)
    }

    /// Measures entries out of view never measured, or changed since, as long as [`MEASURING`]
    /// allows, laid out unseen where they belong; returns how much the first `above` (wholly
    /// above the view) grew.
    fn measure_stale(
        &mut self,
        ui: &mut Ui,
        (entries, above, place): (&[Entry], usize, Place),
        draw: &mut impl FnMut(&mut Ui, usize),
    ) -> f32 {
        let started = Instant::now();
        let stale: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                self.heights
                    .get(*index)
                    .is_some_and(|(_, measured)| *measured != Some(entry.version))
            })
            .map(|(index, _)| index)
            .take(64)
            .collect();
        let mut grew = 0.0;
        for index in stale {
            if started.elapsed() > MEASURING {
                ui.ctx().request_repaint();
                break;
            }
            let top: f32 = self
                .heights
                .iter()
                .take(index)
                .map(|(height, _)| height)
                .sum();
            let builder = UiBuilder::new()
                .max_rect(place.at(top))
                .id_salt(("measuring", index))
                .sizing_pass()
                .invisible();
            let drawn = ui
                .scope_builder(builder, |ui| draw(ui, index))
                .response
                .rect
                .height()
                + place.gap;
            let version = entries.get(index).map(|entry| entry.version);
            if let Some(slot) = self.heights.get_mut(index) {
                if index < above {
                    grew += drawn - slot.0;
                }
                *slot = (drawn, version);
            }
        }
        grew
    }
}

/// Where a list's items go: its top left, its width, and the room between them.
#[derive(Clone, Copy)]
struct Place {
    origin: egui::Pos2,
    width: f32,
    gap: f32,
}

impl Place {
    /// An item's room, `top` below the list's top.
    fn at(self, top: f32) -> Rect {
        Rect::from_min_size(
            pos2(self.origin.x, self.origin.y + top),
            vec2(self.width, f32::INFINITY),
        )
    }
}
