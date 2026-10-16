//! Drawing any range of rows: replaying a chunk's lanes up to the first row asked for, then each
//! row's lines.

use std::borrow::Borrow;
use std::ops::Range;

use crate::layout::{CHUNK_ROWS, Chunk, Edge, EdgeKind, FREE, Row};

/// Where on a row a line ends: its top edge, its middle (where nodes sit), or its bottom edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The row's top edge.
    Top,
    /// The row's middle, where the node is.
    Middle,
    /// The row's bottom edge.
    Bottom,
}

/// A point on a row: a lane, at a level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    /// The lane.
    pub lane: u16,
    /// Where on the row.
    pub level: Level,
}

/// A line to draw on a row, in a lane's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Where it starts.
    pub from: Point,
    /// Where it ends.
    pub to: Point,
    /// Its lane's colour.
    pub color: u16,
}

/// A row as it is drawn: the lanes entering it, the commit's node, and the edges meeting it.
#[derive(Clone, Copy, Debug)]
pub struct RowView<'a> {
    /// Each slot's colour entering the row, or [`FREE`].
    pub before: &'a [u16],
    /// The row.
    pub row: Row,
    /// The edges meeting its node.
    pub edges: &'a [Edge],
}

impl RowView<'_> {
    /// Whether the commit has more than one parent.
    #[must_use]
    pub fn is_merge(&self) -> bool {
        self.edges
            .iter()
            .any(|edge| matches!(edge.kind, EdgeKind::Fork | EdgeKind::Merge))
    }

    /// Appends the row's lines to `out`: lanes passing through, lanes ending at the node, and
    /// the node's lines down toward its parents.
    pub fn segments(&self, out: &mut Vec<Segment>) {
        self.segments_in(0..FREE, out);
    }

    /// Appends the row's lines that cross the lanes in `window` to `out`, as
    /// [`segments`](Self::segments) would, in the same order. A row costs only the lanes in the
    /// window and the edges meeting its node, however wide the graph.
    pub fn segments_in(&self, window: Range<u16>, out: &mut Vec<Segment>) {
        let row = self.row;
        let point = |lane, level| Point { lane, level };
        let crosses = |a: u16, b: u16| a.min(b) < window.end && a.max(b) >= window.start;
        let start = usize::from(window.start);
        let end = usize::from(window.end).min(self.before.len());
        for (lane, &color) in window
            .clone()
            .zip(self.before.get(start..end).unwrap_or_default())
        {
            if color == FREE || self.joins(lane) {
                continue;
            }
            let to = if lane == row.lane {
                point(row.lane, Level::Middle)
            } else {
                point(lane, Level::Bottom)
            };
            out.push(Segment {
                from: point(lane, Level::Top),
                to,
                color,
            });
        }
        if row.continues && crosses(row.lane, row.lane) {
            out.push(Segment {
                from: point(row.lane, Level::Middle),
                to: point(row.lane, Level::Bottom),
                color: row.color,
            });
        }
        for edge in self.edges {
            if !crosses(edge.lane, row.lane) {
                continue;
            }
            let (from, to) = match edge.kind {
                EdgeKind::Join => (point(edge.lane, Level::Top), point(row.lane, Level::Middle)),
                EdgeKind::Fork | EdgeKind::Merge => (
                    point(row.lane, Level::Middle),
                    point(edge.lane, Level::Bottom),
                ),
            };
            out.push(Segment {
                from,
                to,
                color: edge.color,
            });
        }
    }

    /// Whether `lane` ends at the node, joining its lane.
    fn joins(&self, lane: u16) -> bool {
        self.edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::Join && edge.lane == lane)
    }
}

/// Applies a row to the lanes entering it, giving the lanes entering the next.
fn advance(lanes: &mut Vec<u16>, row: Row, edges: &[Edge]) {
    let set = |lanes: &mut Vec<u16>, lane: u16, color: u16| {
        let slot = usize::from(lane);
        if let Some(existing) = lanes.get_mut(slot) {
            *existing = color;
        } else if color != FREE {
            lanes.resize(slot, FREE);
            lanes.push(color);
        }
    };
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::Join) {
        set(lanes, edge.lane, FREE);
    }
    set(
        lanes,
        row.lane,
        if row.continues { row.color } else { FREE },
    );
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::Fork) {
        set(lanes, edge.lane, edge.color);
    }
    while lanes.last() == Some(&FREE) {
        lanes.pop();
    }
}

/// Calls `each` with every row in `rows` (as far as `chunks` go) as it is drawn.
///
/// `chunks` are a graph's chunks in order, every one full but the last. The lanes entering the
/// first row are replayed from its chunk's start: at most [`CHUNK_ROWS`] rows. `lanes` is
/// scratch space, reused between calls.
pub fn for_each_row<C: Borrow<Chunk>>(
    chunks: &[C],
    rows: Range<u64>,
    lanes: &mut Vec<u16>,
    mut each: impl FnMut(u64, RowView<'_>),
) {
    let rows_per_chunk = u64::try_from(CHUNK_ROWS).unwrap_or(u64::MAX);
    let Some(first) = rows
        .start
        .checked_div(rows_per_chunk)
        .and_then(|index| usize::try_from(index).ok())
    else {
        return;
    };
    let Some(start_chunk) = chunks.get(first) else {
        return;
    };
    let start_chunk: &Chunk = start_chunk.borrow();
    lanes.clear();
    lanes.extend_from_slice(start_chunk.entry());
    let skip = usize::try_from(rows.start.saturating_sub(start_chunk.first_row())).unwrap_or(0);
    for index in 0..skip {
        if let Some((row, edges)) = start_chunk.row(index) {
            advance(lanes, row, edges);
        }
    }

    let mut number = rows.start;
    for chunk in chunks.iter().skip(first) {
        let chunk: &Chunk = chunk.borrow();
        let from = usize::try_from(number.saturating_sub(chunk.first_row())).unwrap_or(0);
        for index in from..chunk.len() {
            if number >= rows.end {
                return;
            }
            let Some((row, edges)) = chunk.row(index) else {
                return;
            };
            each(
                number,
                RowView {
                    before: lanes,
                    row,
                    edges,
                },
            );
            advance(lanes, row, edges);
            number = number.saturating_add(1);
        }
    }
}
