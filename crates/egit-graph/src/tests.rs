//! Laying out and drawing histories, from hand-made ones to random ones.

// Test arithmetic on small known numbers.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::BTreeSet;

use crate::{
    CHUNK_ROWS, Chunk, Edge, EdgeKind, FREE, GraphBuilder, Level, Point, Row, Segment, for_each_row,
};

/// Lays out `commits` (children first) and returns the chunks with the lanes before each row.
fn build(commits: &[(u32, Vec<u32>)]) -> (Vec<Chunk>, Vec<Vec<u16>>, usize) {
    build_with(GraphBuilder::new(), commits)
}

/// [`build`], with `builder`.
fn build_with(
    mut builder: GraphBuilder<u32>,
    commits: &[(u32, Vec<u32>)],
) -> (Vec<Chunk>, Vec<Vec<u16>>, usize) {
    let mut chunks = Vec::new();
    let mut before = Vec::new();
    for (commit, parents) in commits {
        before.push(builder.lane_colors().to_vec());
        if let Some(chunk) = builder.push(*commit, parents) {
            chunks.push(chunk);
        }
    }
    let widest = builder.widest();
    chunks.extend(builder.finish());
    (chunks, before, widest)
}

/// A row as drawn: the lanes before it, the row, its edges, and its segments.
type Drawn = (Vec<u16>, Row, Vec<Edge>, Vec<Segment>);

/// Every row, as drawn from row 0.
fn drawn(chunks: &[Chunk], rows: u64) -> Vec<Drawn> {
    let mut lanes = Vec::new();
    let mut out = Vec::new();
    for_each_row(chunks, 0..rows, &mut lanes, |_, view| {
        let mut segments = Vec::new();
        view.segments(&mut segments);
        out.push((
            view.before.to_vec(),
            view.row,
            view.edges.to_vec(),
            segments,
        ));
    });
    out
}

fn segment(from: (u16, Level), to: (u16, Level), color: u16) -> Segment {
    Segment {
        from: Point {
            lane: from.0,
            level: from.1,
        },
        to: Point {
            lane: to.0,
            level: to.1,
        },
        color,
    }
}

#[test]
fn a_straight_history_is_one_lane() {
    let commits = vec![(3, vec![2]), (2, vec![1]), (1, vec![])];
    let (chunks, _, widest) = build(&commits);
    assert_eq!(widest, 1);
    let rows = drawn(&chunks, 3);
    assert_eq!(rows.len(), 3);
    let (before, tip, edges, segments) = &rows[0];
    assert_eq!(before, &Vec::<u16>::new());
    assert_eq!((tip.lane, tip.from_above, tip.continues), (0, false, true));
    assert_eq!(edges, &[]);
    assert_eq!(
        segments,
        &[segment((0, Level::Middle), (0, Level::Bottom), 0)]
    );
    let (_, middle, _, segments) = &rows[1];
    assert_eq!((middle.from_above, middle.continues), (true, true));
    assert_eq!(
        segments,
        &[
            segment((0, Level::Top), (0, Level::Middle), 0),
            segment((0, Level::Middle), (0, Level::Bottom), 0),
        ]
    );
    let (_, root, _, segments) = &rows[2];
    assert_eq!((root.from_above, root.continues), (true, false));
    assert_eq!(segments, &[segment((0, Level::Top), (0, Level::Middle), 0)]);
}

#[test]
fn a_branch_forks_at_its_merge_and_joins_where_it_began() {
    // M merges E (topic: D–E) into C; both lines start at B.
    let commits = vec![
        (7, vec![3, 5]), // M
        (5, vec![4]),    // E
        (4, vec![2]),    // D
        (3, vec![2]),    // C
        (2, vec![1]),    // B
        (1, vec![]),     // A
    ];
    let (chunks, _, widest) = build(&commits);
    assert_eq!(widest, 2);
    let rows = drawn(&chunks, 6);

    let (_, merge, edges, segments) = &rows[0];
    assert_eq!(merge.lane, 0);
    assert_eq!(
        edges,
        &[Edge {
            kind: EdgeKind::Fork,
            lane: 1,
            color: 1
        }]
    );
    assert_eq!(
        segments,
        &[
            segment((0, Level::Middle), (0, Level::Bottom), 0),
            segment((0, Level::Middle), (1, Level::Bottom), 1),
        ]
    );
    // E and D sit in the topic's lane, beside the main lane passing through.
    assert_eq!((rows[1].1.lane, rows[1].1.color), (1, 1));
    assert_eq!((rows[2].1.lane, rows[2].1.color), (1, 1));
    assert!(
        rows[1]
            .3
            .contains(&segment((0, Level::Top), (0, Level::Bottom), 0))
    );
    assert_eq!(rows[3].1.lane, 0);

    // At B, the topic's lane joins the main one.
    let (before, base, edges, segments) = &rows[4];
    assert_eq!(before, &vec![0, 1]);
    assert_eq!(base.lane, 0);
    assert_eq!(
        edges,
        &[Edge {
            kind: EdgeKind::Join,
            lane: 1,
            color: 1
        }]
    );
    assert!(segments.contains(&segment((1, Level::Top), (0, Level::Middle), 1)));
    assert_eq!(rows[5].0, vec![0], "the topic's lane is gone");
}

