//! Staging and unstaging picked lines, checked against `git apply --cached`: for random files,
//! random edits, and random picks, the index egit makes is the one git makes from the same
//! lines as a patch.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use bstr::{BStr, ByteSlice as _};
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, DiffLineKind, DiffOptions, Direction, FileDiff, LinesError, Repository, Status,
    Uncommitted, Wait as _, apply_lines, uncommitted_diff, write_index_content,
};

/// Cases each way.
const CASES: u64 = 250;

/// A small deterministic generator (xorshift), so a failure names its case.
struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound.max(1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// A line from a few words, so that diffs find lines alike to keep.
    fn line(&mut self) -> String {
        const WORDS: [&str; 8] = [
            "alpha",
            "beta",
            "gamma",
            "",
            "delta",
            "{",
            "}",
            "  return x;",
        ];
        let mut line = WORDS[usize::try_from(self.below(8)).unwrap()].to_owned();
        if self.chance(10) {
            line.push('\r');
        }
        line
    }
}

/// Lines joined by newlines, the last one without when `open`.
fn join(lines: &[String], open: bool) -> Vec<u8> {
    let mut text = lines.join("\n");
    if !lines.is_empty() && !open {
        text.push('\n');
    }
    text.into_bytes()
}

/// A file and an edit of it.
fn case(random: &mut Random) -> (Vec<u8>, Vec<u8>) {
    let base: Vec<String> = (0..random.below(30)).map(|_| random.line()).collect();
    let mut edited = Vec::new();
    for line in &base {
        match random.below(10) {
            0 => {}
            1 => edited.push(random.line()),
            2 => {
                edited.push(line.clone());
                edited.push(random.line());
            }
            _ => edited.push(line.clone()),
        }
    }
    if random.chance(20) {
        edited.insert(0, random.line());
    }
    (
        join(&base, random.chance(15)),
        join(&edited, random.chance(15)),
    )
}

/// A line of a patch: its prefix, its text, and whether its file ends with it, unended.
type PatchLine<'a> = (u8, &'a [u8], bool);

