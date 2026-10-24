//! Finding text in a grid: the bar (⌘F), the search (a job, so a text of any size can be
//! searched as the query is typed), the matches on screen, and moving between them.
//!
//! A query with a capital matches case; one without matches ASCII letters in either case.

use std::ops::Range;
use std::sync::Arc;

use egit_text::{Text, find_all, matches_case};
use egui::{Align, Frame, Key, Layout, Margin, Rect, RichText, Stroke, Ui, UiBuilder};
use egui_async::Bind;

use super::GridSource;
use crate::format::count;
use crate::jobs;
use crate::problem::Problem;
use crate::theme::Palette;

/// The most matches found in one pane.
const MAX_MATCHES: usize = 100_000;

/// The bar's width.
const BAR_WIDTH: f32 = 470.0;

/// What a search found: each pane's matches (byte offsets into its text).
#[derive(Debug)]
struct Found {
    panes: Vec<Vec<u32>>,
    /// The query's length, in bytes.
    len: u32,
    /// Whether a pane had more matches than were kept.
    capped: bool,
}

/// Which way to move between matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Next,
    Previous,
}

/// A grid's find bar and what it found.
#[derive(Debug)]
pub(super) struct Find {
    open: bool,
    /// Whether the field takes focus next frame.
    focus: bool,
    query: String,
    found: Bind<Arc<Found>, Problem>,
    /// The query and texts `found` is for (a search is due when they change).
    searched: Option<(String, Vec<Arc<[u8]>>)>,
    /// The match moved to last: its pane and its index there.
    current: Option<(usize, usize)>,
}

impl Default for Find {
    fn default() -> Self {
        Self {
            open: false,
            focus: false,
            query: String::new(),
            found: Bind::new(true),
            searched: None,
            current: None,
        }
    }
}

impl Find {
    /// Whether the bar is open.
    pub(super) const fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the bar, its field focused.
    pub(super) const fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    /// Closes the bar; nothing is marked any more.
    pub(super) const fn close(&mut self) {
        self.open = false;
        self.current = None;
    }

    /// Searches `panes` again when the query or their texts changed.
    pub(super) fn search(&mut self, panes: &[&dyn GridSource]) {
        let texts: Vec<Arc<[u8]>> = panes.iter().map(|pane| pane.text().shared()).collect();
        let same = self.searched.as_ref().is_some_and(|(query, searched)| {
            *query == self.query
                && searched.len() == texts.len()
                && searched.iter().zip(&texts).all(|(a, b)| Arc::ptr_eq(a, b))
        });
        if same {
            return;
        }
        self.searched = Some((self.query.clone(), texts.clone()));
        self.current = None;
        let query = self.query.clone().into_bytes();
        let fold = !matches_case(&self.query);
        self.found.request(jobs::blocking(move || {
            let mut capped = false;
            let panes = texts
                .iter()
                .map(|text| {
                    let found =
                        find_all(text, &query, fold, MAX_MATCHES, || false).unwrap_or_default();
                    capped |= found.len() >= MAX_MATCHES;
                    found
                })
                .collect();
            Ok(Arc::new(Found {
                panes,
                len: u32::try_from(query.len()).unwrap_or(u32::MAX),
                capped,
            }))
        }));
    }

    /// The matches in line `line` of pane `pane` (`text`), as byte ranges in the line, the one
    /// moved to marked.
    pub(super) fn in_line(
        &mut self,
        pane: usize,
        text: &Text,
        line: u32,
        out: &mut Vec<(Range<u32>, bool)>,
    ) {
        if !self.open {
            return;
        }
        let current = self.current;
        let Some(Ok(found)) = self.found.read_as_ref() else {
            return;
        };
        let (Some(matches), Some(start), Some(bytes)) =
            (found.panes.get(pane), text.start(line), text.line(line))
        else {
            return;
        };
        let end = start.saturating_add(u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        let first = matches.partition_point(|&offset| offset < start);
        for (index, &offset) in matches.iter().enumerate().skip(first) {
            if offset >= end {
                break;
            }
            let from = offset.saturating_sub(start);
            out.push((
                from..from.saturating_add(found.len),
                current == Some((pane, index)),
            ));
        }
    }

    /// Moves to the next or previous match (pane by pane, wrapping around); returns its pane
    /// and byte offset, to scroll it into view.
    pub(super) fn step(&mut self, step: Step) -> Option<(usize, u32)> {
        let Some(Ok(found)) = self.found.read_as_ref() else {
            return None;
        };
        let all: Vec<(usize, usize)> = found
            .panes
            .iter()
            .enumerate()
            .flat_map(|(pane, matches)| (0..matches.len()).map(move |index| (pane, index)))
            .collect();
        if all.is_empty() {
            return None;
        }
        let at = self
            .current
            .and_then(|current| all.iter().position(|spot| *spot == current));
        let next = match (step, at) {
            (Step::Next, Some(at)) => at.saturating_add(1).checked_rem(all.len()).unwrap_or(0),
            (Step::Next, None) => 0,
            (Step::Previous, Some(0) | None) => all.len().saturating_sub(1),
            (Step::Previous, Some(at)) => at.saturating_sub(1),
        };
        let (pane, index) = *all.get(next)?;
        self.current = Some((pane, index));
        let offset = *found.panes.get(pane)?.get(index)?;
        Some((pane, offset))
    }

    /// Where things stand: "3 of 120", "No matches", or "Searching…".
    fn status(&mut self) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        let current = self.current;
        match self.found.read_as_ref() {
            None => "Searching…".to_owned(),
            Some(Err(_)) => "Couldn't search".to_owned(),
            Some(Ok(found)) => {
                let total: usize = found.panes.iter().map(Vec::len).sum();
                let total_text = format!(
                    "{}{}",
                    count(u64::try_from(total).unwrap_or(u64::MAX)),
                    if found.capped { "+" } else { "" }
                );
                match (total, current) {
                    (0, _) => "No matches".to_owned(),
                    (_, Some((pane, index))) => {
                        let before: usize = found.panes.iter().take(pane).map(Vec::len).sum();
                        let place = before.saturating_add(index).saturating_add(1);
                        format!(
                            "{} of {total_text}",
                            count(u64::try_from(place).unwrap_or(0))
                        )
                    }
                    (_, None) => format!("{total_text} found"),
                }
            }
        }
    }