#[test]
fn an_octopus_forks_a_lane_per_parent() {
    let commits = vec![
        (9, vec![1, 2, 3, 4]),
        (4, vec![]),
        (3, vec![]),
        (2, vec![]),
        (1, vec![]),
    ];
    let (chunks, _, widest) = build(&commits);
    assert_eq!(widest, 4);
    let rows = drawn(&chunks, 5);
    let forks: Vec<u16> = rows[0].2.iter().map(|edge| edge.lane).collect();
    assert_eq!(forks, [1, 2, 3]);
    assert!(rows[0].2.iter().all(|edge| edge.kind == EdgeKind::Fork));
}

#[test]
fn a_merge_into_a_lane_already_open_draws_into_it() {
    // X and Y both merge P; Y's second parent P already has X's lane.
    let commits = vec![
        (10, vec![5, 6]),
        (11, vec![5, 6]),
        (6, vec![1]),
        (5, vec![1]),
        (1, vec![]),
    ];
    let (chunks, _, _) = build(&commits);
    let rows = drawn(&chunks, 5);
    let (_, second, edges, _) = &rows[1];
    assert!(!second.from_above);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].kind, EdgeKind::Merge);
    assert_eq!(edges[0].lane, 1);
}

#[test]
fn unrelated_histories_sit_side_by_side_and_free_their_lanes() {
    let commits = vec![(2, vec![1]), (12, vec![11]), (11, vec![]), (1, vec![])];
    let (chunks, _, widest) = build(&commits);
    assert_eq!(widest, 2);
    let rows = drawn(&chunks, 4);
    assert_eq!(rows[1].1.lane, 1, "the second tip takes the next free slot");
    assert_eq!(rows[3].0, vec![0], "the second history's lane is gone");
    assert_eq!(rows[3].1.lane, 0);
}

/// A random history of `count` commits, children first: mostly straight lines, with merges,
/// new branches, and a few roots.
fn random_history(count: u32, mut seed: u64) -> Vec<(u32, Vec<u32>)> {
    let mut random = move |bound: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        u32::try_from(seed % u64::from(bound.max(1))).unwrap()
    };
    let mut created: Vec<(u32, Vec<u32>)> = Vec::new();
    for id in 0..count {
        let parents = if id == 0 || random(100) < 2 {
            Vec::new()
        } else {
            let recent = |random: &mut dyn FnMut(u32) -> u32| id - 1 - random(id.min(40));
            let first = if random(100) < 85 {
                id - 1
            } else {
                recent(&mut random)
            };
            let mut parents = vec![first];
            for _ in 0..(if random(100) < 15 { 1 + random(3) } else { 0 }) {
                let other = recent(&mut random);
                if !parents.contains(&other) {
                    parents.push(other);
                }
            }
            parents
        };
        created.push((id, parents));
    }
    created.reverse();
    created
}

#[test]
fn every_lane_drawn_into_a_row_is_drawn_out_of_it() {
    for seed in 1..=20 {
        let commits = random_history(3_000, seed);
        let (chunks, before, _) = build(&commits);
        lanes_drawn_whole(seed, &chunks, &before, false);
    }
}

/// Every lane drawn into each row is drawn out of it, as the lanes say; a node is at most a
/// slot past the lanes open (two with the first kept for a pinned branch).
fn lanes_drawn_whole(seed: u64, chunks: &[Chunk], before: &[Vec<u16>], first_kept: bool) {
    let rows = drawn(chunks, 3_000);
    assert_eq!(rows.len(), 3_000);
    for (index, (lanes, row, _, segments)) in rows.iter().enumerate() {
        assert_eq!(
            lanes, &before[index],
            "seed {seed}: lanes before row {index}"
        );
        let open = |lanes: &[u16]| -> BTreeSet<u16> {
            lanes
                .iter()
                .enumerate()
                .filter(|(_, color)| **color != FREE)
                .map(|(slot, _)| u16::try_from(slot).unwrap())
                .collect()
        };
        let from_top: BTreeSet<u16> = segments
            .iter()
            .filter(|segment| segment.from.level == Level::Top)
            .map(|segment| segment.from.lane)
            .collect();
        assert_eq!(from_top, open(lanes), "seed {seed}: row {index}'s top");
        let after = before.get(index + 1).cloned().unwrap_or_default();
        let to_bottom: BTreeSet<u16> = segments
            .iter()
            .filter(|segment| segment.to.level == Level::Bottom)
            .map(|segment| segment.to.lane)
            .collect();
        assert_eq!(to_bottom, open(&after), "seed {seed}: row {index}'s bottom");
        assert!(
            usize::from(row.lane) <= lanes.len().max(usize::from(first_kept)),
            "seed {seed}: a node far from the lanes"
        );
    }
}

