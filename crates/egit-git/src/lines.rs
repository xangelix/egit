//! Splitting git's streamed output into lines (or NUL-ended records) without copying them.

use std::ops::ControlFlow;

use bstr::ByteSlice as _;

/// Splits a stream into lines across chunk boundaries, copying only a line split between two
/// chunks.
#[derive(Debug)]
pub(crate) struct Lines {
    partial: Vec<u8>,
    /// What ends each: a newline, or (for `-z` output) a NUL.
    end: u8,
}

impl Default for Lines {
    fn default() -> Self {
        Self::ended_by(b'\n')
    }
}

impl Lines {
    /// Records each ended by `end`.
    pub(crate) const fn ended_by(end: u8) -> Self {
        Self {
            partial: Vec::new(),
            end,
        }
    }

    pub(crate) fn feed(
        &mut self,
        mut chunk: &[u8],
        handle: &mut impl FnMut(&[u8]) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        if !self.partial.is_empty() {
            let Some(end) = chunk.find_byte(self.end) else {
                self.partial.extend_from_slice(chunk);
                return ControlFlow::Continue(());
            };
            self.partial
                .extend_from_slice(chunk.get(..end).unwrap_or_default());
            let flow = handle(&self.partial);
            self.partial.clear();
            flow?;
            chunk = chunk.get(end.saturating_add(1)..).unwrap_or_default();
        }
        while let Some(end) = chunk.find_byte(self.end) {
            handle(chunk.get(..end).unwrap_or_default())?;
            chunk = chunk.get(end.saturating_add(1)..).unwrap_or_default();
        }
        self.partial.extend_from_slice(chunk);
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut lines = Lines::default();
        let mut found = Vec::new();
        for chunk in chunks {
            let _ = lines.feed(chunk, &mut |line| {
                found.push(String::from_utf8(line.to_vec()).unwrap());
                ControlFlow::Continue(())
            });
        }
        found
    }

    #[test]
    fn lines_survive_any_chunking() {
        assert_eq!(lines_of(&[b"a b\nc d\n"]), ["a b", "c d"]);
        assert_eq!(lines_of(&[b"a ", b"b\nc", b" d", b"\n"]), ["a b", "c d"]);
        assert_eq!(lines_of(&[b"", b"one\n", b"", b"two\n"]), ["one", "two"]);
        assert_eq!(lines_of(&[b"no end"]), Vec::<String>::new());
        let mut records = Lines::ended_by(0);
        let mut found = Vec::new();
        for chunk in [&b"a\nb\0c"[..], b"\0"] {
            let _ = records.feed(chunk, &mut |record| {
                found.push(record.to_vec());
                ControlFlow::Continue(())
            });
        }
        assert_eq!(found, [b"a\nb".to_vec(), b"c".to_vec()]);
    }
}