    /// Draws the bar at the top right of `area`; returns a step asked for. Closes the bar on
    /// Escape or Close.
    pub(super) fn bar(&mut self, ui: &mut Ui, area: Rect, palette: Palette) -> Option<Step> {
        let mut step = None;
        let width = BAR_WIDTH.min(area.width());
        let rect = Rect::from_min_size(
            egui::pos2(area.right() - width - 8.0, area.top() + 6.0),
            egui::vec2(width, 40.0),
        );
        let status = self.status();
        let mut close = false;
        // Against the grid's right edge, its parts in places of their own width: nothing in it
        // moves as what it says changes.
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Min)),
            |ui| {
                Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_BOX))
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let label = ui.label(RichText::new("Find").color(palette.muted));
                            crate::help::note_topic(ui, label.rect, "find-in-file");
                            let field = ui
                                .add(
                                    crate::widgets::line_field(&mut self.query)
                                        .desired_width(150.0),
                                )
                                .labelled_by(label.id);
                            if std::mem::take(&mut self.focus) {
                                field.request_focus();
                            }
                            if field.lost_focus() {
                                if ui.input(|input| input.key_pressed(Key::Escape)) {
                                    close = true;
                                } else if ui.input(|input| input.key_pressed(Key::Enter)) {
                                    let back = ui.input(|input| input.modifiers.shift);
                                    step = Some(if back { Step::Previous } else { Step::Next });
                                    self.focus = true;
                                }
                            }
                            let room = crate::widgets::text_width(ui, "Searching…")
                                .max(crate::widgets::text_width(ui, "99,999 of 99,999"));
                            crate::widgets::column(
                                ui,
                                (room, crate::theme::CONTROL),
                                Align::Max,
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(status).color(palette.muted),
                                        )
                                        .selectable(false)
                                        .truncate(),
                                    );
                                },
                            );
                            let icon = crate::theme::icon::ARROW_UP;
                            if crate::widgets::icon_tool(ui, true, icon, "Previous")
                                .on_hover_text("Previous (Shift+Enter)")
                                .clicked()
                            {
                                step = Some(Step::Previous);
                            }
                            let icon = crate::theme::icon::ARROW_DOWN;
                            if crate::widgets::icon_tool(ui, true, icon, "Next")
                                .on_hover_text("Next (Enter)")
                                .clicked()
                            {
                                step = Some(Step::Next);
                            }
                            if crate::widgets::icon_tool(ui, true, crate::theme::icon::X, "Close")
                                .on_hover_text("Close (Escape)")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            },
        );
        if close {
            self.close();
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn searched(found: Vec<Vec<u32>>) -> Find {
        let mut find = Find {
            open: true,
            query: "ab".to_owned(),
            ..Find::default()
        };
        find.found.fill(Ok(Arc::new(Found {
            panes: found,
            len: 2,
            capped: false,
        })));
        find
    }

    #[test]
    fn steps_through_each_pane_in_turn_and_wraps() {
        let mut find = searched(vec![vec![0, 10], vec![4]]);
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(find.step(Step::Next));
        }
        assert_eq!(
            seen,
            [Some((0, 0)), Some((0, 10)), Some((1, 4)), Some((0, 0))]
        );
        assert_eq!(find.status(), "1 of 3");
        assert_eq!(find.step(Step::Previous), Some((1, 4)));
        assert_eq!(find.status(), "3 of 3");
    }

    #[test]
    fn marks_a_lines_matches_and_the_one_moved_to() {
        let text = Text::new(b"ab ab\nxx ab\n".to_vec()).unwrap_or_default();
        let mut find = searched(vec![vec![0, 3, 9]]);
        let _ = find.step(Step::Next);
        let mut marks = Vec::new();
        find.in_line(0, &text, 0, &mut marks);
        assert_eq!(marks, [(0..2, true), (3..5, false)]);
        marks.clear();
        find.in_line(0, &text, 1, &mut marks);
        assert_eq!(marks, [(3..5, false)]);
    }

    #[test]
    fn says_how_many_it_found() {
        let mut find = searched(vec![Vec::new()]);
        assert_eq!(find.status(), "No matches");
        let mut find = searched(vec![vec![1, 2], vec![3]]);
        assert_eq!(find.status(), "3 found");
        find.query.clear();
        assert_eq!(find.status(), "");
    }
}