/// The patch git would apply for the picked lines of `diff`, moving them as `direction` says
/// (always forward: unstaging is the patch from the index to `HEAD`).
fn picked_patch(diff: &FileDiff, direction: Direction, picked: &dyn Fn(usize) -> bool) -> Vec<u8> {
    let (removes, adds) = match direction {
        Direction::Stage => (DiffLineKind::Removed, DiffLineKind::Added),
        Direction::Unstage => (DiffLineKind::Added, DiffLineKind::Removed),
    };
    let texts: Vec<&[u8]> = diff.text.split(|&byte| byte == b'\n').collect();
    // Each hunk's old range (unchanged by picking: lines not picked out stay as context) and
    // lines.
    let mut hunks: Vec<((u32, u32), Vec<PatchLine<'_>>)> = Vec::new();
    for (index, line) in diff.lines.iter().enumerate() {
        let text = texts[index];
        let open = line.missing_newline;
        if line.kind == DiffLineKind::Hunk {
            let header = std::str::from_utf8(text).unwrap();
            let mut fields = header.split(' ');
            let (old, new) = (fields.nth(1).unwrap(), fields.next().unwrap());
            let range = |field: &str| -> (u32, u32) {
                let mut parts = field[1..].split(',');
                let start = parts.next().unwrap().parse().unwrap();
                (
                    start,
                    parts.next().map_or(1, |count| count.parse().unwrap()),
                )
            };
            let from = match direction {
                Direction::Stage => range(old),
                Direction::Unstage => range(new),
            };
            hunks.push((from, Vec::new()));
            continue;
        }
        let lines = &mut hunks.last_mut().unwrap().1;
        match line.kind {
            DiffLineKind::Context => lines.push((b' ', text, open)),
            kind if kind == removes && picked(index) => lines.push((b'-', text, open)),
            kind if kind == removes => lines.push((b' ', text, open)),
            kind if kind == adds && picked(index) => lines.push((b'+', text, open)),
            _ => {}
        }
    }
    let mut patch = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n".to_vec();
    // How many more lines the new file has than the old, before the hunk.
    let mut offset: i64 = 0;
    for ((old_start, old_count), lines) in hunks {
        // A hunk whose changes were all left out is only context, which git refuses (and
        // whose lines it would leave as they are anyway).
        if lines.iter().all(|&(prefix, _, _)| prefix == b' ') {
            continue;
        }
        let mut body = Vec::new();
        let mut new_count = 0_u32;
        for (index, &(prefix, text, open)) in lines.iter().enumerate() {
            // A line left unended that others now follow gains its newline.
            let followed = lines[index + 1..]
                .iter()
                .any(|&(later, _, _)| later != b'-');
            let mut push = |prefix: u8, open: bool| {
                body.push(prefix);
                body.extend_from_slice(text);
                body.push(b'\n');
                if open {
                    body.extend_from_slice(b"\\ No newline at end of file\n");
                }
                if prefix != b'-' {
                    new_count += 1;
                }
            };
            match (prefix, open && followed) {
                (b' ', true) => {
                    push(b'-', true);
                    push(b'+', false);
                }
                (b'+', true) => push(b'+', false),
                _ => push(prefix, open),
            }
        }
        // A range of no lines starts at the line before where it would be.
        let old_first = i64::from(old_start) + i64::from(old_count == 0);
        let new_first = old_first + offset;
        let new_start = new_first - i64::from(new_count == 0);
        offset += i64::from(new_count) - i64::from(old_count);
        let header = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n");
        patch.extend_from_slice(header.as_bytes());
        patch.extend_from_slice(&body);
    }
    patch
}

/// The index's content at `f`.
fn index_content(sandbox: &Sandbox, repo: &Path) -> Vec<u8> {
    sandbox
        .run(repo, "cat-file", &["blob", ":f"])
        .unwrap()
        .stdout
}

/// Applies `patch` to the index with git.
fn git_apply(sandbox: &Sandbox, repo: &Path, patch: &[u8], context: u32) {
    let mut command = sandbox
        .git()
        .write("apply")
        .args(["--cached", "--whitespace=nowarn"]);
    if context == 0 {
        command = command.arg("--unidiff-zero");
    }
    if let Err(error) = command
        .current_dir(repo)
        .stdin(patch.to_vec())
        .output()
        .wait()
    {
        panic!("{error}: {:?}", BStr::new(patch));
    }
}

/// Runs `CASES` cases each way: random content in `HEAD` or the index, an edit of it, a random
/// diff context, random picks.
fn run(direction: Direction) {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let cancel = CancelToken::new();
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let mut skipped = 0;
    for number in 0..CASES {
        let (base, edited) = case(&mut random);
        let context = u32::try_from(random.below(4)).unwrap();
        std::fs::write(repo.join("f"), &base).unwrap();
        sandbox.run(&repo, "add", &["f"]).unwrap();
        let which = match direction {
            Direction::Stage => Uncommitted::Unstaged,
            Direction::Unstage => {
                sandbox
                    .run(
                        &repo,
                        "commit",
                        &["--quiet", "--allow-empty", "--message", "base"],
                    )
                    .unwrap();
                std::fs::write(repo.join("f"), &edited).unwrap();
                sandbox.run(&repo, "add", &["f"]).unwrap();
                Uncommitted::Staged
            }
        };
        std::fs::write(repo.join("f"), &edited).unwrap();
        let options = DiffOptions {
            context,
            ..DiffOptions::default()
        };
        let diff = uncommitted_diff(
            sandbox.git(),
            &repository,
            &[BStr::new("f")],
            which,
            &options,
            &cancel,
        )
        .wait()
        .unwrap();
        let picks: Vec<bool> = diff.lines.iter().map(|_| random.chance(50)).collect();
        let picked = |index: usize| picks[index];
        let index = index_content(&sandbox, &repo);
        let ours = apply_lines(&index, &diff, direction, picked)
            .unwrap_or_else(|error| panic!("case {number}: {error}"));
        if diff.lines.is_empty() {
            assert_eq!(ours, index, "case {number}: nothing to apply");
            continue;
        }
        let patch = picked_patch(&diff, direction, &picked);
        let theirs = if !patch.contains_str("@@") {
            // Nothing picked: the index stays as it is.
            index.clone()
        } else if context == 0 && patch.contains_str("\\ No newline") {
            // Without context, `git apply` is confused about which line ends the file (its
            // documentation warns as much): those cases have no reference to compare with.
            skipped += 1;
            continue;
        } else {
            git_apply(&sandbox, &repo, &patch, context);
            index_content(&sandbox, &repo)
        };
        assert_eq!(
            BStr::new(&ours),
            BStr::new(&theirs),
            "case {number} ({direction:?}, context {context}): base {:?}, edited {:?}, \
             picks {picks:?}, patch {:?}",
            BStr::new(&base),
            BStr::new(&edited),
            BStr::new(&patch),
        );
    }
    assert!(
        skipped * 10 < CASES,
        "{skipped} cases had nothing to compare with"
    );
}

#[test]
fn staging_picked_lines_matches_git_apply() {
    run(Direction::Stage);
}

#[test]
fn unstaging_picked_lines_matches_git_apply() {
    run(Direction::Unstage);
}

#[test]
fn writes_content_over_the_index_entry_it_was_made_from() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "f", "one\ntwo\n", "Add")
        .unwrap();
    std::fs::write(repo.join("f"), "one\nTWO\nthree\n").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let cancel = CancelToken::new();
    let status = Status::read(sandbox.git(), &repository, &cancel)
        .wait()
        .unwrap();
    let entry = status.entry(0).unwrap().index();

    let diff = uncommitted_diff(
        sandbox.git(),
        &repository,
        &[BStr::new("f")],
        Uncommitted::Unstaged,
        &DiffOptions::default(),
        &cancel,
    )
    .wait()
    .unwrap();
    // Only the new last line.
    let last = diff.lines.len() - 1;
    let content = apply_lines(b"one\ntwo\n", &diff, Direction::Stage, |index| {
        index == last
    })
    .unwrap();
    write_index_content(
        sandbox.git(),
        &repository,
        BStr::new("f"),
        entry,
        0o100_644,
        content,
        &cancel,
    )
    .wait()
    .unwrap();
    assert_eq!(index_content(&sandbox, &repo), b"one\ntwo\nthree\n");

    // The entry changed since: nothing is written.
    let stale = write_index_content(
        sandbox.git(),
        &repository,
        BStr::new("f"),
        entry,
        0o100_644,
        b"other\n".to_vec(),
        &cancel,
    );
    assert!(matches!(stale.wait(), Err(LinesError::Stale)));
    assert_eq!(index_content(&sandbox, &repo), b"one\ntwo\nthree\n");

    // A file not yet tracked is written where nothing was.
    std::fs::write(repo.join("new"), "a\nb\n").unwrap();
    let diff = uncommitted_diff(
        sandbox.git(),
        &repository,
        &[BStr::new("new")],
        Uncommitted::Untracked,
        &DiffOptions::default(),
        &cancel,
    )
    .wait()
    .unwrap();
    let content = apply_lines(b"", &diff, Direction::Stage, |index| index == 2).unwrap();
    write_index_content(
        sandbox.git(),
        &repository,
        BStr::new("new"),
        None,
        0o100_644,
        content,
        &cancel,
    )
    .wait()
    .unwrap();
    let staged = sandbox.run(&repo, "cat-file", &["blob", ":new"]).unwrap();
    assert_eq!(staged.stdout, b"b\n");
}
