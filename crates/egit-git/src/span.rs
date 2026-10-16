//! Fields of git's NUL-separated output kept as byte ranges of it, so a parsed listing is one
//! buffer and a table of small ranges rather than a string per field.

use bstr::ByteSlice as _;

/// A byte range of a buffer under 4 GiB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

/// The bytes `span` covers in `buffer` (none, should it lie outside).
pub(crate) fn span_bytes(buffer: &[u8], span: Span) -> &[u8] {
    let index = |position: u32| usize::try_from(position).unwrap_or(usize::MAX);
    buffer
        .get(index(span.start)..index(span.end))
        .unwrap_or_default()
}

/// A position known to fit: the whole buffer was checked to be under 4 GiB.
pub(crate) fn as_u32(position: usize) -> u32 {
    u32::try_from(position).unwrap_or(u32::MAX)
}

/// Splits a buffer at NUL bytes into spans; the final NUL terminates the last field.
pub(crate) struct SpanSplitter<'a> {
    buffer: &'a [u8],
    position: usize,
}

impl<'a> SpanSplitter<'a> {
    pub(crate) const fn new(buffer: &'a [u8]) -> Self {
        Self {
            buffer,
            position: 0,
        }
    }
}

impl Iterator for SpanSplitter<'_> {
    type Item = Span;

    fn next(&mut self) -> Option<Span> {
        let rest = self.buffer.get(self.position..)?;
        if rest.is_empty() {
            return None;
        }
        let length = rest.find_byte(b'\0').unwrap_or(rest.len());
        let start = self.position;
        let end = start.saturating_add(length);
        self.position = end.saturating_add(1);
        Some(Span {
            start: as_u32(start),
            end: as_u32(end),
        })
    }
}
