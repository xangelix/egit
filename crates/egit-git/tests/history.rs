//! Walking history, listing refs, reading objects, and a commit's changes, against the real git.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::Path;

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, ChangeKind, Commit, ObjectId, ObjectKind, ObjectReader, RefKind, Repository,
    commit_changes, list_references, walk_history, write_commit_graph,
};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// `main`: A–B–C–M, where M merges `topic` (B–D–E) into C.
fn branchy(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "a\n", "A").unwrap();
    sandbox.commit_file(&repo, "b", "b\n", "B").unwrap();
    sandbox.run(&repo, "branch", &["topic"]).unwrap();
    sandbox.commit_file(&repo, "c", "c\n", "C").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "topic"]).unwrap();
    sandbox.commit_file(&repo, "d", "d\n", "D").unwrap();
    sandbox.commit_file(&repo, "e", "e\n", "E").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &["--quiet", "--no-ff", "--message", "M", "topic"],
        )
        .unwrap();
    repo
}

fn walk(sandbox: &Sandbox, repo: &Path) -> Vec<(ObjectId, Vec<ObjectId>)> {
    let repository = Repository::discover(sandbox.git(), repo).unwrap();
    let mut commits = Vec::new();
    let walked = walk_history(
        sandbox.git(),
        &repository,
        &CancelToken::new(),
        |id, parents| {
            commits.push((id, parents.to_vec()));
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(walked.commits, u64::try_from(commits.len()).unwrap());
    assert!(!walked.stopped);
    commits
}

#[test]
fn walks_every_commit_children_first() {
    let sandbox = sandbox();
    let repo = branchy(&sandbox);
    let commits = walk(&sandbox, &repo);
    assert_eq!(commits.len(), 6);
    let position: HashMap<ObjectId, usize> = commits
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, index))
        .collect();
    for (id, parents) in &commits {
        for parent in parents {
            assert!(position[id] < position[parent], "a parent came first");
        }
    }
    let merge = rev(&sandbox, &repo, "main");
    let (first, merge_parents) = &commits[0];
    assert_eq!(*first, merge);
    assert_eq!(
        merge_parents,
        &[
            rev(&sandbox, &repo, "main^1"),
            rev(&sandbox, &repo, "main^2")
        ]
    );
    let root = rev(&sandbox, &repo, "main~2^1");
    assert_eq!(commits.last().unwrap(), &(root, Vec::new()));
}

#[test]
fn leaves_out_stashes() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "file", "one\n", "One").unwrap();
    std::fs::write(repo.join("file"), "changed\n").unwrap();
    sandbox.run(&repo, "stash", &["--quiet"]).unwrap();
    assert_eq!(walk(&sandbox, &repo).len(), 1);
}

#[test]
fn an_empty_repository_has_no_history() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    assert_eq!(walk(&sandbox, &repo), []);
}

#[test]
fn a_walk_can_stop_early() {
    let sandbox = sandbox();
    let repo = branchy(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let walked = walk_history(sandbox.git(), &repository, &CancelToken::new(), |_, _| {
        ControlFlow::Break(())
    })
    .unwrap();
    assert_eq!(walked.commits, 1);
    assert!(walked.stopped);
}

#[test]
fn lists_refs_with_tags_followed_to_their_commits() {
    let sandbox = sandbox();
    let repo = branchy(&sandbox);
    sandbox.run(&repo, "tag", &["light", "topic"]).unwrap();
    sandbox
        .run(
            &repo,
            "tag",
            &["--annotate", "--message", "Release", "v1.0", "main~1"],
        )
        .unwrap();
    let main = rev(&sandbox, &repo, "main").to_string();
    sandbox
        .run(&repo, "update-ref", &["refs/remotes/origin/main", &main])
        .unwrap();
    sandbox
        .run(
            &repo,
            "symbolic-ref",
            &["refs/remotes/origin/HEAD", "refs/remotes/origin/main"],
        )
        .unwrap();

    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let references = list_references(sandbox.git(), &repository).unwrap();
    let find = |name: &str| references.iter().find(|reference| reference.name == name);
    assert!(
        find("refs/remotes/origin/HEAD").is_none(),
        "symbolic refs are left out"
    );
    let main_ref = find("refs/heads/main").unwrap();
    assert_eq!(main_ref.kind, RefKind::Branch);
    assert_eq!(main_ref.short_name(), "main");
    assert_eq!(
        find("refs/heads/topic").unwrap().target,
        rev(&sandbox, &repo, "topic")
    );
    assert_eq!(
        find("refs/remotes/origin/main").unwrap().kind,
        RefKind::RemoteBranch
    );
    let annotated = find("refs/tags/v1.0").unwrap();
    assert_eq!(annotated.kind, RefKind::Tag);
    assert_eq!(
        annotated.target,
        rev(&sandbox, &repo, "main~1"),
        "the tag is followed to its commit"
    );
    assert_eq!(
        find("refs/tags/light").unwrap().target,
        rev(&sandbox, &repo, "topic")
    );
}

#[test]
fn reads_objects_through_one_process() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    // More commits than one batch of requests, through fast-import.
    let mut stream = String::new();
    for index in 1..=300 {
        let message = format!("Commit number {index}\n");
        let _ = write!(
            stream,
            "commit refs/heads/main\nmark :{index}\ncommitter Test <t@example.com> {} +0000\ndata {}\n{message}",
            1_700_000_000 + index,
            message.len()
        );
        if index > 1 {
            let _ = writeln!(stream, "from :{}", index - 1);
        }
        stream.push('\n');
    }
    sandbox
        .git()
        .write("fast-import")
        .arg("--quiet")
        .stdin(stream.into_bytes())
        .current_dir(&repo)
        .output()
        .unwrap();

    let commits: Vec<ObjectId> = walk(&sandbox, &repo)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(commits.len(), 300);
    let mut reader = ObjectReader::start(sandbox.git(), &repo).unwrap();
    let missing = ObjectId::from_hex(b"0123456789012345678901234567890123456789").unwrap();
    let mut asked = commits.clone();
    asked.insert(150, missing);
    let mut subjects = Vec::new();
    reader
        .read(&asked, |id, object| match object {
            Some(object) => {
                assert_eq!(object.kind, ObjectKind::Commit);
                subjects.push((
                    id,
                    Commit::parse(object.data).unwrap().subject().to_string(),
                ));
            }
            None => subjects.push((id, "missing".to_owned())),
        })
        .unwrap();
    assert_eq!(subjects.len(), 301);
    assert_eq!(subjects[0].1, "Commit number 300");
    assert_eq!(subjects[150], (missing, "missing".to_owned()));
    assert_eq!(subjects[300].1, "Commit number 1");

    // The same process answers again.
    let mut again = None;
    reader
        .read(&commits[..1], |_, object| {
            again = object.map(|object| Commit::parse(object.data).unwrap().committer.time);
        })
        .unwrap();
    assert_eq!(again, Some(1_700_000_300));
}

