//! Conflicts from a merge, a rebase, a cherry-pick, and a revert: their sides read, merged as
//! git merges them, and resolved; and on random edits, taking one side everywhere gives what
//! `merge-file --ours` or `--theirs` gives.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, ConflictSides, GitVersion, IncomingCommit, Merge, MergedText, ObjectId, Operation,
    Pick, Repository, Resolution, ResolveError, TreeEntry, WholeOnly, conflict_sides,
    incoming_commit, merge_sides, resolve_conflict,
};

/// Random cases.
const CASES: u64 = 250;

/// A repository on `main` with `file` holding `base`, and a branch `side` changing it to
/// `incoming` while `main` changes it to `current`: merging `side` stops at a conflict.
fn conflicted(sandbox: &Sandbox, file: &str, base: &str, current: &str, incoming: &str) -> Repo {
    conflicted_in(sandbox, "repo", file, [base, current, incoming])
}

/// As [`conflicted`], in a repository called `name`.
fn conflicted_in(
    sandbox: &Sandbox,
    name: &str,
    file: &str,
    [base, current, incoming]: [&str; 3],
) -> Repo {
    let path = sandbox.init(name).unwrap();
    sandbox.commit_file(&path, file, base, "Base").unwrap();
    sandbox
        .run(&path, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&path, file, incoming, "Incoming change")
        .unwrap();
    sandbox.run(&path, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&path, file, current, "Current change")
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
    Repo { path, repository }
}

struct Repo {
    path: std::path::PathBuf,
    repository: Repository,
}

/// Runs git expecting it to fail (a merge or pick stopping at a conflict).
fn stops(sandbox: &Sandbox, repo: &Path, subcommand: &'static str, args: &[&str]) {
    let done = sandbox
        .git()
        .write(subcommand)
        .args(args)
        .current_dir(repo)
        .output_unchecked()
        .unwrap();
    assert!(!done.status.success(), "{subcommand} {args:?} didn't stop");
}

fn sides(sandbox: &Sandbox, repo: &Repo, file: &str) -> ConflictSides {
    conflict_sides(
        sandbox.git(),
        &repo.repository,
        BStr::new(file),
        &CancelToken::new(),
    )
    .unwrap()
    .unwrap()
}

fn merged(sandbox: &Sandbox, repo: &Repo, sides: &ConflictSides) -> MergedText {
    match merge_sides(sandbox.git(), &repo.repository, sides, &CancelToken::new()).unwrap() {
        Merge::Text(text) => text,
        Merge::WholeOnly(why) => panic!("not two texts: {why:?}"),
    }
}

fn incoming(sandbox: &Sandbox, repo: &Repo, operation: Operation) -> Option<IncomingCommit> {
    incoming_commit(
        sandbox.git(),
        &repo.repository,
        operation,
        &CancelToken::new(),
    )
    .unwrap()
}

fn resolve(
    sandbox: &Sandbox,
    repo: &Repo,
    file: &str,
    sides: &ConflictSides,
    resolution: Resolution,
) -> Result<(), ResolveError> {
    resolve_conflict(
        sandbox.git(),
        &repo.repository,
        BStr::new(file),
        sides,
        resolution,
        &CancelToken::new(),
    )
}

/// What `merge-file` makes of three blobs, taking `side` (`--ours` or `--theirs`) in conflicts,
/// with the diff `merge_sides` uses (git's merge's, where `merge-file` has it).
fn merge_file(sandbox: &Sandbox, repo: &Path, ids: [ObjectId; 3], side: &str) -> Vec<u8> {
    let [current, base, incoming] = ids.map(|id| id.to_string());
    let histogram = sandbox.git().version() >= GitVersion::new(2, 44, 0);
    sandbox
        .git()
        .read("merge-file")
        .args(histogram.then_some("--diff-algorithm=histogram"))
        .args(["--object-id", "-p", side, &current, &base, &incoming])
        .current_dir(repo)
        .output()
        .unwrap()
        .stdout
}

fn blob(sandbox: &Sandbox, repo: &Path, content: &[u8]) -> ObjectId {
    let hashed = sandbox
        .git()
        .write("hash-object")
        .args(["-w", "--stdin"])
        .current_dir(repo)
        .stdin(content.to_vec())
        .output()
        .unwrap();
    ObjectId::from_hex(hashed.stdout.trim_ascii()).unwrap()
}

fn staged(sandbox: &Sandbox, repo: &Path, file: &str) -> Vec<u8> {
    sandbox
        .run(repo, "cat-file", &["blob", &format!(":0:{file}")])
        .unwrap()
        .stdout
}

