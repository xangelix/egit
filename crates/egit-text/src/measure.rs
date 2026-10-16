//! A text measured once: how many cells each line takes and, in long lines, where every
//! [`CHECKPOINT_CELLS`]th cell is, so any cell is found without reading its line from the start.

use crate::cells::{Cells, Options, line_cells};
use crate::text::Text;

/// Lines longer than this, in bytes, get checkpoints; shorter ones are read from their start.
const LONG_LINE: usize = 4096;

/// Cells between a long line's checkpoints.
pub const CHECKPOINT_CELLS: u32 = 4096;

/// How often a measurement asks whether to stop, in lines.
const CHECK_EVERY: u32 = 4096;

/// How many cells each line of a [`Text`] takes. Costs 4 bytes a line, and 8 bytes per
/// [`CHECKPOINT_CELLS`] cells of lines longer than 4 KiB.
#[derive(Clone, Debug)]
pub struct Measure {
    options: Options,
    cells: Vec<u32>,
    widest: u32,
    long: Vec<LongLine>,
}

/// A long line's checkpoints.
#[derive(Clone, Debug)]
struct LongLine {
    line: u32,
    /// For every [`CHECKPOINT_CELLS`]th cell, the character covering it; empty when every byte
    /// is a cell (printable ASCII), where a cell's byte is its number.
    checkpoints: Box<[Found]>,
}

/// A character in a line: where it starts, in bytes and in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Its first byte in the line.
    pub byte: u32,
    /// Its first cell in the line.
    pub cell: u32,
}

impl Measure {
    /// Measures every line of `text`, asking `stop` every few thousand lines whether to give up
    /// (`None`). Printable ASCII is measured by its length; Linux's `MAINTAINERS` takes a few
    /// milliseconds.
    pub fn new(text: &Text, options: Options, mut stop: impl FnMut() -> bool) -> Option<Self> {
        Self::checked(text, options, || if stop() { Err(()) } else { Ok(()) }).ok()
    }

    /// Measures every line of `text`, however long it takes (on a job's thread, for a large one).
    #[must_use]
    pub fn all(text: &Text, options: Options) -> Self {
        match Self::checked::<std::convert::Infallible>(text, options, || Ok(())) {
            Ok(measure) => measure,
            Err(never) => match never {},
        }
    }

    /// Measures every line, `check`ing every few thousand lines whether to go on.
    fn checked<E>(
        text: &Text,
        options: Options,
        mut check: impl FnMut() -> Result<(), E>,
    ) -> Result<Self, E> {
        let lines = text.lines();
        let mut cells = Vec::with_capacity(usize::try_from(lines).unwrap_or(0));
        let mut widest = 0;
        let mut long = Vec::new();
        for index in 0..lines {
            if index.checked_rem(CHECK_EVERY) == Some(0) {
                check()?;
            }
            let line = text.line(index).unwrap_or_default();
            let width = if line.len() > LONG_LINE {
                let (width, checkpoints) = measure_long(line, options);
                long.push(LongLine {
                    line: index,
                    checkpoints,
                });
                width
            } else {
                line_cells(line, options)
            };
            widest = widest.max(width);
            cells.push(width);
        }
        Ok(Self {
            options,
            cells,
            widest,
            long,
        })
    }

    /// Gives each line at least as many cells as `other`'s line of the same number, so that
    /// wrapped at one width, both texts' lines take the same rows (two sides of a diff shown
    /// side by side). The rows past a line's own cells are empty; lines past `other`'s last are
    /// left as they are.
    pub fn match_rows(&mut self, other: &Self) {
        for (cells, &theirs) in self.cells.iter_mut().zip(&other.cells) {
            *cells = (*cells).max(theirs);
        }
        self.widest = self.widest.max(other.widest);
    }

    /// How lines were laid out.
    #[must_use]
    pub const fn options(&self) -> Options {
        self.options
    }

    /// How many lines were measured.
    #[must_use]
    pub fn lines(&self) -> u32 {
        u32::try_from(self.cells.len()).unwrap_or(u32::MAX)
    }

    /// How many cells line `line` takes (0 past the end).
    #[must_use]
    pub fn cells(&self, line: u32) -> u32 {
        usize::try_from(line)
            .ok()
            .and_then(|line| self.cells.get(line))
            .copied()
            .unwrap_or(0)
    }

    /// The most cells a line takes.
    #[must_use]
    pub const fn widest(&self) -> u32 {
        self.widest
    }

    /// The character covering cell `cell` of line `line` (a combining mark belongs to the
    /// character before it), or the line's end past its last cell. Reads at most
    /// [`CHECKPOINT_CELLS`] cells (or 4 KiB) of the line.
    #[must_use]
    pub fn seek(&self, text: &Text, line: u32, cell: u32) -> Found {
        let Some(bytes) = text.line(line) else {
            return Found::default();
        };
        let mut from = Found::default();
        if let Ok(index) = self.long.binary_search_by_key(&line, |long| long.line)
            && let Some(long) = self.long.get(index)
        {
            if long.checkpoints.is_empty() {
                let at = cell.min(self.cells(line));
                return Found { byte: at, cell: at };
            }
            let checkpoint = usize::try_from(cell.checked_div(CHECKPOINT_CELLS).unwrap_or(0))
                .unwrap_or(usize::MAX);
            if let Some(found) = long
                .checkpoints
                .get(checkpoint)
                .or_else(|| long.checkpoints.last())
            {
                from = *found;
            }
        }
        Cells::resume(bytes, from.byte, from.cell, self.options)
            .find(|found| found.width > 0 && found.end() > cell)
            .map_or_else(
                || Found {
                    byte: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                    cell: self.cells(line),
                },
                |found| Found {
                    byte: found.byte,
                    cell: found.cell,
                },
            )
    }
}

