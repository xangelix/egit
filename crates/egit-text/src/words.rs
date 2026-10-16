//! Which words of a changed line changed: the bytes of an old and a new line outside their
//! longest common sequence of tokens, so a diff can mark them.
//!
//! A token is a run of letters, digits, `_`, and non-ASCII characters (a word or a number), a run
//! of whitespace, or one other character. Lines too long to compare quickly, and pairs sharing
//! too little to be one line edited rather than another written in its place, aren't marked.

use std::ops::Range;

/// The most tokens a line may have to be compared (the comparison takes their product).
const MAX_TOKENS: usize = 500;

/// The least share of their words two lines must have in common to be marked, in percent.
const MIN_SHARED: usize = 40;

/// The bytes that changed between an old and a new line, merged where they touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangedWords {
    /// In the old line.
    pub old: Vec<Range<u32>>,
    /// In the new line.
    pub new: Vec<Range<u32>>,
}

/// The bytes of `old` and `new` (each in its own line) that changed; `None` when the lines are
/// too long to compare or share too little to be worth marking.
#[must_use]
pub fn changed_words(old: &[u8], new: &[u8]) -> Option<ChangedWords> {
    let old_tokens = tokens(old);
    let new_tokens = tokens(new);
    if old_tokens.len() > MAX_TOKENS || new_tokens.len() > MAX_TOKENS {
        return None;
    }
    let old_text: Vec<&[u8]> = old_tokens.iter().map(|token| slice(old, token)).collect();
    let new_text: Vec<&[u8]> = new_tokens.iter().map(|token| slice(new, token)).collect();
    let (old_kept, new_kept) = common(&old_text, &new_text);

    let words = |texts: &[&[u8]]| {
        texts
            .iter()
            .filter(|text| !text.iter().all(u8::is_ascii_whitespace))
            .count()
    };
    let shared = old_text
        .iter()
        .zip(&old_kept)
        .filter(|(text, kept)| **kept && !text.iter().all(u8::is_ascii_whitespace))
        .count();
    let most = words(&old_text).max(words(&new_text));
    if most > 0 && shared.saturating_mul(100) < most.saturating_mul(MIN_SHARED) {
        return None;
    }
    Some(ChangedWords {
        old: unkept(&old_tokens, &old_kept),
        new: unkept(&new_tokens, &new_kept),
    })
}

/// The bytes of `token` in `line`.
fn slice<'a>(line: &'a [u8], token: &Range<u32>) -> &'a [u8] {
    let start = usize::try_from(token.start).unwrap_or(usize::MAX);
    let end = usize::try_from(token.end).unwrap_or(usize::MAX);
    line.get(start..end).unwrap_or_default()
}

/// Splits `line` into tokens: word runs, whitespace runs, and single other characters.
fn tokens(line: &[u8]) -> Vec<Range<u32>> {
    #[derive(PartialEq, Eq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |byte: u8| {
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 {
            Class::Word
        } else if byte.is_ascii_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut tokens = Vec::new();
    let mut start = 0_usize;
    while let Some(&first) = line.get(start) {
        let kind = class(first);
        let mut end = start.saturating_add(1);
        if kind != Class::Other {
            while line.get(end).is_some_and(|&byte| class(byte) == kind) {
                end = end.saturating_add(1);
            }
        }
        tokens
            .push(u32::try_from(start).unwrap_or(u32::MAX)..u32::try_from(end).unwrap_or(u32::MAX));
        start = end;
    }
    tokens
}

/// Which tokens of each side are in their longest common subsequence.
fn common(old: &[&[u8]], new: &[&[u8]]) -> (Vec<bool>, Vec<bool>) {
    let (rows, columns) = (old.len(), new.len());
    let width = columns.saturating_add(1);
    // `lengths[i * width + j]`: the longest common subsequence of `old[i..]` and `new[j..]`.
    let mut lengths = vec![0_u16; rows.saturating_add(1).saturating_mul(width)];
    let at = |i: usize, j: usize| i.saturating_mul(width).saturating_add(j);
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            let length = if old.get(i) == new.get(j) {
                lengths
                    .get(at(i.saturating_add(1), j.saturating_add(1)))
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1)
            } else {
                let down = lengths
                    .get(at(i.saturating_add(1), j))
                    .copied()
                    .unwrap_or(0);
                let right = lengths
                    .get(at(i, j.saturating_add(1)))
                    .copied()
                    .unwrap_or(0);
                down.max(right)
            };
            if let Some(cell) = lengths.get_mut(at(i, j)) {
                *cell = length;
            }
        }
    }
    let mut old_kept = vec![false; rows];
    let mut new_kept = vec![false; columns];
    let (mut i, mut j) = (0, 0);
    while i < rows && j < columns {
        if old.get(i) == new.get(j) {
            if let Some(kept) = old_kept.get_mut(i) {
                *kept = true;
            }
            if let Some(kept) = new_kept.get_mut(j) {
                *kept = true;
            }
            i = i.saturating_add(1);
            j = j.saturating_add(1);
        } else if lengths.get(at(i.saturating_add(1), j)) >= lengths.get(at(i, j.saturating_add(1)))
        {
            i = i.saturating_add(1);
        } else {
            j = j.saturating_add(1);
        }
    }
    (old_kept, new_kept)
}

/// The byte ranges of the tokens not kept, merged where they touch.
fn unkept(tokens: &[Range<u32>], kept: &[bool]) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for (token, kept) in tokens.iter().zip(kept) {
        if *kept {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.end == token.start => last.end = token.end,
            _ => ranges.push(token.clone()),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(line: &str, ranges: &[Range<u32>]) -> Vec<String> {
        ranges
            .iter()
            .map(|range| String::from_utf8(slice(line.as_bytes(), range).to_vec()).unwrap())
            .collect()
    }

    #[test]
    fn marks_the_words_that_changed() {
        let old = "let total = price * count;";
        let new = "let total = price * quantity + tax;";
        let changed = changed_words(old.as_bytes(), new.as_bytes()).unwrap();
        assert_eq!(marked(old, &changed.old), ["count"]);
        assert_eq!(marked(new, &changed.new), ["quantity + tax"]);
    }

    #[test]
    fn identical_lines_mark_nothing() {
        let line = b"fn main() {}";
        assert_eq!(changed_words(line, line), Some(ChangedWords::default()));
    }

    #[test]
    fn a_rewritten_line_isnt_marked() {
        assert_eq!(
            changed_words(
                b"return compute(a, b);",
                b"// TODO: something else entirely"
            ),
            None
        );
    }

    #[test]
    fn whitespace_changes_are_marked_too() {
        let changed = changed_words(b"a  b", b"a b").unwrap();
        assert_eq!(marked("a  b", &changed.old), ["  "]);
        assert_eq!(marked("a b", &changed.new), [" "]);
    }

    #[test]
    fn tokens_split_words_spaces_and_punctuation() {
        let line = "foo_bar(x1, 漢字)  ";
        let pieces = marked(line, &tokens(line.as_bytes()));
        assert_eq!(pieces, ["foo_bar", "(", "x1", ",", " ", "漢字", ")", "  "]);
    }

    #[test]
    fn very_long_lines_arent_compared() {
        let long = "a ".repeat(400);
        assert_eq!(changed_words(long.as_bytes(), b"a"), None);
    }
}
