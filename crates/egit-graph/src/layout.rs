//! Laying out lanes in one pass over a history, children before parents.

use std::collections::HashMap;
use std::hash::Hash;

/// How many rows a [`Chunk`] holds (the last may hold fewer).
pub const CHUNK_ROWS: usize = 1024;

/// A lane slot with no lane in it.
pub const FREE: u16 = u16::MAX;

/// How many colours lanes cycle through before reusing one.
const COLORS: u16 = 4096;

/// The end of a chain of lanes leading to the same commit.
const NO_SLOT: u32 = u32::MAX;

/// Slots per word of the occupied-slot bitmap.
const WORD_BITS: usize = 64;

/// How a lane meets a commit's node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EdgeKind {
    /// Another child's lane ends here: it comes from above into the node.
    Join,
    /// A lane starts here, from the node down toward a merge parent no lane leads to yet.
    Fork,
    /// A merge parent another lane already leads to: a line from the node down into that lane,
    /// which goes on.
    Merge,
}

/// A lane meeting a commit's node, other than the node's own lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// How it meets the node.
    pub kind: EdgeKind,
    /// The other lane.
    pub lane: u16,
    /// The other lane's colour.
    pub color: u16,
}

/// One commit's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// The lane the commit's node sits in.
    pub lane: u16,
    /// That lane's colour.
    pub color: u16,
    /// Whether the node's lane comes from above (a child leads to this commit); a branch's tip
    /// starts its lane here.
    pub from_above: bool,
    /// Whether the node's lane goes on below, toward the first parent; a root commit ends it.
    pub continues: bool,
    edges_start: u32,
    edges_len: u16,
}

/// Up to [`CHUNK_ROWS`] consecutive rows, with the lanes open where they start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    first_row: u64,
    /// Each lane slot's colour entering the first row ([`FREE`] for an empty slot).
    entry: Box<[u16]>,
    rows: Vec<Row>,
    edges: Vec<Edge>,
}

impl Chunk {
    fn starting(first_row: u64, entry: Box<[u16]>) -> Self {
        Self {
            first_row,
            entry,
            rows: Vec::with_capacity(CHUNK_ROWS),
            edges: Vec::new(),
        }
    }

    /// The number of its first row.
    #[must_use]
    pub const fn first_row(&self) -> u64 {
        self.first_row
    }

    /// How many rows it holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether it holds no rows.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The lanes entering its first row: each slot's colour, or [`FREE`].
    #[must_use]
    pub fn entry(&self) -> &[u16] {
        &self.entry
    }

    /// Its `index`th row and the edges meeting that row's node.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<(Row, &[Edge])> {
        let row = *self.rows.get(index)?;
        let start = usize::try_from(row.edges_start).ok()?;
        let end = start.checked_add(usize::from(row.edges_len))?;
        Some((row, self.edges.get(start..end)?))
    }

    /// The memory it takes, roughly, in bytes.
    #[must_use]
    pub fn footprint(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.entry.len().saturating_mul(size_of::<u16>()))
            .saturating_add(self.rows.capacity().saturating_mul(size_of::<Row>()))
            .saturating_add(self.edges.capacity().saturating_mul(size_of::<Edge>()))
    }
}

/// A lane slot's occupant: its colour, the next slot whose lane leads to the same commit
/// ([`NO_SLOT`] for none), and whether it is set aside (a stash's, [`GraphBuilder::push_aside`]):
/// the commit it leads to never takes it as its own. The commit it leads to is the key of the
/// chain it's in.
#[derive(Clone, Copy, Debug)]
struct Lane {
    color: u16,
    next: u32,
    aside: bool,
}

/// Lays out lanes as commits arrive, children before parents (`git rev-list --topo-order`).
///
/// A commit's node takes the leftmost lane leading to it, or the leftmost free slot when none
/// does (a branch's tip). Other lanes leading to it join it there. Its lane goes on toward its
/// first parent; each merge parent gets a new lane to the node's right, unless a lane already
/// leads there. Freed slots are reused, keeping the graph narrow.
///
/// A commit costs time in proportion to its parents and the lanes meeting it, never the graph's
/// width: the lanes leading to each commit are chained from a hash table, and free slots are
/// found in a bitmap.
///
/// A branch can be pinned to the left ([`Self::pinned`]): the first slot is kept for its tip's
/// first-parent line, which nothing else takes; once the tip has it, the line keeps it, a node
/// taking the leftmost lane leading to it.
///
/// A commit on no branch (a stash) is pushed aside ([`Self::push_aside`]): its node takes a slot
/// right of the first, and its lane to the commit it was made on gives way there, so the
/// branches keep their lanes and the stash joins one beside it.
#[derive(Debug)]
pub struct GraphBuilder<K> {
    lanes: Vec<Option<Lane>>,
    /// One bit per slot, set while a lane occupies it.
    occupied: Vec<u64>,
    /// The first slot of each chain of lanes leading to the same commit.
    chains: HashMap<K, u32>,
    next_color: u16,
    chunk: Chunk,
    rows: u64,
    widest: usize,
    /// The lanes leading to the commit being laid out (reused).
    leading: Vec<usize>,
    /// The pinned branch's tip, until it is laid out.
    pinned: Option<K>,
    /// Whether the first slot is kept for the pinned branch.
    keep_first: bool,
}