#[test]
fn lists_what_a_commit_changed() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(
            &repo,
            "old name",
            "a long enough file to be recognised when renamed\n",
            "Start",
        )
        .unwrap();
    sandbox.commit_file(&repo, "keep", "one\n", "Keep").unwrap();
    sandbox.run(&repo, "mv", &["old name", "new name"]).unwrap();
    std::fs::write(repo.join("keep"), "two\n").unwrap();
    std::fs::write(repo.join("added"), "new\n").unwrap();
    sandbox.run(&repo, "add", &["--all"]).unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--message", "Change"])
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();

    let head = rev(&sandbox, &repo, "HEAD");
    let parent = rev(&sandbox, &repo, "HEAD^");
    let mut changes = commit_changes(sandbox.git(), &repository, head, Some(parent)).unwrap();
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let kinds: Vec<(String, ChangeKind)> = changes
        .into_iter()
        .map(|change| (change.path.to_string(), change.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("added".to_owned(), ChangeKind::Added),
            ("keep".to_owned(), ChangeKind::Modified),
            (
                "new name".to_owned(),
                ChangeKind::Renamed {
                    from: "old name".into(),
                    similarity: 100
                }
            ),
        ]
    );

    let root = rev(&sandbox, &repo, "HEAD~2");
    let first = commit_changes(sandbox.git(), &repository, root, None).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, ChangeKind::Added);
}

#[test]
fn writes_a_commit_graph() {
    let sandbox = sandbox();
    let repo = branchy(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    assert!(!repository.has_commit_graph());
    write_commit_graph(sandbox.git(), &repository, &CancelToken::new()).unwrap();
    assert!(repository.has_commit_graph());
    assert_eq!(walk(&sandbox, &repo).len(), 6);
}

/// How many trees a repository holds.
fn trees(sandbox: &Sandbox, repo: &Path) -> usize {
    let listed = sandbox
        .run(
            repo,
            "cat-file",
            &["--batch-all-objects", "--batch-check=%(objecttype)"],
        )
        .unwrap();
    listed
        .stdout
        .split(|&byte| byte == b'\n')
        .filter(|kind| *kind == b"tree")
        .count()
}

#[test]
fn a_partial_clones_commit_graph_fetches_nothing() {
    let sandbox = sandbox();
    let source = branchy(&sandbox);
    sandbox
        .run(&source, "config", &["uploadpack.allowFilter", "true"])
        .unwrap();
    let url = format!("file://{}", source.display());
    let clone = sandbox.path("clone");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &[
                "--quiet",
                "--filter=tree:0",
                "--no-checkout",
                &url,
                clone.to_str().unwrap(),
            ],
        )
        .unwrap();
    let before = trees(&sandbox, &clone);

    let repository = Repository::discover(sandbox.git(), &clone).unwrap();
    write_commit_graph(sandbox.git(), &repository, &CancelToken::new()).unwrap();
    assert!(repository.has_commit_graph());
    assert_eq!(
        trees(&sandbox, &clone),
        before,
        "computing changed paths fetched trees"
    );
}
