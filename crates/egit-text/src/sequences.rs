//! Which elements two versions of a sequence share, so what changed between two versions of a
//! list (a document's blocks, say) can be marked, at a cost following what the change touches
//! rather than the lists' lengths.
//!
//! The common start and end are set aside first. Between them, the elements found once on each
//! side anchor the rest, as git's patience diff anchors lines: the longest run of them in the
//! same order on both sides is kept, and each stretch between two anchors is compared by
//! Myers' O(ND) algorithm (its own common start and end set aside too) for its longest common
//! subsequence. Past [`MOST_EDITS`] edits in one stretch the search stops and the stretch
//! counts as changed.

use std::collections::HashMap;
use std::hash::Hash;

/// The most edits searched for in one stretch.
pub const MOST_EDITS: usize = 1_000;

/// Which elements of `old` and `new` are kept; the others were removed or added.
///
/// What is kept is a common subsequence anchored on the elements found once on each side, and
/// otherwise longest (as far as [`MOST_EDITS`] edits a stretch find it).
#[must_use]
pub fn kept<T: Eq + Hash>(old: &[T], new: &[T]) -> (Vec<bool>, Vec<bool>) {
    let (start, old_middle, new_middle) = trimmed(old, new);
    let mut old_kept = vec![false; old.len()];
    let mut new_kept = vec![false; new.len()];
    for (kept, middle) in [(&mut old_kept, old_middle), (&mut new_kept, new_middle)] {
        let after = start.saturating_add(middle.len());
        for (index, kept) in kept.iter_mut().enumerate() {
            *kept = index < start || index >= after;
        }
    }
    let mut keep = |(x, y): (usize, usize)| {
        if let Some(kept) = old_kept.get_mut(start.saturating_add(x)) {
            *kept = true;
        }
        if let Some(kept) = new_kept.get_mut(start.saturating_add(y)) {
            *kept = true;
        }
    };
    // Each stretch up to an anchor, the anchor, and the stretch after the last.
    let mut from = (0, 0);
    let anchors = anchors(old_middle, new_middle);
    for anchor in anchors.iter().copied().map(Some).chain([None]) {
        let (to_x, to_y) = anchor.unwrap_or((old_middle.len(), new_middle.len()));
        let old_stretch = old_middle.get(from.0..to_x).unwrap_or_default();
        let new_stretch = new_middle.get(from.1..to_y).unwrap_or_default();
        align(old_stretch, new_stretch, &mut |(x, y)| {
            keep((from.0.saturating_add(x), from.1.saturating_add(y)));
        });
        if let Some(anchor) = anchor {
            keep(anchor);
        }
        from = (to_x.saturating_add(1), to_y.saturating_add(1));
    }
    (old_kept, new_kept)
}

/// `old` and `new` but their common start and end: the start's length, and what is between.
fn trimmed<'a, T: PartialEq>(old: &'a [T], new: &'a [T]) -> (usize, &'a [T], &'a [T]) {
    let start = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| old == new)
        .count();
    let (old_rest, new_rest) = (
        old.get(start..).unwrap_or_default(),
        new.get(start..).unwrap_or_default(),
    );
    let end = old_rest
        .iter()
        .rev()
        .zip(new_rest.iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    (
        start,
        old_rest
            .get(..old_rest.len().saturating_sub(end))
            .unwrap_or_default(),
        new_rest
            .get(..new_rest.len().saturating_sub(end))
            .unwrap_or_default(),
    )
}

/// Hands `keep` the pairs of `old`'s and `new`'s elements kept: their common start and end, and
/// between them a shortest edit's (none past [`MOST_EDITS`] edits).
fn align<T: PartialEq>(old: &[T], new: &[T], keep: &mut impl FnMut((usize, usize))) {
    let (start, old_middle, new_middle) = trimmed(old, new);
    let end = old
        .len()
        .saturating_sub(start)
        .saturating_sub(old_middle.len());
    for index in 0..start {
        keep((index, index));
    }
    for (x, y) in matches(old_middle, new_middle).unwrap_or_default() {
        keep((start.saturating_add(x), start.saturating_add(y)));
    }
    let (old_end, new_end) = (
        start.saturating_add(old_middle.len()),
        start.saturating_add(new_middle.len()),
    );
    for index in 0..end {
        keep((old_end.saturating_add(index), new_end.saturating_add(index)));
    }
}

/// The elements found once on each side, as pairs of their places, the longest run of them in
/// the same order on both (by patience sorting).
fn anchors<T: Eq + Hash>(old: &[T], new: &[T]) -> Vec<(usize, usize)> {
    // Each element's count and place on each side.
    let mut seen: HashMap<&T, [(u8, usize); 2]> = HashMap::with_capacity(old.len());
    for (side, elements) in [old, new].into_iter().enumerate() {
        for (place, element) in elements.iter().enumerate() {
            if side == 1 && !seen.contains_key(element) {
                continue;
            }
            let counts = seen.entry(element).or_default();
            if let Some((count, at)) = counts.get_mut(side) {
                *count = count.saturating_add(1);
                *at = place;
            }
        }
    }
    let mut once: Vec<(usize, usize)> = seen
        .into_values()
        .filter(|[(old, _), (new, _)]| *old == 1 && *new == 1)
        .map(|[(_, x), (_, y)]| (x, y))
        .collect();
    once.sort_unstable();
    increasing(&once)
}

