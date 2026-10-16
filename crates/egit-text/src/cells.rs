//! A line as cells: what each character shows, and how many cells it takes.
//!
//! Printable ASCII takes one cell; East Asian wide characters and emoji take two (Unicode's
//! East Asian Width, ambiguous characters narrow); combining marks take none and draw over the
//! character before them. Nothing is hidden and nothing reorders: a tab takes the cells up to its
//! next stop, C0 controls show as `^X`, bytes that aren't UTF-8 as `<FF>`, and characters that
//! are invisible or change the text's direction (zero-width spaces, bidirectional controls, byte
//! order marks: the stuff of spoofed diffs) as `<200B>`. Only a carriage return ending a line
//! may be hidden.

use std::fmt::Write as _;

use unicode_width::UnicodeWidthChar as _;

/// How lines are laid out in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Cells between tab stops (at least 1).
    pub tab_width: u8,
    /// Whether a `\r` ending a line (CRLF line endings) is hidden rather than shown as `^M`.
    pub hide_trailing_cr: bool,
}

impl Default for Options {
    /// Tab stops every 8 cells, as git and `less` show them, and CRLF line endings hidden.
    fn default() -> Self {
        Self {
            tab_width: 8,
            hide_trailing_cr: true,
        }
    }
}

/// What a character shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// The character itself.
    Char(char),
    /// A tab: blank cells up to the next stop.
    Tab,
    /// A C0 control character or DEL, shown as `^X` (`^@` for NUL, `^?` for DEL).
    Control(u8),
    /// An invisible or direction-changing character, or a C1 control, shown as its code
    /// point in hexadecimal: `<200B>`.
    Escape(char),
    /// A byte that isn't part of valid UTF-8, shown in hexadecimal: `<FF>`.
    Invalid(u8),
    /// A carriage return ending the line, not shown (see [`Options::hide_trailing_cr`]).
    Hidden,
}

impl Glyph {
    /// Writes what it shows into `out` (a tab as `width` spaces).
    pub fn write(self, width: u32, out: &mut String) {
        match self {
            Self::Char(c) => out.push(c),
            Self::Tab => {
                for _ in 0..width {
                    out.push(' ');
                }
            }
            Self::Control(byte) => {
                out.push('^');
                out.push(char::from(byte ^ 0x40));
            }
            Self::Escape(c) => {
                // Writing into a `String` can't fail.
                let _ = write!(out, "<{:04X}>", u32::from(c));
            }
            Self::Invalid(byte) => {
                let _ = write!(out, "<{byte:02X}>");
            }
            Self::Hidden => {}
        }
    }

    /// Whether it is shown as something other than itself (an escape, a control, a bad byte),
    /// which is styled apart.
    #[must_use]
    pub const fn is_escaped(self) -> bool {
        matches!(self, Self::Control(_) | Self::Escape(_) | Self::Invalid(_))
    }
}

/// One character of a line, placed on the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Where it starts in the line.
    pub byte: u32,
    /// How many bytes it takes (1 to 4).
    pub len: u8,
    /// The first cell it takes, counted from the start of the line.
    pub cell: u32,
    /// How many cells it takes (0 for a combining mark, which draws over the cell before).
    pub width: u32,
    /// What it shows.
    pub glyph: Glyph,
}

impl Cell {
    /// The cell after its last.
    #[must_use]
    pub const fn end(&self) -> u32 {
        self.cell.saturating_add(self.width)
    }
}

/// The cells of a line, from its start or from a character found by
/// [`Measure::seek`](crate::Measure::seek).
#[derive(Clone, Debug)]
pub struct Cells<'a> {
    line: &'a [u8],
    byte: usize,
    cell: u32,
    options: Options,
}

impl<'a> Cells<'a> {
    /// The cells of `line` (without its `\n`).
    #[must_use]
    pub fn new(line: &'a [u8], options: Options) -> Self {
        Self::resume(line, 0, 0, options)
    }

    /// The cells of `line` from the character starting at `byte`, which takes cell `cell` on.
    #[must_use]
    pub fn resume(line: &'a [u8], byte: u32, cell: u32, options: Options) -> Self {
        Self {
            line,
            byte: usize::try_from(byte).unwrap_or(usize::MAX),
            cell,
            options,
        }
    }
}

impl Iterator for Cells<'_> {
    type Item = Cell;

    fn next(&mut self) -> Option<Cell> {
        let rest = self.line.get(self.byte..).filter(|rest| !rest.is_empty())?;
        let (glyph, len) = decode(rest, self.options);
        let width = glyph_width(glyph, self.cell, self.options);
        let cell = Cell {
            byte: u32::try_from(self.byte).unwrap_or(u32::MAX),
            len,
            cell: self.cell,
            width,
            glyph,
        };
        self.byte = self.byte.saturating_add(usize::from(len));
        self.cell = self.cell.saturating_add(width);
        Some(cell)
    }
}

/// The first character of `rest` (a line's tail), and how many bytes it takes.
fn decode(rest: &[u8], options: Options) -> (Glyph, u8) {
    let Some(&first) = rest.first() else {
        return (Glyph::Hidden, 1);
    };
    if first < 0x80 {
        let glyph = match first {
            0x20..=0x7e => Glyph::Char(char::from(first)),
            b'\t' => Glyph::Tab,
            b'\r' if rest.len() == 1 && options.hide_trailing_cr => Glyph::Hidden,
            _ => Glyph::Control(first),
        };
        return (glyph, 1);
    }
    let len: u8 = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (Glyph::Invalid(first), 1),
    };
    let decoded = rest
        .get(..usize::from(len))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.chars().next());
    match decoded {
        Some(c) if is_escaped(c) => (Glyph::Escape(c), len),
        Some(c) => (Glyph::Char(c), len),
        None => (Glyph::Invalid(first), 1),
    }
}

