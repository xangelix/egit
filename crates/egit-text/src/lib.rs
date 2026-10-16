//! Text on a grid of cells, as a terminal shows it.
//!
//! A [`Text`] is bytes split into lines (any bytes: what isn't UTF-8 shows as such). A
//! [`Measure`] counts each line's cells once, in the background, after which nothing needs
//! reading a line from its start: [`Measure::seek`] finds any cell from checkpoints, and
//! [`Rows`] wraps every line at a width by arithmetic. [`Cells`] says what a line's characters
//! show (see its module for the rules: wide characters, tabs, controls, escapes).
//!
//! A view keeps a [`Position`] at its top (a line, and the cell its top row starts at): drawn
//! from there, a screen costs the same at line 1 as at line 1,000,000, and wrapping at another
//! width keeps the same text at the top. [`Rows`] is only needed for a scroll bar.
//!
//! [`changed_words`] finds which words of an edited line changed, for a diff to mark, and
//! [`find_all`] every place a query occurs.

mod cells;
mod find;
mod measure;
mod rows;
mod text;
mod words;

pub use cells::{Cell, Cells, Glyph, Options, line_cells};
pub use find::{find_all, matches_case};
pub use measure::{CHECKPOINT_CELLS, Found, Measure};
pub use rows::{Position, Rows, Wrap};
pub use text::{Text, TooLarge};
pub use words::{ChangedWords, changed_words};