#[test]
fn a_merge_conflict_is_merged_resolved_and_committed() {
    let sandbox = Sandbox::new().unwrap();
    let repo = conflicted(
        &sandbox,
        "notes.txt",
        "a\nb\nc\nd\ne\n",
        "a\nB1\nc\nd\nE1",
        "a\nB2\nc\nd\nE2\n",
    );
    stops(&sandbox, &repo.path, "merge", &["--quiet", "side"]);
    let sides = sides(&sandbox, &repo, "notes.txt");
    assert!(sides.base.is_some() && sides.current.is_some() && sides.incoming.is_some());
    let commit = incoming(&sandbox, &repo, Operation::Merge).unwrap();
    assert_eq!(commit.subject, "Incoming change");
    assert_eq!(commit.branches, ["side"]);
    assert_eq!(incoming(&sandbox, &repo, Operation::CherryPick), None);

    let text = merged(&sandbox, &repo, &sides);
    assert_eq!(text.conflicts(), 2);
    // The current side's file ends without a newline; so does a resolution ending with it.
    assert_eq!(text.resolve(|_| Pick::Current), b"a\nB1\nc\nd\nE1");
    let picks = [Pick::Incoming, Pick::CurrentThenIncoming];
    let resolved = text.resolve(|index| picks[index]);
    assert_eq!(resolved, b"a\nB2\nc\nd\nE1\nE2\n");

    let mode = sides.merged_mode().unwrap();
    resolve(
        &sandbox,
        &repo,
        "notes.txt",
        &sides,
        Resolution::Content {
            content: resolved.clone(),
            mode,
        },
    )
    .unwrap();
    // Resolved: no sides left, the index holds it, and so does the work tree.
    let cancel = CancelToken::new();
    let left = conflict_sides(
        sandbox.git(),
        &repo.repository,
        BStr::new("notes.txt"),
        &cancel,
    );
    assert_eq!(left.unwrap(), None);
    assert_eq!(staged(&sandbox, &repo.path, "notes.txt"), resolved);
    assert_eq!(
        std::fs::read(repo.path.join("notes.txt")).unwrap(),
        resolved
    );
    // Again, from the sides read before: they are gone.
    let again = resolve(&sandbox, &repo, "notes.txt", &sides, Resolution::Delete);
    assert!(matches!(again, Err(ResolveError::Stale)), "{again:?}");
    sandbox
        .run(&repo.path, "commit", &["--quiet", "--no-edit"])
        .unwrap();
    let parents = sandbox
        .run(&repo.path, "log", &["-1", "--format=%P"])
        .unwrap();
    assert_eq!(parents.stdout.split(|&byte| byte == b' ').count(), 2);
}

#[test]
fn a_deleted_side_binary_contents_and_a_link_resolve_only_whole() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "gone", "kept\n", "Base")
        .unwrap();
    sandbox
        .commit_file(&repo, "data", "a\0\n", "Binary")
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "gone", "changed\n", "Change")
        .unwrap();
    sandbox
        .commit_file(&repo, "data", "b\0\n", "Theirs")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.run(&repo, "rm", &["--quiet", "gone"]).unwrap();
    sandbox.commit_file(&repo, "data", "c\0\n", "Ours").unwrap();
    stops(&sandbox, &repo, "merge", &["--quiet", "side"]);
    let repo = Repo {
        repository: Repository::discover(sandbox.git(), &repo).unwrap(),
        path: repo,
    };
    let cancel = CancelToken::new();
    let merge = |sides: &ConflictSides| {
        merge_sides(sandbox.git(), &repo.repository, sides, &cancel).unwrap()
    };

    let gone = sides(&sandbox, &repo, "gone");
    assert_eq!(gone.current, None);
    assert_eq!(merge(&gone), Merge::WholeOnly(WholeOnly::Deleted));
    // Kept as the incoming side changed it.
    let incoming = gone.incoming.unwrap();
    resolve(&sandbox, &repo, "gone", &gone, Resolution::Side(incoming)).unwrap();
    assert_eq!(std::fs::read(repo.path.join("gone")).unwrap(), b"changed\n");

    let data = sides(&sandbox, &repo, "data");
    assert_eq!(merge(&data), Merge::WholeOnly(WholeOnly::Binary));
    resolve(&sandbox, &repo, "data", &data, Resolution::Delete).unwrap();
    assert!(!repo.path.join("data").exists());
    let status = sandbox
        .run(&repo.path, "status", &["--porcelain=v2"])
        .unwrap();
    assert!(!status.stdout.starts_with(b"u "), "{status:?}");

    let link = ConflictSides {
        incoming: Some(TreeEntry {
            mode: 0o120_000,
            ..incoming
        }),
        ..data
    };
    assert_eq!(merge(&link), Merge::WholeOnly(WholeOnly::Link));
}