impl<K: Copy + Eq + Hash> Default for GraphBuilder<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Eq + Hash> GraphBuilder<K> {
    /// A builder with no rows yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lanes: Vec::new(),
            occupied: Vec::new(),
            chains: HashMap::new(),
            next_color: 0,
            chunk: Chunk::starting(0, Box::default()),
            rows: 0,
            widest: 0,
            leading: Vec::new(),
            pinned: None,
            keep_first: false,
        }
    }

    /// A builder keeping the first lane for `tip`'s first-parent line (a branch pinned to the
    /// left): before the tip comes, the lane is empty; from it on, the line is there.
    #[must_use]
    pub fn pinned(tip: K) -> Self {
        Self {
            pinned: Some(tip),
            keep_first: true,
            ..Self::new()
        }
    }

    /// How many rows have been laid out.
    #[must_use]
    pub const fn rows(&self) -> u64 {
        self.rows
    }

    /// The most lanes any row has had open.
    #[must_use]
    pub const fn widest(&self) -> usize {
        self.widest
    }

    /// Lays out the next commit, given its parents (first parent first). Returns the chunk this
    /// row completed, if any.
    pub fn push(&mut self, commit: K, parents: &[K]) -> Option<Chunk> {
        let full = self.next_chunk();
        self.lay_out(commit, parents, false);
        self.rows = self.rows.saturating_add(1);
        full
    }

    /// Lays out the next commit as one on no branch (a stash), made on `base`: its node in a
    /// slot right of the first, its lane to `base` set aside (`base` takes a lane of its own,
    /// this one joining it). Returns the chunk this row completed, if any.
    pub fn push_aside(&mut self, commit: K, base: K) -> Option<Chunk> {
        let full = self.next_chunk();
        self.lay_out(commit, &[base], true);
        self.rows = self.rows.saturating_add(1);
        full
    }

    /// The chunk the next row starts after, when the current one is full.
    fn next_chunk(&mut self) -> Option<Chunk> {
        (self.chunk.len() == CHUNK_ROWS).then(|| {
            let next = Chunk::starting(self.rows, self.lane_colors());
            std::mem::replace(&mut self.chunk, next)
        })
    }

    /// The rows laid out since the last full chunk, if any.
    #[must_use]
    pub fn finish(self) -> Option<Chunk> {
        (!self.chunk.is_empty()).then_some(self.chunk)
    }

    /// Each slot's colour, or [`FREE`].
    pub(crate) fn lane_colors(&self) -> Box<[u16]> {
        self.lanes
            .iter()
            .map(|lane| lane.map_or(FREE, |lane| lane.color))
            .collect()
    }

    fn lay_out(&mut self, commit: K, parents: &[K], aside: bool) {
        self.leading.clear();
        let mut next = self.chains.remove(&commit).unwrap_or(NO_SLOT);
        while let Some(slot) = slot_index(next) {
            self.leading.push(slot);
            next = self.lane(slot).map_or(NO_SLOT, |lane| lane.next);
        }
        self.leading.sort_unstable();
        let edges_start = u32::try_from(self.chunk.edges.len()).unwrap_or(u32::MAX);
        // The pinned tip starts its lane in the first slot, kept empty for it: every lane
        // leading to it (its children's) joins it there. Otherwise the node takes the leftmost
        // lane leading to it but those set aside (a stash's).
        let pinned_here = self.pinned == Some(commit);
        let own = if pinned_here {
            None
        } else {
            self.leading
                .iter()
                .copied()
                .find(|&slot| self.lane(slot).is_some_and(|lane| !lane.aside))
        };
        let (node, color, from_above) = if pinned_here {
            self.pinned = None;
            (0, self.new_color(), false)
        } else if let Some(own) = own {
            (own, self.lane(own).map_or(0, |lane| lane.color), true)
        } else {
            // A commit on no branch keeps out of the first slot, where a branch's line runs.
            (self.free_slot(usize::from(aside)), self.new_color(), false)
        };
        for index in 0..self.leading.len() {
            let Some(&other) = self.leading.get(index) else {
                continue;
            };
            if Some(other) == own {
                continue;
            }
            let other_color = self.lane(other).map_or(0, |lane| lane.color);
            self.chunk.edges.push(Edge {
                kind: EdgeKind::Join,
                lane: slot_number(other),
                color: other_color,
            });
            self.set(other, None);
        }

        let continues = self.lead_to_parents(node, (color, aside), parents);
        self.widest = self.widest.max(self.lanes.len());
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        self.occupied.truncate(self.lanes.len().div_ceil(WORD_BITS));

        let edges_len = self
            .chunk
            .edges
            .len()
            .saturating_sub(usize::try_from(edges_start).unwrap_or(usize::MAX));
        self.chunk.rows.push(Row {
            lane: slot_number(node),
            color,
            from_above,
            continues,
            edges_start,
            edges_len: u16::try_from(edges_len).unwrap_or(u16::MAX),
        });
    }

    /// Points the node's lane (its colour, and whether it is set aside) at the first parent
    /// and gives each merge parent a lane (a new one, or the one already leading there).
    /// Returns whether the node's lane goes on.
    fn lead_to_parents(&mut self, node: usize, (color, aside): (u16, bool), parents: &[K]) -> bool {
        let Some((&first, merged)) = parents.split_first() else {
            self.set(node, None);
            return false;
        };
        self.open(node, first, (color, aside));
        for &parent in merged {
            let edge = if let Some(existing) = self.leftmost_leading_to(parent) {
                Edge {
                    kind: EdgeKind::Merge,
                    lane: slot_number(existing),
                    color: self.lane(existing).map_or(0, |lane| lane.color),
                }
            } else {
                let slot = self.free_slot(node.saturating_add(1));
                let lane_color = self.new_color();
                self.open(slot, parent, (lane_color, false));
                Edge {
                    kind: EdgeKind::Fork,
                    lane: slot_number(slot),
                    color: lane_color,
                }
            };
            self.chunk.edges.push(edge);
        }
        true
    }

    fn lane(&self, slot: usize) -> Option<Lane> {
        self.lanes.get(slot).copied().flatten()
    }

    /// The leftmost slot whose lane leads to `commit`, if any.
    fn leftmost_leading_to(&self, commit: K) -> Option<usize> {
        let mut next = self.chains.get(&commit).copied().unwrap_or(NO_SLOT);
        let mut leftmost = None;
        while let Some(slot) = slot_index(next) {
            leftmost = Some(leftmost.map_or(slot, |leftmost: usize| leftmost.min(slot)));
            next = self.lane(slot).map_or(NO_SLOT, |lane| lane.next);
        }
        leftmost
    }

    /// Opens a lane in `slot` leading to `expects` (its colour, and whether it is set aside),
    /// chaining it to the others leading there.
    fn open(&mut self, slot: usize, expects: K, (color, aside): (u16, bool)) {
        let link = u32::try_from(slot).unwrap_or(NO_SLOT);
        let next = self.chains.insert(expects, link).unwrap_or(NO_SLOT);
        self.set(slot, Some(Lane { color, next, aside }));
    }

    /// Puts `lane` in `slot`. Closing a lane ([`None`]) is only done once its chain is gone.
    fn set(&mut self, slot: usize, lane: Option<Lane>) {
        let open = lane.is_some();
        if let Some(existing) = self.lanes.get_mut(slot) {
            *existing = lane;
        } else if open {
            self.lanes.resize(slot, None);
            self.lanes.push(lane);
        } else {
            return;
        }
        let (word, bit) = bit(slot);
        if self.occupied.len() <= word {
            self.occupied.resize(word.saturating_add(1), 0);
        }
        if let Some(word) = self.occupied.get_mut(word) {
            if open {
                *word |= bit;
            } else {
                *word &= !bit;
            }
        }
    }

    /// The first free slot at or after `from` (never the first, while it is kept), or a new one
    /// past the last.
    fn free_slot(&self, from: usize) -> usize {
        let from = if self.keep_first { from.max(1) } else { from };
        let (first, bit) = bit(from);
        let mut wanted = !bit.wrapping_sub(1);
        for (index, &word) in self.occupied.iter().enumerate().skip(first) {
            let free = !word & wanted;
            if free != 0 {
                let offset = usize::try_from(free.trailing_zeros()).unwrap_or(0);
                return index.saturating_mul(WORD_BITS).saturating_add(offset);
            }
            wanted = u64::MAX;
        }
        self.lanes.len().max(from)
    }

    fn new_color(&mut self) -> u16 {
        let color = self.next_color;
        self.next_color = self
            .next_color
            .wrapping_add(1)
            .checked_rem(COLORS)
            .unwrap_or(0);
        color
    }
}

/// The slot a chain link names, unless it ends the chain.
fn slot_index(link: u32) -> Option<usize> {
    (link != NO_SLOT)
        .then(|| usize::try_from(link).ok())
        .flatten()
}

/// The bitmap word holding a slot's bit, and the bit.
fn bit(slot: usize) -> (usize, u64) {
    let shift = u32::try_from(slot.rem_euclid(WORD_BITS)).unwrap_or(0);
    (slot.div_euclid(WORD_BITS), 1_u64.wrapping_shl(shift))
}

/// A slot's number as rows store it (graphs are never 65,535 lanes wide).
fn slot_number(slot: usize) -> u16 {
    u16::try_from(slot).unwrap_or(FREE.saturating_sub(1))
}
