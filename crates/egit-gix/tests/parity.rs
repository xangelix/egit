//! gitoxide's reads against git's, on sandboxed repositories made to be awkward: branches, tags
//! of every kind (lightweight, annotated, a tag of a tag, tags of a tree and of a blob), remote
//! branches and their symbolic `HEAD`, a stash, notes, another worktree, refs packed and loose;
//! on random histories; and on a partial clone, whose missing objects git fetches. Each read
//! must give exactly what egit's git commands give, and a repository gix wouldn't read as git
//! does mustn't be opened.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers;
// and git's reads beside gix's, named alike.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::similar_names
)]

use std::path::Path;

use bstr::ByteSlice as _;
use egit_git::testing::Sandbox;
use std::ops::ControlFlow;

use egit_git::{
    CancelToken, ObjectId, ObjectKind, ObjectReader, Reference, Repository, Wait as _,
    list_references, walk_history, walk_history_from,
};
use egit_gix::GixRepository;

/// What `git rev-list --no-walk=unsorted --exclude=refs/stash --all` lists.
fn git_tips(sandbox: &Sandbox, repo: &Path) -> Vec<ObjectId> {
    let listed = sandbox
        .run(
            repo,
            "rev-list",
            &["--no-walk=unsorted", "--exclude=refs/stash", "--all"],
        )
        .unwrap();
    listed
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| ObjectId::from_hex(line).unwrap())
        .collect()
}

fn sorted(mut references: Vec<Reference>) -> Vec<Reference> {
    references.sort_by(|a, b| a.name.cmp(&b.name));
    references
}

/// An object as read: its kind and contents, or `None` when it isn't there.
type Read = Option<(ObjectKind, Vec<u8>)>;

/// Every object in `repo`, as `cat-file --batch-all-objects` lists them.
fn all_objects(sandbox: &Sandbox, repo: &Path) -> Vec<ObjectId> {
    let all = sandbox
        .run(
            repo,
            "cat-file",
            &["--batch-all-objects", "--batch-check=%(objectname)"],
        )
        .unwrap();
    all.stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| ObjectId::from_hex(line).unwrap())
        .collect()
}

/// `ids` read in `repo` by `cat-file`.
fn read_by_git(sandbox: &Sandbox, repo: &Path, ids: &[ObjectId]) -> Vec<Read> {
    let mut read = Vec::new();
    let mut reader = ObjectReader::start(sandbox.git(), repo).wait().unwrap();
    reader
        .read(ids, |_, object| {
            read.push(object.map(|object| (object.kind, object.data.to_vec())));
        })
        .wait()
        .unwrap();
    assert_eq!(read.len(), ids.len());
    read
}

/// `ids` read in `repo` as egit reads them: by gix, then by git from where gix stopped.
fn read_by_gix(sandbox: &Sandbox, repo: &Path, gix: &GixRepository, ids: &[ObjectId]) -> Vec<Read> {
    let mut read = Vec::new();
    let handed = gix
        .read(ids, |_, object| {
            read.push(object.map(|object| (object.kind, object.data.to_vec())));
        })
        .unwrap();
    assert_eq!(handed, read.len());
    if handed < ids.len() {
        read.extend(read_by_git(sandbox, repo, &ids[handed..]));
    }
    read
}

/// Every object in `repo`, as cat-file reads it and as gix reads it, compared; and a missing
/// one missing from both.
fn same_objects(sandbox: &Sandbox, repo: &Path, gix: &GixRepository) {
    let mut ids = all_objects(sandbox, repo);
    ids.push(ObjectId::from_hex(&[b'0'; 40]).unwrap());
    let ours = read_by_git(sandbox, repo, &ids);
    let theirs = read_by_gix(sandbox, repo, gix, &ids);
    for (index, (ours, theirs)) in ours.iter().zip(&theirs).enumerate() {
        assert_eq!(ours, theirs, "object {}", ids[index]);
    }
}

/// Every read compared in `repo`.
fn same_reads(sandbox: &Sandbox, repo: &Path) {
    let gix = GixRepository::open(repo).expect("gix opens it");
    same_references(sandbox, repo, &gix);
    same_objects(sandbox, repo, &gix);
}

