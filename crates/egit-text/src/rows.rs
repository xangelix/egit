//! Rows: lines as the grid shows them, one row each or wrapped.
//!
//! Wrapped at `width`, a line `n` cells long takes `ceil(n / width)` rows (at least one), and its
//! row `k` shows cells `k * width` on. A wide character starting in a row's last cell hangs one
//! cell past its end, so no row depends on the rows before it: rows are found by arithmetic,
//! and wrapping a million lines at a new width takes a millisecond.

use crate::measure::Measure;

/// Lines a block of the row index covers.
const BLOCK: u32 = 1024;

/// How lines become rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// One row a line (scrolled sideways to see the rest).
    Off,
    /// Rows this many cells wide (at least one).
    At(u32),
}

/// A place in the text: a line, and the cell a row of it starts at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// The line.
    pub line: u32,
    /// The row's first cell in the line.
    pub cell: u32,
}

impl Wrap {
    /// Rows `width` cells wide (at least one).
    #[must_use]
    pub fn at(width: u32) -> Self {
        Self::At(width.max(1))
    }

    /// How many rows a line `cells` long takes.
    #[must_use]
    pub fn rows(self, cells: u32) -> u32 {
        match self {
            Self::Off => 1,
            Self::At(width) => cells.div_ceil(width.max(1)).max(1),
        }
    }

    /// The first cell of the row holding `cell`.
    #[must_use]
    pub fn row_start(self, cell: u32) -> u32 {
        match self {
            Self::Off => 0,
            Self::At(width) => cell.saturating_sub(cell.checked_rem(width).unwrap_or(0)),
        }
    }

    /// The row after the one starting at `position`, if there is one.
    #[must_use]
    pub fn next(self, measure: &Measure, position: Position) -> Option<Position> {
        if let Self::At(width) = self {
            let cell = position.cell.saturating_add(width);
            if cell < measure.cells(position.line) {
                return Some(Position { cell, ..position });
            }
        }
        let line = position.line.checked_add(1)?;
        (line < measure.lines()).then_some(Position { line, cell: 0 })
    }

    /// The row before the one starting at `position`, if there is one.
    #[must_use]
    pub fn previous(self, measure: &Measure, position: Position) -> Option<Position> {
        if let Self::At(width) = self
            && position.cell > 0
        {
            return Some(Position {
                cell: self.row_start(position.cell.saturating_sub(width.min(position.cell))),
                ..position
            });
        }
        let line = position.line.checked_sub(1)?;
        let rows = self.rows(measure.cells(line));
        let cell = match self {
            Self::Off => 0,
            Self::At(width) => rows.saturating_sub(1).saturating_mul(width),
        };
        Some(Position { line, cell })
    }
}

/// Where every row is at one [`Wrap`]: the rows before each block of 1,024 lines. Costs 8 bytes
/// per block; built in one pass over the measure.
#[derive(Clone, Debug)]
pub struct Rows {
    wrap: Wrap,
    blocks: Vec<u64>,
    total: u64,
}

impl Rows {
    /// Indexes `measure`'s rows at `wrap`.
    #[must_use]
    pub fn new(measure: &Measure, wrap: Wrap) -> Self {
        let lines = measure.lines();
        let mut blocks = Vec::with_capacity(usize::try_from(lines.div_ceil(BLOCK)).unwrap_or(0));
        let mut total = 0_u64;
        for line in 0..lines {
            if line.checked_rem(BLOCK) == Some(0) {
                blocks.push(total);
            }
            total = total.saturating_add(u64::from(wrap.rows(measure.cells(line))));
        }
        Self {
            wrap,
            blocks,
            total,
        }
    }

    /// How lines became rows.
    #[must_use]
    pub const fn wrap(&self) -> Wrap {
        self.wrap
    }

    /// How many rows there are.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// The row (from the top) of the row holding `position`.
    #[must_use]
    pub fn row_of(&self, measure: &Measure, position: Position) -> u64 {
        let block = position.line.checked_div(BLOCK).unwrap_or(0);
        let first = block.saturating_mul(BLOCK);
        let mut row = usize::try_from(block)
            .ok()
            .and_then(|block| self.blocks.get(block))
            .copied()
            .unwrap_or(self.total);
        for line in first..position.line.min(measure.lines()) {
            row = row.saturating_add(u64::from(self.wrap.rows(measure.cells(line))));
        }
        let within = match self.wrap {
            Wrap::Off => 0,
            Wrap::At(width) => position
                .cell
                .min(measure.cells(position.line).saturating_sub(1))
                .checked_div(width)
                .unwrap_or(0),
        };
        row.saturating_add(u64::from(within))
    }

