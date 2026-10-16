//! Text split into lines: the bytes as they are, and where each line starts.

use std::fmt;
use std::sync::Arc;

/// A text too large to hold: line offsets are `u32`, so at most 4 GiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLarge {
    /// Its size, in bytes.
    pub bytes: usize,
}

impl fmt::Display for TooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes is more than egit can show (4 GiB)", self.bytes)
    }
}

impl std::error::Error for TooLarge {}

/// Bytes split into lines at `\n`. A final `\n` ends the last line rather than starting an
/// empty one; the bytes needn't be UTF-8.
///
/// Costs the bytes and 4 bytes a line.
#[derive(Clone, Debug)]
pub struct Text {
    bytes: Arc<[u8]>,
    /// Where each line starts.
    starts: Vec<u32>,
}

impl Text {
    /// Splits `bytes` into lines.
    ///
    /// # Errors
    ///
    /// [`TooLarge`] past 4 GiB.
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Result<Self, TooLarge> {
        let bytes = bytes.into();
        let len = u32::try_from(bytes.len()).map_err(|_| TooLarge { bytes: bytes.len() })?;
        // Counted first, so the index is allocated once at its size.
        let breaks = memchr::memchr_iter(b'\n', &bytes).count();
        let mut starts = Vec::with_capacity(breaks.saturating_add(1));
        if len > 0 {
            starts.push(0);
        }
        for end in memchr::memchr_iter(b'\n', &bytes) {
            let next = u32::try_from(end.saturating_add(1)).unwrap_or(len);
            if next < len {
                starts.push(next);
            }
        }
        Ok(Self { bytes, starts })
    }

    /// All of it.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// All of it, shared (for work on another thread).
    #[must_use]
    pub fn shared(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }

    /// How many lines it has.
    #[must_use]
    pub fn lines(&self) -> u32 {
        u32::try_from(self.starts.len()).unwrap_or(u32::MAX)
    }

    /// Where line `index` starts in [`Self::bytes`].
    #[must_use]
    pub fn start(&self, index: u32) -> Option<u32> {
        self.starts.get(usize::try_from(index).ok()?).copied()
    }

    /// Line `index`, without its `\n`.
    #[must_use]
    pub fn line(&self, index: u32) -> Option<&[u8]> {
        let index = usize::try_from(index).ok()?;
        let start = usize::try_from(*self.starts.get(index)?).ok()?;
        let end = match self.starts.get(index.checked_add(1)?) {
            Some(&next) => usize::try_from(next).ok()?.checked_sub(1)?,
            None if self.bytes.last() == Some(&b'\n') => self.bytes.len().checked_sub(1)?,
            None => self.bytes.len(),
        };
        self.bytes.get(start..end)
    }

    /// The line holding byte `offset` (the last line for an offset past the end).
    #[must_use]
    pub fn line_of(&self, offset: u32) -> u32 {
        let after = self.starts.partition_point(|&start| start <= offset);
        u32::try_from(after.saturating_sub(1)).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(bytes: &[u8]) -> Vec<String> {
        let text = Text::new(bytes.to_vec()).unwrap();
        (0..text.lines())
            .map(|index| String::from_utf8(text.line(index).unwrap().to_vec()).unwrap())
            .collect()
    }

    #[test]
    fn splits_at_newlines_and_a_final_one_ends_the_last_line() {
        assert_eq!(Text::new(Vec::new()).unwrap().lines(), 0);
        assert_eq!(lines(b"abc"), ["abc"]);
        assert_eq!(lines(b"abc\n"), ["abc"]);
        assert_eq!(lines(b"\n"), [""]);
        assert_eq!(lines(b"a\n\nb"), ["a", "", "b"]);
        assert_eq!(lines(b"a\r\nb\r\n"), ["a\r", "b\r"]);
    }

    #[test]
    fn finds_the_line_of_an_offset() {
        let text = Text::new(b"ab\ncd\n\nef".to_vec()).unwrap();
        assert_eq!(text.lines(), 4);
        let found: Vec<u32> = (0..10).map(|offset| text.line_of(offset)).collect();
        assert_eq!(found, [0, 0, 0, 1, 1, 1, 2, 3, 3, 3]);
        assert_eq!(text.start(3), Some(7));
        assert_eq!(text.line(4), None);
    }
}