/// The references, the walk's tips, and the walk from them compared in `repo`.
fn same_references(sandbox: &Sandbox, repo: &Path, gix: &GixRepository) {
    let repository = Repository::discover(sandbox.git(), repo).wait().unwrap();
    assert_eq!(
        sorted(gix.references().unwrap()),
        sorted(list_references(sandbox.git(), &repository).wait().unwrap())
    );
    assert_eq!(gix.tips().unwrap(), git_tips(sandbox, repo));
    // The walk from gix's tips is git's own walk.
    let walk = |from_gix: bool| {
        let mut walked: Vec<(ObjectId, Vec<ObjectId>)> = Vec::new();
        let each = |id: ObjectId, parents: &[ObjectId]| {
            walked.push((id, parents.to_vec()));
            ControlFlow::Continue(())
        };
        if from_gix {
            walk_history_from(
                sandbox.git(),
                &repository,
                gix.tips().unwrap(),
                &CancelToken::new(),
                each,
            )
            .wait()
        } else {
            walk_history(sandbox.git(), &repository, &CancelToken::new(), each).wait()
        }
        .unwrap();
        walked
    };
    assert_eq!(walk(true), walk(false));
}

/// A history with every kind of reference, packed and loose.
fn awkward(sandbox: &Sandbox) -> std::path::PathBuf {
    let origin = sandbox.init("origin").unwrap();
    sandbox.commit_file(&origin, "a", "a\n", "Start").unwrap();
    sandbox
        .run(&origin, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox.commit_file(&origin, "s", "s\n", "Side").unwrap();
    sandbox
        .run(&origin, "switch", &["--quiet", "main"])
        .unwrap();
    let repo = sandbox.path("repo");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", origin.to_str().unwrap(), repo.to_str().unwrap()],
        )
        .unwrap();
    sandbox.commit_file(&repo, "b", "b\n", "Second").unwrap();
    for args in [
        &["tag", "light"][..],
        &["tag", "--annotate", "--message", "Annotated", "annotated"],
        &[
            "tag",
            "--annotate",
            "--message",
            "Of a tag",
            "nested",
            "annotated",
        ],
        &["tag", "tree", "HEAD^{tree}"],
        &[
            "tag",
            "--annotate",
            "--message",
            "Of a blob",
            "blob",
            "HEAD:b",
        ],
        &["branch", "topic", "HEAD~1"],
    ] {
        sandbox.run(&repo, args[0], &args[1..]).unwrap();
    }
    // A stash, notes, and refs packed, then more loose ones.
    std::fs::write(repo.join("b"), "changed\n").unwrap();
    sandbox.run(&repo, "stash", &["--quiet"]).unwrap();
    sandbox
        .run(&repo, "notes", &["add", "--message", "A note", "HEAD~1"])
        .unwrap();
    sandbox.run(&repo, "pack-refs", &["--all"]).unwrap();
    sandbox.run(&repo, "branch", &["loose", "HEAD"]).unwrap();
    // Another worktree, detached at the first commit.
    let other = sandbox.path("other");
    sandbox
        .run(
            &repo,
            "worktree",
            &[
                "add",
                "--quiet",
                "--detach",
                other.to_str().unwrap(),
                "HEAD~1",
            ],
        )
        .unwrap();
    sandbox
        .commit_file(&other, "o", "o\n", "Only in the other worktree")
        .unwrap();
    repo
}

#[test]
fn every_kind_of_reference_reads_as_git_reads_it() {
    let sandbox = Sandbox::new().unwrap();
    let repo = awkward(&sandbox);
    same_reads(&sandbox, &repo);
    // From the other worktree, too.
    same_reads(&sandbox, &sandbox.path("other"));
}

#[test]
fn replaced_objects_are_left_to_git() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "a\n", "One").unwrap();
    sandbox.commit_file(&repo, "a", "b\n", "Two").unwrap();
    assert!(GixRepository::open(&repo).is_some());
    sandbox.run(&repo, "replace", &["HEAD~1", "HEAD"]).unwrap();
    assert!(GixRepository::open(&repo).is_none());
}

/// An index below `len`, by `random`.
fn pick(random: &mut impl FnMut(u64) -> u64, len: usize) -> usize {
    usize::try_from(random(u64::try_from(len).unwrap())).unwrap()
}