/// How many cells `glyph` takes when it starts at `cell`.
fn glyph_width(glyph: Glyph, cell: u32, options: Options) -> u32 {
    match glyph {
        Glyph::Char(c) => u32::try_from(c.width().unwrap_or(0)).unwrap_or(1),
        Glyph::Tab => {
            let tab = u32::from(options.tab_width.max(1));
            tab.saturating_sub(cell.checked_rem(tab).unwrap_or(0))
        }
        Glyph::Control(_) => 2,
        Glyph::Escape(c) => {
            if u32::from(c) > 0xffff {
                8
            } else {
                6
            }
        }
        Glyph::Invalid(_) => 4,
        Glyph::Hidden => 0,
    }
}

/// Whether `c` is shown as an escape: C1 controls, and characters that are invisible or change
/// the text's direction, which could make a diff read other than it is.
const fn is_escaped(c: char) -> bool {
    matches!(
        c,
        '\u{80}'..='\u{9f}'
            // Soft hyphen, Arabic letter mark, Mongolian vowel separator.
            | '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            // Zero-width space, left-to-right and right-to-left marks.
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            // Line and paragraph separators, directional embeddings and overrides.
            | '\u{2028}'..='\u{202e}'
            // Word joiner, directional isolates.
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            // Hangul fillers, which show as nothing.
            | '\u{115f}'
            | '\u{1160}'
            | '\u{3164}'
            | '\u{ffa0}'
            // Byte order mark (zero-width no-break space).
            | '\u{feff}'
    )
}

/// How many cells `line` takes, quickly when it is printable ASCII.
#[must_use]
pub fn line_cells(line: &[u8], options: Options) -> u32 {
    let body = match line.split_last() {
        Some((b'\r', body)) if options.hide_trailing_cr => body,
        _ => line,
    };
    if body.iter().all(|byte| matches!(byte, 0x20..=0x7e)) {
        return u32::try_from(body.len()).unwrap_or(u32::MAX);
    }
    Cells::new(line, options)
        .last()
        .map_or(0, |last| last.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(line: &[u8]) -> (String, Vec<u32>) {
        let mut text = String::new();
        let mut widths = Vec::new();
        for cell in Cells::new(line, Options::default()) {
            cell.glyph.write(cell.width, &mut text);
            widths.push(cell.width);
        }
        (text, widths)
    }

    #[test]
    fn ascii_takes_a_cell_each() {
        assert_eq!(shown(b"fn main()"), ("fn main()".to_owned(), vec![1; 9]));
        assert_eq!(line_cells(b"fn main()", Options::default()), 9);
    }

    #[test]
    fn tabs_reach_the_next_stop() {
        let (text, widths) = shown(b"a\tbc\td");
        assert_eq!(text, "a       bc      d");
        assert_eq!(widths, [1, 7, 1, 1, 6, 1]);
        let narrow = Options {
            tab_width: 4,
            ..Options::default()
        };
        assert_eq!(line_cells(b"\t\tx", narrow), 9);
    }

    #[test]
    fn controls_and_bad_bytes_are_spelled_out() {
        let (text, widths) = shown(b"a\x00b\x7f\x1b[0m\xff\xc3");
        assert_eq!(text, "a^@b^?^[[0m<FF><C3>");
        assert_eq!(widths, [1, 2, 1, 2, 2, 1, 1, 1, 4, 4]);
    }

    #[test]
    fn a_carriage_return_ending_the_line_hides_but_not_elsewhere() {
        assert_eq!(shown(b"ab\r"), ("ab".to_owned(), vec![1, 1, 0]));
        assert_eq!(shown(b"a\rb").0, "a^Mb");
        assert_eq!(line_cells(b"ab\r", Options::default()), 2);
        let shown_cr = Options {
            hide_trailing_cr: false,
            ..Options::default()
        };
        assert_eq!(line_cells(b"ab\r", shown_cr), 4);
    }

    #[test]
    fn wide_characters_take_two_cells_and_marks_none() {
        let (text, widths) = shown("漢字 e\u{301} 🦀".as_bytes());
        assert_eq!(text, "漢字 e\u{301} 🦀");
        assert_eq!(widths, [2, 2, 1, 1, 0, 1, 2]);
        assert_eq!(
            line_cells("漢字 e\u{301} 🦀".as_bytes(), Options::default()),
            9
        );
    }

    #[test]
    fn invisible_and_direction_changing_characters_are_escaped() {
        let (text, widths) = shown("a\u{200b}b\u{202e}c\u{feff}\u{85}".as_bytes());
        assert_eq!(text, "a<200B>b<202E>c<FEFF><0085>");
        assert_eq!(widths, [1, 6, 1, 6, 1, 6, 6]);
    }

    #[test]
    fn cells_resume_mid_line() {
        let line = "a\t漢b".as_bytes();
        let all: Vec<Cell> = Cells::new(line, Options::default()).collect();
        let from: Vec<Cell> =
            Cells::resume(line, all[2].byte, all[2].cell, Options::default()).collect();
        assert_eq!(from, all[2..]);
        assert_eq!(all[2].cell, 8);
    }
}
