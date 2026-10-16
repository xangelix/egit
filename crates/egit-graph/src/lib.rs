//! The commit graph's lanes: laid out in one pass over a history, drawn for any range of rows.
//!
//! [`GraphBuilder`] lays out commits as they arrive, children before parents
//! (`git rev-list --topo-order`). Each commit gets a [`Row`]: the lane its node sits in, the
//! lane's colour, and the [`Edge`]s where other lanes meet it (lanes of other children ending
//! there, lanes starting toward its merge parents). Rows are kept in [`Chunk`]s of
//! [`CHUNK_ROWS`], each starting with the lanes open at its first row. [`for_each_row`] draws any
//! range of rows by replaying at most one chunk, so a graph costs about 12 bytes a commit rather
//! than every lane of every row, and [`RowView::segments`] says which lines a row shows.
//!
//! Nothing here knows git: commits are any `Copy + Eq + Hash` key.

mod draw;
mod layout;

pub use draw::{Level, Point, RowView, Segment, for_each_row};
pub use layout::{CHUNK_ROWS, Chunk, Edge, EdgeKind, FREE, GraphBuilder, Row};

#[cfg(test)]
mod tests;