#[test]
fn a_pinned_branch_keeps_the_first_lane_from_its_tip_down() {
    // `topic` (4, 3) is newer than `main` (2, 1), both from 0: pinned, `main` takes the first
    // lane, which is empty above it; `topic`'s lane joins it at 0.
    let commits = [
        (4, vec![3]),
        (3, vec![0]),
        (2, vec![1]),
        (1, vec![0]),
        (0, vec![]),
    ];
    let (chunks, before, widest) = build_with(GraphBuilder::pinned(2), &commits);
    let rows = drawn(&chunks, 5);
    let lanes: Vec<u16> = rows.iter().map(|(_, row, _, _)| row.lane).collect();
    assert_eq!(lanes, [1, 1, 0, 0, 0]);
    assert_eq!(widest, 2);
    assert_eq!(
        before[1],
        [FREE, 0],
        "nothing in the first lane above the tip"
    );
    assert!(!rows[2].1.from_above, "the tip starts its lane");
    let (_, root, edges, _) = &rows[4];
    assert_eq!(root.lane, 0);
    assert_eq!(
        edges[0].kind,
        EdgeKind::Join,
        "topic joins main at its root"
    );

    // Unpinned, the newer branch would have the first lane.
    let (chunks, _, _) = build(&commits);
    let lanes: Vec<u16> = drawn(&chunks, 5)
        .iter()
        .map(|(_, row, _, _)| row.lane)
        .collect();
    assert_eq!(lanes, [0, 0, 1, 1, 0]);
}

#[test]
fn a_pinned_line_alone_takes_the_first_lane_in_any_history() {
    for seed in 1..=20 {
        let commits = random_history(3_000, seed);
        // The tip of the newest branch whose first-parent line is long: the first commit
        // listed that no other commit has as a parent.
        let parents: BTreeSet<u32> = commits
            .iter()
            .flat_map(|(_, parents)| parents.iter().copied())
            .collect();
        let tips: Vec<u32> = commits
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !parents.contains(id))
            .collect();
        let tip = tips[usize::try_from(seed).unwrap() % tips.len()];
        let mut line = BTreeSet::new();
        let mut at = Some(tip);
        let first_parent: std::collections::HashMap<u32, Option<u32>> = commits
            .iter()
            .map(|(id, parents)| (*id, parents.first().copied()))
            .collect();
        while let Some(id) = at {
            line.insert(id);
            at = first_parent[&id];
        }
        let (chunks, before, _) = build_with(GraphBuilder::pinned(tip), &commits);
        lanes_drawn_whole(seed, &chunks, &before, true);
        for ((id, _), (_, row, _, _)) in commits.iter().zip(drawn(&chunks, 3_000)) {
            assert_eq!(
                row.lane == 0,
                line.contains(id),
                "seed {seed}: commit {id} in lane {}",
                row.lane
            );
        }
    }
}

#[test]
fn any_range_draws_as_it_does_from_the_start() {
    let commits = random_history(5_000, 7);
    let (chunks, _, _) = build(&commits);
    assert_eq!(chunks.len(), 5, "4 full chunks and a partial one");
    assert!(chunks[..4].iter().all(|chunk| chunk.len() == CHUNK_ROWS));
    let all = drawn(&chunks, 5_000);
    for start in [0_u64, 1, 1023, 1024, 1025, 2047, 3000, 4999] {
        let mut lanes = Vec::new();
        let mut seen = Vec::new();
        for_each_row(
            &chunks,
            start..(start + 40).min(5_000),
            &mut lanes,
            |number, view| {
                seen.push((number, view.before.to_vec(), view.row));
            },
        );
        for (number, before, row) in seen {
            let index = usize::try_from(number).unwrap();
            assert_eq!(
                (before, row),
                (all[index].0.clone(), all[index].1),
                "row {number}"
            );
        }
    }
    let mut lanes = Vec::new();
    let mut none = 0;
    for_each_row(&chunks, 5_000..5_010, &mut lanes, |_, _| none += 1);
    assert_eq!(none, 0, "rows past the end aren't drawn");
}

#[test]
fn a_row_costs_twelve_bytes() {
    assert_eq!(size_of::<Row>(), 12);
    assert_eq!(size_of::<Edge>(), 6);
}

#[test]
fn a_window_draws_only_the_lines_crossing_it() {
    let commits = random_history(4_000, 11);
    let (chunks, _, widest) = build(&commits);
    assert!(widest > 8, "a history wide enough to window");
    let crosses = |segment: &Segment, window: &std::ops::Range<u16>| {
        let (low, high) = (
            segment.from.lane.min(segment.to.lane),
            segment.from.lane.max(segment.to.lane),
        );
        low < window.end && high >= window.start
    };
    let mut lanes = Vec::new();
    for window in [0..1, 0..4, 2..5, 3..u16::MAX, 7..7, 40..60] {
        for_each_row(&chunks, 0..4_000, &mut lanes, |number, view| {
            let mut all = Vec::new();
            view.segments(&mut all);
            all.retain(|segment| crosses(segment, &window));
            let mut windowed = Vec::new();
            view.segments_in(window.clone(), &mut windowed);
            assert_eq!(windowed, all, "row {number} in {window:?}");
        });
    }
}
