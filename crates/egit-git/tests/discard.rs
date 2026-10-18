//! Discarding changes not staged, with the real git: whole files (changed with some staged,
//! deleted, untracked, an executable, a link) and some lines (a hunk of two, of a file whose
//! line endings git converts, of an untracked file, of a deleted one), each put back and done
//! again; a diff read before its file changed refused, putting back refused once a file moved
//! on, and a file too large to keep leaving a discard kept by nothing.

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
use egit_git::testing::{Sandbox, set_mode};
use egit_git::{
    CancelToken, DiffLineKind, DiffOptions, DiscardError, DiscardSide, FileDiff, Repository,
    Status, TreeEntry, Uncommitted, discard_files, discard_lines, put_back, uncommitted_diff,
};

fn read(repo: &Path, file: &str) -> String {
    String::from_utf8(std::fs::read(repo.join(file)).unwrap()).unwrap()
}

fn diff(sandbox: &Sandbox, repository: &Repository, path: &str, which: Uncommitted) -> FileDiff {
    uncommitted_diff(
        sandbox.git(),
        repository,
        &[BStr::new(path)],
        which,
        &DiffOptions::default(),
        &CancelToken::new(),
    )
    .unwrap()
}

/// What the index holds at `path`.
fn entry(sandbox: &Sandbox, repository: &Repository, path: &str) -> Option<TreeEntry> {
    let status = Status::read(sandbox.git(), repository, &CancelToken::new()).unwrap();
    status
        .entries()
        .find(|entry| entry.path == path)
        .and_then(egit_git::StatusEntry::index)
}

/// The indices of the changed lines of `diff`'s `hunk`th hunk (from 0).
fn hunk_changes(diff: &FileDiff, hunk: usize) -> Vec<usize> {
    let mut seen = 0;
    let mut picked = Vec::new();
    for (index, line) in diff.lines.iter().enumerate() {
        match line.kind {
            DiffLineKind::Hunk => seen += 1,
            DiffLineKind::Added | DiffLineKind::Removed if seen == hunk + 1 => picked.push(index),
            _ => {}
        }
    }
    picked
}

/// The paths with changes, as `git status --short` lists them.
fn short_status(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox
        .run(repo, "status", &["--short", "--untracked-files=all"])
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn whole_files_are_discarded_put_back_and_discarded_again() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "one\n", "A").unwrap();
    sandbox.commit_file(&repo, "b", "keep\n", "B").unwrap();
    // `a` has a staged change and one not staged; `b` is deleted; two files are new.
    std::fs::write(repo.join("a"), "one\nstaged\n").unwrap();
    sandbox.run(&repo, "add", &["a"]).unwrap();
    std::fs::write(repo.join("a"), "one\nstaged\nnot staged\n").unwrap();
    std::fs::remove_file(repo.join("b")).unwrap();
    std::fs::create_dir(repo.join("new")).unwrap();
    std::fs::write(repo.join("new/run.sh"), "#!/bin/sh\n").unwrap();
    set_mode(&repo.join("new/run.sh"), 0o755).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a", repo.join("link")).unwrap();
    let untracked: &[&BStr] = if cfg!(unix) {
        &[BStr::new("new/run.sh"), BStr::new("link")]
    } else {
        &[BStr::new("new/run.sh")]
    };
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();

    let files = discard_files(
        sandbox.git(),
        &repository,
        &[BStr::new("a"), BStr::new("b")],
        untracked,
        &cancel,
    )
    .unwrap()
    .expect("kept");
    assert_eq!(read(&repo, "a"), "one\nstaged\n", "what is staged stays");
    assert_eq!(read(&repo, "b"), "keep\n");
    assert!(!repo.join("new/run.sh").exists());
    assert!(repo.join("link").symlink_metadata().is_err());
    assert_eq!(short_status(&sandbox, &repo), "M  a\n");
    assert_eq!(files.len(), 2 + untracked.len());
    assert!(files[1].before.is_none() && files[1].after.is_some(), "b");
    assert!(
        files[2].before.is_some() && files[2].after.is_none(),
        "run.sh"
    );

    // Undone: every file as it was, the executable still one, the link a link.
    put_back(
        sandbox.git(),
        &repository,
        &files,
        DiscardSide::Before,
        &cancel,
    )
    .unwrap();
    assert_eq!(read(&repo, "a"), "one\nstaged\nnot staged\n");
    assert!(!repo.join("b").exists());
    assert_eq!(read(&repo, "new/run.sh"), "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(repo.join("new/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "executable again");
        assert_eq!(
            std::fs::read_link(repo.join("link")).unwrap(),
            Path::new("a")
        );
    }
    // Done again.
    put_back(
        sandbox.git(),
        &repository,
        &files,
        DiscardSide::After,
        &cancel,
    )
    .unwrap();
    assert_eq!(short_status(&sandbox, &repo), "M  a\n");

    // A file changed since: nothing is put back.
    std::fs::write(repo.join("a"), "someone else's\n").unwrap();
    let refused = put_back(
        sandbox.git(),
        &repository,
        &files,
        DiscardSide::Before,
        &cancel,
    );
    assert!(matches!(refused, Err(DiscardError::Stale(path)) if path == "a"));
    assert_eq!(read(&repo, "b"), "keep\n", "nothing written");
}