#[test]
fn random_histories_read_as_git_reads_them() {
    let mut seed = 0x5eed_u64;
    let mut random = |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    for case in 0..50 {
        let sandbox = Sandbox::new().unwrap();
        let repo = sandbox.init("repo").unwrap();
        sandbox
            .commit_file(&repo, "f0", "start\n", "Start")
            .unwrap();
        let mut branches = vec!["main".to_owned()];
        for step in 0..(5 + random(20)) {
            match random(6) {
                0 => {
                    let name = format!("b{step}");
                    sandbox.run(&repo, "branch", &[&name]).unwrap();
                    branches.push(name);
                }
                1 => {
                    let branch = &branches[pick(&mut random, branches.len())];
                    sandbox.run(&repo, "switch", &["--quiet", branch]).unwrap();
                }
                2 => {
                    let tag = format!("t{step}");
                    let mut args = vec!["--message", "A tag", "--annotate", &tag];
                    if random(2) == 0 {
                        args.clear();
                        args.push(&tag);
                    }
                    sandbox.run(&repo, "tag", &args).unwrap();
                }
                3 if branches.len() > 1 => {
                    let other = &branches[pick(&mut random, branches.len())];
                    let _merged = sandbox.run(
                        &repo,
                        "merge",
                        &["--quiet", "--no-edit", "--allow-unrelated-histories", other],
                    );
                    let _ = sandbox.run(&repo, "merge", &["--abort"]);
                }
                4 => {
                    sandbox.run(&repo, "pack-refs", &["--all"]).unwrap();
                    sandbox.run(&repo, "gc", &["--quiet"]).unwrap();
                    if random(2) == 0 {
                        sandbox
                            .run(&repo, "commit-graph", &["write", "--reachable"])
                            .unwrap();
                    }
                }
                _ => {
                    sandbox
                        .commit_file(
                            &repo,
                            &format!("f{}", random(4)),
                            &format!("{step} {case}\n"),
                            &format!("Commit {step}"),
                        )
                        .unwrap();
                }
            }
        }
        same_reads(&sandbox, &repo);
    }
}

#[test]
fn a_partial_clone_reads_what_it_has_and_leaves_git_the_rest() {
    let sandbox = Sandbox::new().unwrap();
    let origin = sandbox.init("origin").unwrap();
    for (step, file) in ["a", "b", "a", "c"].into_iter().enumerate() {
        sandbox
            .commit_file(
                &origin,
                file,
                &format!("{step}\n"),
                &format!("Commit {step}"),
            )
            .unwrap();
    }
    sandbox
        .run(
            &origin,
            "tag",
            &["--annotate", "--message", "Old", "old", "HEAD~2"],
        )
        .unwrap();
    sandbox
        .run(&origin, "config", &["uploadpack.allowFilter", "true"])
        .unwrap();
    let repo = sandbox.path("partial");
    let url = format!("file://{}", origin.display());
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &[
                "--quiet",
                "--filter=blob:none",
                "--no-checkout",
                &url,
                repo.to_str().unwrap(),
            ],
        )
        .unwrap();
    let gix = GixRepository::open(&repo).expect("gix opens a partial clone");
    same_references(&sandbox, &repo, &gix);
    // Every object the origin has, though the clone holds no blobs: gix stops at the first
    // missing one, and git fetches the rest as it reads them.
    let ids = all_objects(&sandbox, &origin);
    let missing = ids.len() - all_objects(&sandbox, &repo).len();
    assert!(missing > 0, "the clone has every object");
    let expected = read_by_git(&sandbox, &origin, &ids);
    assert!(expected.iter().all(Option::is_some));
    assert_eq!(read_by_gix(&sandbox, &repo, &gix, &ids), expected);
}

#[test]
fn reftable_references_are_left_to_git() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.path("reftable");
    let made = sandbox.run(
        sandbox.home(),
        "init",
        &["--quiet", "--ref-format=reftable", repo.to_str().unwrap()],
    );
    // Git before 2.45 has no reftable.
    if made.is_err() {
        return;
    }
    sandbox.commit_file(&repo, "a", "a\n", "One").unwrap();
    assert!(GixRepository::open(&repo).is_none());
}