/// The longest run of `pairs` (in order of their first) whose seconds increase too.
fn increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // `tails[i]`: the pair ending the best run of length `i + 1` so far (its smallest second).
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; pairs.len()];
    let second = |index: usize| pairs.get(index).map_or(0, |(_, y)| *y);
    for (index, (_, y)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|tail| second(*tail) < *y);
        if let Some(slot) = before.get_mut(index) {
            *slot = at
                .checked_sub(1)
                .and_then(|shorter| tails.get(shorter).copied());
        }
        match tails.get_mut(at) {
            Some(tail) => *tail = index,
            None => tails.push(index),
        }
    }
    let mut run = Vec::new();
    let mut at = tails.last().copied();
    while let Some(index) = at {
        run.extend(pairs.get(index).copied());
        at = before.get(index).copied().flatten();
    }
    run.reverse();
    run
}

/// The pairs of `old`'s and `new`'s elements a shortest edit keeps, in order; `None` past
/// [`MOST_EDITS`] edits.
///
/// Diagonal `k = x - y` is kept at index `k + offset` of each round's furthest `x`; a round's
/// values are kept (those of its diagonals, `-d..=d`) to trace the path back.
fn matches<T: PartialEq>(old: &[T], new: &[T]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (old.len(), new.len());
    let limit = n.saturating_add(m).min(MOST_EDITS);
    let offset = limit.saturating_add(1);
    let mut furthest = vec![0_usize; offset.saturating_mul(2).saturating_add(1)];
    let mut rounds: Vec<Vec<usize>> = Vec::new();
    for d in 0..=limit {
        let (low, high) = (offset.saturating_sub(d), offset.saturating_add(d));
        let mut ended = None;
        for index in (low..=high).step_by(2) {
            let (mut x, _) = step(&furthest, 0, (index, low, high));
            let mut y = x.saturating_add(offset).saturating_sub(index);
            // Past either end, `get` finds nothing: the snake stops.
            while old
                .get(x)
                .zip(new.get(y))
                .is_some_and(|(old, new)| old == new)
            {
                x = x.saturating_add(1);
                y = y.saturating_add(1);
            }
            if let Some(slot) = furthest.get_mut(index) {
                *slot = x;
            }
            if x >= n && y >= m {
                ended = Some((x, y));
                break;
            }
        }
        rounds.push(furthest.get(low..=high).unwrap_or_default().to_vec());
        if let Some(end) = ended {
            return Some(trace_back(&rounds, offset, end));
        }
    }
    None
}

/// Where round `d`'s path on diagonal `index` starts (its `x`), from the previous round's
/// furthest points (`values`, whose first is diagonal `base`): down from the diagonal above, or
/// right from the one below, whichever reaches further. Says whether it went down.
fn step(values: &[usize], base: usize, (index, low, high): (usize, usize, usize)) -> (usize, bool) {
    let at = |index: usize| {
        index
            .checked_sub(base)
            .and_then(|at| values.get(at))
            .copied()
            .unwrap_or(0)
    };
    let down = index == low
        || (index != high && at(index.saturating_sub(1)) < at(index.saturating_add(1)));
    if down {
        (at(index.saturating_add(1)), true)
    } else {
        (at(index.saturating_sub(1)).saturating_add(1), false)
    }
}

/// The matched pairs along the path ending at `end`, from the last round back to the first.
fn trace_back(rounds: &[Vec<usize>], offset: usize, end: (usize, usize)) -> Vec<(usize, usize)> {
    let (mut x, mut y) = end;
    let mut matched = Vec::new();
    for d in (0..rounds.len()).rev() {
        let index = offset.saturating_add(x).saturating_sub(y);
        // The first round starts at the top left; the others from the round before.
        let (start, from) = d
            .checked_sub(1)
            .and_then(|previous| rounds.get(previous))
            .map_or(((0, 0), (0, 0)), |values| moved(values, offset, (d, index)));
        while x > start.0 && y > start.1 {
            x = x.saturating_sub(1);
            y = y.saturating_sub(1);
            matched.push((x, y));
        }
        (x, y) = from;
    }
    matched.reverse();
    matched
}