#[test]
fn some_lines_are_discarded_and_put_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let lines: Vec<String> = (1..=20).map(|n| format!("line {n}\n")).collect();
    sandbox
        .commit_file(&repo, "f", &lines.concat(), "F")
        .unwrap();
    let mut changed = lines.clone();
    changed[1] = "LINE 2\n".to_owned();
    changed[15] = "LINE 16\n".to_owned();
    std::fs::write(repo.join("f"), changed.concat()).unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();

    let unstaged = diff(&sandbox, &repository, "f", Uncommitted::Unstaged);
    let picked = hunk_changes(&unstaged, 1);
    assert_eq!(picked.len(), 2, "the second hunk's removal and addition");
    let file = discard_lines(
        sandbox.git(),
        &repository,
        (BStr::new("f"), &unstaged),
        |index| picked.contains(&index),
        entry(&sandbox, &repository, "f"),
        &cancel,
    )
    .unwrap();
    let mut kept = lines;
    kept[1] = "LINE 2\n".to_owned();
    assert_eq!(read(&repo, "f"), kept.concat(), "the first hunk stays");

    let files = [file];
    put_back(
        sandbox.git(),
        &repository,
        &files,
        DiscardSide::Before,
        &cancel,
    )
    .unwrap();
    assert_eq!(read(&repo, "f"), changed.concat());
    put_back(
        sandbox.git(),
        &repository,
        &files,
        DiscardSide::After,
        &cancel,
    )
    .unwrap();
    assert_eq!(read(&repo, "f"), kept.concat());

    // A diff read before the file changed: refused, nothing written.
    let unstaged = diff(&sandbox, &repository, "f", Uncommitted::Unstaged);
    std::fs::write(repo.join("f"), "rewritten\n").unwrap();
    let refused = discard_lines(
        sandbox.git(),
        &repository,
        (BStr::new("f"), &unstaged),
        |_| true,
        entry(&sandbox, &repository, "f"),
        &cancel,
    );
    assert!(
        matches!(refused, Err(DiscardError::Stale(_))),
        "{refused:?}"
    );
    assert_eq!(read(&repo, "f"), "rewritten\n");
}

#[test]
fn lines_keep_the_line_endings_git_converts() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .run(&repo, "config", &["core.autocrlf", "true"])
        .unwrap();
    sandbox
        .commit_file(&repo, "f", "a\r\nb\r\nc\r\n", "F")
        .unwrap();
    std::fs::write(repo.join("f"), "a\r\nB\r\nc\r\nd\r\n").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let unstaged = diff(&sandbox, &repository, "f", Uncommitted::Unstaged);
    // Only `d`, the last line added.
    let last = unstaged.lines.len() - 1;
    assert_eq!(unstaged.lines[last].kind, DiffLineKind::Added);
    discard_lines(
        sandbox.git(),
        &repository,
        (BStr::new("f"), &unstaged),
        |index| index == last,
        entry(&sandbox, &repository, "f"),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(read(&repo, "f"), "a\r\nB\r\nc\r\n");
}

#[test]
fn lines_of_untracked_and_deleted_files() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "gone", "x\ny\n", "Gone")
        .unwrap();
    std::fs::remove_file(repo.join("gone")).unwrap();
    std::fs::write(repo.join("new"), "1\n2\n3\n").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();

    // An untracked file without its second line.
    let untracked = diff(&sandbox, &repository, "new", Uncommitted::Untracked);
    let second = untracked
        .lines
        .iter()
        .position(|line| line.kind == DiffLineKind::Added && line.new == 2)
        .unwrap();
    discard_lines(
        sandbox.git(),
        &repository,
        (BStr::new("new"), &untracked),
        |index| index == second,
        None,
        &cancel,
    )
    .unwrap();
    assert_eq!(read(&repo, "new"), "1\n3\n");

    // A deleted file: its first line's removal discarded, so it comes back holding that line.
    let deleted = diff(&sandbox, &repository, "gone", Uncommitted::Unstaged);
    let first = deleted
        .lines
        .iter()
        .position(|line| line.kind == DiffLineKind::Removed && line.old == 1)
        .unwrap();
    let file = discard_lines(
        sandbox.git(),
        &repository,
        (BStr::new("gone"), &deleted),
        |index| index == first,
        entry(&sandbox, &repository, "gone"),
        &cancel,
    )
    .unwrap();
    assert_eq!(read(&repo, "gone"), "x\n");
    assert!(file.before.is_none());
    put_back(
        sandbox.git(),
        &repository,
        &[file],
        DiscardSide::Before,
        &cancel,
    )
    .unwrap();
    assert!(!repo.join("gone").exists());
}

#[test]
fn a_file_too_large_to_keep_leaves_nothing_kept() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "one\n", "A").unwrap();
    // Sparse: as large as it says, written in no time.
    let large = std::fs::File::create(repo.join("large.bin")).unwrap();
    large.set_len(egit_git::KEEP_BYTES + 1).unwrap();
    drop(large);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let kept = discard_files(
        sandbox.git(),
        &repository,
        &[],
        &[BStr::new("large.bin")],
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(kept, None);
    assert!(!repo.join("large.bin").exists(), "discarded all the same");
}