#[test]
fn picked_rebased_and_reverted_conflicts_name_their_commit_and_continue() {
    let sandbox = Sandbox::new().unwrap();
    let texts = ["base\n", "current\n", "incoming\n"];
    // Resolves `f` with its incoming side (so nothing is left empty), carries the operation
    // on, and says what came in.
    let resolve_and_continue = |repo: &Repo, operation, subcommand| {
        let sides = sides(&sandbox, repo, "f");
        let commit = incoming(&sandbox, repo, operation).unwrap();
        let content = merged(&sandbox, repo, &sides).resolve(|_| Pick::Incoming);
        let mode = sides.merged_mode().unwrap();
        resolve(
            &sandbox,
            repo,
            "f",
            &sides,
            Resolution::Content { content, mode },
        )
        .unwrap();
        sandbox
            .run(&repo.path, subcommand, &["--continue"])
            .unwrap();
        commit.subject
    };

    let repo = conflicted_in(&sandbox, "picked", "f", texts);
    stops(&sandbox, &repo.path, "cherry-pick", &["side"]);
    let picked = resolve_and_continue(&repo, Operation::CherryPick, "cherry-pick");
    assert_eq!(picked, "Incoming change");

    // `main`'s change replayed onto `side`.
    let repo = conflicted_in(&sandbox, "rebased", "f", texts);
    stops(&sandbox, &repo.path, "rebase", &["--quiet", "side"]);
    let replayed = resolve_and_continue(&repo, Operation::Rebase, "rebase");
    assert_eq!(replayed, "Current change");

    // Undoing `main`'s change after another one over it.
    let repo = conflicted_in(&sandbox, "reverted", "f", texts);
    sandbox
        .commit_file(&repo.path, "f", "again\n", "Again")
        .unwrap();
    stops(&sandbox, &repo.path, "revert", &["--no-edit", "HEAD~1"]);
    let reverted = resolve_and_continue(&repo, Operation::Revert, "revert");
    assert_eq!(reverted, "Current change");
    let head = sandbox
        .run(&repo.path, "log", &["-1", "--format=%s"])
        .unwrap();
    assert!(head.stdout.starts_with(b"Revert"), "{head:?}");
}

#[test]
fn files_added_on_both_sides_merge_from_nothing() {
    let sandbox = Sandbox::new().unwrap();
    let path = sandbox.init("repo").unwrap();
    sandbox.commit_file(&path, "base", "x\n", "Base").unwrap();
    sandbox
        .run(&path, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&path, "new", "same\ntheirs\n", "Theirs")
        .unwrap();
    sandbox.run(&path, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .commit_file(&path, "new", "same\nours\n", "Ours")
        .unwrap();
    stops(&sandbox, &path, "merge", &["--quiet", "side"]);
    let repo = Repo {
        repository: Repository::discover(sandbox.git(), &path).unwrap(),
        path,
    };
    let sides = sides(&sandbox, &repo, "new");
    assert_eq!(sides.base, None);
    let text = merged(&sandbox, &repo, &sides);
    assert_eq!(text.resolve(|_| Pick::Incoming), b"same\ntheirs\n");
    assert_eq!(text.resolve(|_| Pick::Current), b"same\nours\n");
}

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

    /// A line from a few words (so the sides share lines), now and then one like a marker.
    fn line(&mut self) -> String {
        const WORDS: [&str; 10] = [
            "alpha",
            "beta",
            "gamma",
            "",
            "delta",
            "{",
            "}",
            "<<<<<<< not a marker",
            "=======",
            "||||||||||| neither",
        ];
        WORDS[usize::try_from(self.below(10)).unwrap()].to_owned()
    }

    fn text(&mut self, lines: &[String]) -> Vec<u8> {
        let mut text = lines.join("\n").into_bytes();
        if !lines.is_empty() && self.chance(80) {
            text.push(b'\n');
        }
        text
    }

    /// `lines` changed here and there: lines replaced, inserted, and removed.
    fn edit(&mut self, lines: &[String]) -> Vec<String> {
        let mut edited = Vec::new();
        for line in lines {
            match self.below(10) {
                0 => {}
                1 => edited.push(self.line()),
                2 => {
                    edited.push(line.clone());
                    edited.push(self.line());
                }
                _ => edited.push(line.clone()),
            }
        }
        if self.chance(20) {
            edited.push(self.line());
        }
        edited
    }
}

#[test]
fn one_side_taken_everywhere_is_what_merge_file_takes() {
    let sandbox = Sandbox::new().unwrap();
    let path = sandbox.init("repo").unwrap();
    let repo = Repo {
        repository: Repository::discover(sandbox.git(), &path).unwrap(),
        path,
    };
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut conflicts = 0;
    for case in 0..CASES {
        let length = random.below(30);
        let base: Vec<String> = (0..length).map(|_| random.line()).collect();
        let current = random.edit(&base);
        let incoming = random.edit(&base);
        let texts = [
            random.text(&current),
            random.text(&base),
            random.text(&incoming),
        ];
        let ids = texts.clone().map(|text| blob(&sandbox, &repo.path, &text));
        let entry = |id| {
            Some(TreeEntry {
                mode: 0o100_644,
                id,
            })
        };
        let sides = ConflictSides {
            base: entry(ids[1]),
            current: entry(ids[0]),
            incoming: entry(ids[2]),
        };
        let text = merged(&sandbox, &repo, &sides);
        conflicts += text.conflicts();
        assert_eq!(
            BStr::new(&text.resolve(|_| Pick::Current)),
            BStr::new(&merge_file(&sandbox, &repo.path, ids, "--ours")),
            "case {case}: {texts:?}"
        );
        assert_eq!(
            BStr::new(&text.resolve(|_| Pick::Incoming)),
            BStr::new(&merge_file(&sandbox, &repo.path, ids, "--theirs")),
            "case {case}: {texts:?}"
        );
    }
    // The cases conflict, mostly.
    assert!(conflicts > usize::try_from(CASES).unwrap(), "{conflicts}");
}