    /// The start of row `row` (the last row past the end).
    #[must_use]
    pub fn at(&self, measure: &Measure, row: u64) -> Position {
        let row = row.min(self.total.saturating_sub(1));
        let block = self
            .blocks
            .partition_point(|&start| start <= row)
            .saturating_sub(1);
        let mut start = self.blocks.get(block).copied().unwrap_or(0);
        let first = u32::try_from(block)
            .unwrap_or(u32::MAX)
            .saturating_mul(BLOCK);
        let last = first.saturating_add(BLOCK).min(measure.lines());
        for line in first..last {
            let rows = u64::from(self.wrap.rows(measure.cells(line)));
            if row < start.saturating_add(rows) {
                let within = u32::try_from(row.saturating_sub(start)).unwrap_or(u32::MAX);
                let cell = match self.wrap {
                    Wrap::Off => 0,
                    Wrap::At(width) => within.saturating_mul(width),
                };
                return Position { line, cell };
            }
            start = start.saturating_add(rows);
        }
        Position {
            line: measure.lines().saturating_sub(1),
            cell: 0,
        }
    }
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::{Options, Text};

    fn measured(bytes: &[u8]) -> Measure {
        let text = Text::new(bytes.to_vec()).unwrap();
        Measure::new(&text, Options::default(), || false).unwrap()
    }

    #[test]
    fn a_line_takes_a_row_per_width_of_cells() {
        let wrap = Wrap::at(10);
        assert_eq!(wrap.rows(0), 1);
        assert_eq!(wrap.rows(10), 1);
        assert_eq!(wrap.rows(11), 2);
        assert_eq!(Wrap::Off.rows(1_000), 1);
        assert_eq!(wrap.row_start(25), 20);
        assert_eq!(Wrap::at(0), Wrap::At(1));
    }

    /// Every row, walked from the top with `next`.
    fn walk(measure: &Measure, wrap: Wrap) -> Vec<Position> {
        let mut rows = vec![Position::default()];
        while let Some(next) = wrap.next(measure, *rows.last().unwrap()) {
            rows.push(next);
        }
        rows
    }

    /// A text of `lines` random lines, some long, some empty, some wide.
    fn random_text(lines: usize, mut seed: u64) -> Vec<u8> {
        let mut random = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let mut bytes = Vec::new();
        for _ in 0..lines {
            let len = match random(10) {
                0 => 0,
                1 => random(500),
                _ => random(60),
            };
            for _ in 0..len {
                let piece: &[u8] = match random(12) {
                    0 => "漢".as_bytes(),
                    1 => b"\t",
                    2 => "\u{301}".as_bytes(),
                    _ => b"x",
                };
                bytes.extend_from_slice(piece);
            }
            bytes.push(b'\n');
        }
        bytes
    }

    #[test]
    fn rows_are_found_both_ways_as_walking_finds_them() {
        for seed in 1..=8 {
            let measure = measured(&random_text(3_000, seed));
            for wrap in [Wrap::Off, Wrap::at(1), Wrap::at(7), Wrap::at(80)] {
                let rows = Rows::new(&measure, wrap);
                let walked = walk(&measure, wrap);
                assert_eq!(rows.total(), u64::try_from(walked.len()).unwrap());
                for (row, position) in walked.iter().enumerate().step_by(37) {
                    let row = u64::try_from(row).unwrap();
                    assert_eq!(rows.at(&measure, row), *position, "{wrap:?} row {row}");
                    assert_eq!(
                        rows.row_of(&measure, *position),
                        row,
                        "{wrap:?} {position:?}"
                    );
                }
                // And back up again, row by row.
                let mut back = vec![*walked.last().unwrap()];
                while let Some(previous) = wrap.previous(&measure, *back.last().unwrap()) {
                    back.push(previous);
                }
                back.reverse();
                assert_eq!(back, walked, "{wrap:?} walking back up");
            }
        }
    }

    #[test]
    fn rows_past_the_end_are_the_last_one() {
        let measure = measured(b"a\nbb\nccc");
        let rows = Rows::new(&measure, Wrap::at(2));
        assert_eq!(rows.total(), 4);
        assert_eq!(rows.at(&measure, 3), Position { line: 2, cell: 2 });
        assert_eq!(rows.at(&measure, 99), Position { line: 2, cell: 2 });
    }

    #[test]
    fn a_cell_inside_a_row_counts_as_that_row() {
        let measure = measured(&b"y".repeat(25));
        let rows = Rows::new(&measure, Wrap::at(10));
        assert_eq!(rows.row_of(&measure, Position { line: 0, cell: 17 }), 1);
        assert_eq!(rows.row_of(&measure, Position { line: 0, cell: 99 }), 2);
    }
}