/// A long line's width and checkpoints (none when it is printable ASCII).
fn measure_long(line: &[u8], options: Options) -> (u32, Box<[Found]>) {
    let body = match line.split_last() {
        Some((b'\r', body)) if options.hide_trailing_cr => body,
        _ => line,
    };
    if body.iter().all(|byte| matches!(byte, 0x20..=0x7e)) {
        return (
            u32::try_from(body.len()).unwrap_or(u32::MAX),
            Box::default(),
        );
    }
    let mut checkpoints = Vec::new();
    let mut next = 0_u32;
    let mut width = 0;
    for cell in Cells::new(line, options) {
        while cell.width > 0 && cell.end() > next {
            checkpoints.push(Found {
                byte: cell.byte,
                cell: cell.cell,
            });
            next = next.saturating_add(CHECKPOINT_CELLS);
        }
        width = cell.end();
    }
    (width, checkpoints.into_boxed_slice())
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn measured(bytes: &[u8]) -> (Text, Measure) {
        let text = Text::new(bytes.to_vec()).unwrap();
        let measure = Measure::all(&text, Options::default());
        (text, measure)
    }

    #[test]
    fn measures_every_line() {
        let (_, measure) = measured("short\n\ttab\n漢字\n\nlast".as_bytes());
        let cells: Vec<u32> = (0..measure.lines())
            .map(|line| measure.cells(line))
            .collect();
        assert_eq!(cells, [5, 11, 4, 0, 4]);
        assert_eq!(measure.widest(), 11);
        assert_eq!(measure.cells(99), 0);
    }

    #[test]
    fn two_sides_take_the_same_rows() {
        let (left_text, mut left) = measured(b"short\n\nlonger line here\n");
        let (_, right) = measured(b"a much longer line\nmid\nx\nextra");
        left.match_rows(&right);
        let cells: Vec<u32> = (0..left.lines()).map(|line| left.cells(line)).collect();
        assert_eq!(cells, [18, 3, 16]);
        assert_eq!(left.widest(), 18);
        // Past a line's own cells, a seek finds its end.
        assert_eq!(left.seek(&left_text, 0, 10), Found { byte: 5, cell: 18 });
    }

    #[test]
    fn stops_when_asked() {
        let text = Text::new(vec![b'\n'; 10_000]).unwrap();
        let mut asked = 0;
        let stopped = Measure::new(&text, Options::default(), || {
            asked += 1;
            asked > 1
        });
        assert!(stopped.is_none());
        assert_eq!(asked, 2);
    }

    /// Seeking every cell of `bytes`' first line finds what reading the line from its start
    /// does.
    fn seeks_like_a_scan(bytes: &[u8]) {
        let (text, measure) = measured(bytes);
        let line = text.line(0).unwrap();
        let cells: Vec<_> = Cells::new(line, Options::default()).collect();
        let width = measure.cells(0);
        for target in (0..=width.saturating_add(2))
            .step_by(97)
            .chain([0, 1, width])
        {
            let expected = cells
                .iter()
                .find(|cell| cell.width > 0 && cell.end() > target)
                .map_or_else(
                    || Found {
                        byte: u32::try_from(line.len()).unwrap(),
                        cell: width,
                    },
                    |cell| Found {
                        byte: cell.byte,
                        cell: cell.cell,
                    },
                );
            assert_eq!(measure.seek(&text, 0, target), expected, "cell {target}");
        }
    }

    #[test]
    fn long_lines_are_sought_from_checkpoints() {
        // Plain ASCII, and a mix of tabs, wide characters, marks, and bad bytes.
        seeks_like_a_scan(&b"0123456789".repeat(5_000));
        let mixed = "a\t漢e\u{301}\u{200b}🦀"
            .as_bytes()
            .iter()
            .copied()
            .chain([0xff]);
        let mixed: Vec<u8> = mixed.cycle().take(60_000).collect();
        seeks_like_a_scan(&mixed);
        // Short lines are read from their start.
        seeks_like_a_scan("x\t漢y".as_bytes());
    }

    #[test]
    fn a_plain_long_line_seeks_by_arithmetic() {
        let mut line = b"x".repeat(10_000);
        line.extend_from_slice(b"\r\n");
        let (text, measure) = measured(&line);
        assert_eq!(measure.cells(0), 10_000);
        assert_eq!(
            measure.seek(&text, 0, 7_777),
            Found {
                byte: 7_777,
                cell: 7_777
            }
        );
        assert_eq!(
            measure.seek(&text, 0, 20_000),
            Found {
                byte: 10_000,
                cell: 10_000
            }
        );
    }
}
