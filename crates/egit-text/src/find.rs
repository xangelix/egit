//! Finding text: every place a query occurs, as byte offsets into a text.

/// How often a search asks whether to stop, in bytes searched.
const CHECK_EVERY: usize = 1 << 22;

/// Where `query` occurs in `bytes`: each match's first byte, matches not overlapping.
///
/// At most `limit` are found; `stop` is asked every few megabytes whether to give up (`None`).
/// With `fold_case`, ASCII letters match in either case. A query spanning lines finds nothing.
pub fn find_all(
    bytes: &[u8],
    query: &[u8],
    fold_case: bool,
    limit: usize,
    mut stop: impl FnMut() -> bool,
) -> Option<Vec<u32>> {
    let mut found = Vec::new();
    if query.is_empty() || query.contains(&b'\n') {
        return Some(found);
    }
    let fold = fold_case && query.iter().any(u8::is_ascii_alphabetic);
    let search = |window: &[u8]| {
        if fold {
            find_folded(window, query)
        } else {
            memchr::memmem::find(window, query)
        }
    };
    // Searched a window at a time, so that `stop` is asked even where nothing matches; windows
    // overlap by the query's length, so no match is missed at their edges.
    let mut at = 0_usize;
    let mut next_check = CHECK_EVERY;
    while found.len() < limit && at < bytes.len() {
        if at >= next_check {
            if stop() {
                return None;
            }
            next_check = at.saturating_add(CHECK_EVERY);
        }
        let end = at
            .saturating_add(CHECK_EVERY)
            .saturating_add(query.len())
            .min(bytes.len());
        if let Some(offset) = search(bytes.get(at..end)?) {
            let start = at.saturating_add(offset);
            found.push(u32::try_from(start).unwrap_or(u32::MAX));
            at = start.saturating_add(query.len());
        } else if end == bytes.len() {
            break;
        } else {
            at = end
                .saturating_sub(query.len())
                .saturating_add(1)
                .max(at.saturating_add(1));
            if stop() {
                return None;
            }
            next_check = at.saturating_add(CHECK_EVERY);
        }
    }
    Some(found)
}

/// Whether a query should match case: it should when it has a capital (smart case).
#[must_use]
pub fn matches_case(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// A query made ready to look for in many short texts (each commit's message, say): matched
/// as it is through a finder built once, or with ASCII letters in either case.
#[derive(Clone, Debug)]
pub struct Needle {
    finder: memchr::memmem::Finder<'static>,
    fold: bool,
}

impl Needle {
    /// `query`, its case matched unless `fold_case` (and it has a letter to fold).
    #[must_use]
    pub fn new(query: &[u8], fold_case: bool) -> Self {
        Self {
            finder: memchr::memmem::Finder::new(query).into_owned(),
            fold: fold_case && query.iter().any(u8::is_ascii_alphabetic),
        }
    }

    /// Where it first occurs in `haystack`; an empty query occurs at the start of anything.
    #[must_use]
    pub fn find(&self, haystack: &[u8]) -> Option<usize> {
        if self.fold {
            find_folded(haystack, self.finder.needle())
        } else {
            self.finder.find(haystack)
        }
    }
}

/// The first place `query` occurs in `haystack`, ASCII letters in either case. Scans for the
/// query's first byte in both cases, then compares the rest.
fn find_folded(haystack: &[u8], query: &[u8]) -> Option<usize> {
    let (&first, rest) = query.split_first()?;
    let (lower, upper) = (first.to_ascii_lowercase(), first.to_ascii_uppercase());
    let mut from = 0_usize;
    loop {
        let candidate = from.saturating_add(memchr::memchr2(lower, upper, haystack.get(from..)?)?);
        let after = candidate.saturating_add(1);
        let end = after.saturating_add(rest.len());
        match haystack.get(after..end) {
            Some(tail) if tail.eq_ignore_ascii_case(rest) => return Some(candidate),
            Some(_) => from = after,
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(bytes: &str, query: &str, fold: bool) -> Vec<u32> {
        find_all(bytes.as_bytes(), query.as_bytes(), fold, usize::MAX, || {
            false
        })
        .unwrap()
    }

    #[test]
    fn finds_every_match_without_overlap() {
        assert_eq!(all("abcabcab", "abc", false), [0, 3]);
        assert_eq!(all("aaaa", "aa", false), [0, 2]);
        assert_eq!(all("no match", "xyz", false), Vec::<u32>::new());
        assert_eq!(all("anything", "", false), Vec::<u32>::new());
    }

    #[test]
    fn folds_ascii_case_when_asked() {
        assert_eq!(all("Foo foo FOO", "foo", true), [0, 4, 8]);
        assert_eq!(all("Foo foo FOO", "foo", false), [4]);
        // Non-letters and other scripts match as they are.
        assert_eq!(all("x=1; X=1;", "x=1", true), [0, 5]);
        assert_eq!(all("Grüße grüße", "grüße", true), [0, 8]);
    }

    #[test]
    fn stops_at_the_limit_and_when_asked() {
        let many = "ab".repeat(100);
        assert_eq!(
            find_all(many.as_bytes(), b"ab", false, 3, || false).unwrap(),
            [0, 2, 4]
        );
        let huge = vec![b'a'; (1 << 22) + 10];
        assert_eq!(find_all(&huge, b"b", false, usize::MAX, || true), None);
    }

    #[test]
    fn matches_across_window_edges_are_found() {
        let mut bytes = vec![b'.'; (1 << 22) - 2];
        bytes.extend_from_slice(b"needle");
        bytes.extend(vec![b'.'; 100]);
        bytes.extend_from_slice(b"NEEDLE");
        let found = find_all(&bytes, b"needle", true, usize::MAX, || false).unwrap();
        assert_eq!(found, [(1 << 22) - 2, (1 << 22) + 104]);
    }

    #[test]
    fn a_needle_finds_its_first_place_in_each_text() {
        let needle = Needle::new(b"leak", true);
        assert_eq!(needle.find(b"Fix a memory LEAK, a leak"), Some(13));
        assert_eq!(needle.find(b"no such thing"), None);
        let exact = Needle::new(b"Leak", false);
        assert_eq!(exact.find(b"leak, then Leak"), Some(11));
        // Nothing to fold: matched as it is.
        assert_eq!(Needle::new(b"1.2", true).find(b"v1.2"), Some(1));
        assert_eq!(Needle::new(b"", true).find(b"x"), Some(0));
    }

    #[test]
    fn a_query_spanning_lines_finds_nothing() {
        assert_eq!(all("a\nb", "a\nb", false), Vec::<u32>::new());
    }

    #[test]
    fn capitals_mean_matching_case() {
        assert!(!matches_case("parse"));
        assert!(matches_case("Parse"));
        assert!(!matches_case("漢字"));
    }
}