/// Round `d`'s move onto diagonal `index`, from the previous round's furthest points
/// (`values`): where it left the path (its snake starts there), and where it came from.
fn moved(
    values: &[usize],
    offset: usize,
    (d, index): (usize, usize),
) -> ((usize, usize), (usize, usize)) {
    let low = offset.saturating_sub(d);
    let high = offset.saturating_add(d);
    let (start_x, down) = step(values, low.saturating_add(1), (index, low, high));
    let (from_index, from_x) = if down {
        (index.saturating_add(1), start_x)
    } else {
        (index.saturating_sub(1), start_x.saturating_sub(1))
    };
    let from_y = from_x.saturating_add(offset).saturating_sub(from_index);
    let start_y = start_x.saturating_add(offset).saturating_sub(index);
    ((start_x, start_y), (from_x, from_y))
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// The longest common subsequence's length, the slow way.
    fn longest(old: &[u8], new: &[u8]) -> usize {
        let mut lengths = vec![vec![0_usize; new.len() + 1]; old.len() + 1];
        for (i, a) in old.iter().enumerate().rev() {
            for (j, b) in new.iter().enumerate().rev() {
                lengths[i][j] = if a == b {
                    lengths[i + 1][j + 1] + 1
                } else {
                    lengths[i + 1][j].max(lengths[i][j + 1])
                };
            }
        }
        lengths[0][0]
    }

    /// The elements kept, in order: they must read the same on both sides.
    fn kept_of(items: &[u8], kept: &[bool]) -> Vec<u8> {
        items
            .iter()
            .zip(kept)
            .filter(|(_, kept)| **kept)
            .map(|(item, _)| *item)
            .collect()
    }

    #[test]
    fn what_both_keep_reads_the_same_and_myers_finds_the_longest() {
        // A small generator of its own: the same cases every run.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            u8::try_from(state % below).unwrap()
        };
        for _ in 0..500 {
            let old: Vec<u8> = (0..next(40)).map(|_| next(5)).collect();
            let mut new = old.clone();
            for _ in 0..next(8) {
                let at = usize::from(next(u64::try_from(new.len()).unwrap() + 1));
                match next(3) {
                    0 if at < new.len() => {
                        new.remove(at);
                    }
                    1 if at < new.len() => new[at] = next(5),
                    _ => new.insert(at.min(new.len()), next(5)),
                }
            }
            // Anchored or not, what is kept is common to both, in order.
            let (old_kept, new_kept) = kept(&old, &new);
            let (a, b) = (kept_of(&old, &old_kept), kept_of(&new, &new_kept));
            assert_eq!(a, b, "{old:?} {new:?}");
            // Myers' pairs are a longest common subsequence's.
            let pairs = matches(&old, &new).unwrap();
            assert!(pairs.iter().all(|(x, y)| old[*x] == new[*y]));
            assert!(
                pairs
                    .windows(2)
                    .all(|two| two[0].0 < two[1].0 && two[0].1 < two[1].1)
            );
            assert_eq!(pairs.len(), longest(&old, &new), "{old:?} {new:?}");
        }
    }

    #[test]
    fn elements_found_once_anchor_the_rest() {
        // Every section's heading renamed, its text the same: far past the most edits, yet
        // only the headings changed.
        let section = |n: u32, edition: &str| {
            [
                format!("## Section {n} ({edition})"),
                format!("Text of {n}."),
                "- a point".to_owned(),
                "- another".to_owned(),
            ]
        };
        let old: Vec<String> = (0..2_000).flat_map(|n| section(n, "first")).collect();
        let new: Vec<String> = (0..2_000).flat_map(|n| section(n, "second")).collect();
        let (old_kept, new_kept) = kept(&old, &new);
        for (index, kept) in old_kept.iter().chain(&new_kept).enumerate() {
            assert_eq!(*kept, index % 4 != 0, "{index}");
        }
    }

    #[test]
    fn changes_far_apart_keep_what_lies_between() {
        let old: Vec<u32> = (0..10_000).collect();
        let mut new = old.clone();
        new[3] = 99_999;
        new.insert(9_000, 88_888);
        let (old_kept, new_kept) = kept(&old, &new);
        assert_eq!(old_kept.iter().filter(|kept| !**kept).count(), 1);
        assert_eq!(new_kept.iter().filter(|kept| !**kept).count(), 2);
        assert!(!new_kept[3] && !new_kept[9_000]);
    }

    #[test]
    fn past_the_most_edits_a_stretch_is_changed() {
        // Nothing found once to anchor on, and 3,000 edits.
        let old: Vec<u32> = (0..3_000).map(|n| n % 2).collect();
        let new: Vec<u32> = (0..3_000).map(|n| (n % 2) * 2).collect();
        let (old_kept, new_kept) = kept(&old, &new);
        // The first stays (the common start), the rest between is changed.
        assert!(old_kept[0] && new_kept[0]);
        assert!(old_kept[1..].iter().all(|kept| !kept));
        assert!(new_kept[1..].iter().all(|kept| !kept));
        assert_eq!(kept::<u8>(&[], &[]), (vec![], vec![]));
        assert_eq!(kept(&[1], &[]), (vec![false], vec![]));
    }
}
