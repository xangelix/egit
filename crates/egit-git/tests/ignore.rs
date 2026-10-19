//! Ignoring files git doesn't track, checked by git's own reading of the patterns: each name
//! (glob characters, a leading `#` or `!`, trailing spaces) ignored alone, a look-alike left
//! untracked; an extension and a folder; in `.gitignore` or `info/exclude`; taken back and
//! added again.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use bstr::{BString, ByteSlice as _};
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, IgnoreError, IgnoreIn, Repository, extension_pattern, ignore, ignore_again,
    path_pattern, unignore,
};

/// The untracked files git lists (not ignored), sorted.
fn untracked(sandbox: &Sandbox, repo: &Path) -> Vec<String> {
    let out = sandbox
        .run(
            repo,
            "status",
            &["--porcelain=v1", "-z", "--untracked-files=all"],
        )
        .unwrap();
    let mut listed: Vec<String> = out
        .stdout
        .split_str(b"\0")
        .filter_map(|entry| entry.strip_prefix(b"?? "))
        .map(|path| path.to_str_lossy().into_owned())
        .collect();
    listed.sort();
    listed
}

fn touch(repo: &Path, path: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x\n").unwrap();
}

#[test]
fn each_name_is_ignored_alone_and_taken_back() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();
    // Each name, and what a careless pattern of it would match too.
    let mut names = vec![
        ("plain.txt", "plain.txt.bak"),
        ("star*name", "starXname"),
        ("q?mark", "qXmark"),
        ("[ab]", "a"),
        ("#hash", "hash"),
        ("!bang", "bang"),
        ("日本語.txt", "日本.txt"),
        ("deep/folder/file.c", "file.c"),
    ];
    if cfg!(unix) {
        // Names Windows can't hold.
        names.push(("trailing ", "trailing"));
        names.push(("back\\slash", "backslash"));
    }
    for (name, look_alike) in &names {
        touch(&repo, name);
        touch(&repo, look_alike);
    }
    let patterns: Vec<BString> = names
        .iter()
        .map(|(name, _)| path_pattern((*name).into(), false).unwrap())
        .collect();
    let ignored = ignore(
        sandbox.git(),
        &repository,
        IgnoreIn::Private,
        &patterns,
        &cancel,
    )
    .unwrap();
    assert!(ignored.file.ends_with("info/exclude"), "{ignored:?}");
    let mut left: Vec<&str> = names.iter().map(|(_, look_alike)| *look_alike).collect();
    left.sort_unstable();
    assert_eq!(untracked(&sandbox, &repo), left);

    // Asked again, nothing is added twice.
    let again = ignore(
        sandbox.git(),
        &repository,
        IgnoreIn::Private,
        &patterns,
        &cancel,
    )
    .unwrap();
    assert!(again.added.is_empty());

    // Taken back, every name is listed again; added again, ignored again.
    let before = std::fs::read(&ignored.file).unwrap();
    unignore(sandbox.git(), &ignored).unwrap();
    assert_eq!(untracked(&sandbox, &repo).len(), names.len() * 2);
    let redone = ignore_again(sandbox.git(), &ignored).unwrap();
    assert_eq!(std::fs::read(&ignored.file).unwrap(), before);
    assert_eq!(untracked(&sandbox, &repo), left);
    unignore(sandbox.git(), &redone).unwrap();
}

#[test]
fn an_extension_or_a_folder_is_ignored_in_the_shared_file() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();
    for path in [
        "a.log",
        "deep/b.log",
        "build/out/c.o",
        "builder/d.o",
        "keep.txt",
    ] {
        touch(&repo, path);
    }
    // A .gitignore with CRLF endings and no final one: kept so.
    std::fs::write(repo.join(".gitignore"), "*.tmp\r\n/target").unwrap();
    let patterns = [
        extension_pattern("log".into()).unwrap(),
        path_pattern("build".into(), true).unwrap(),
    ];
    let ignored = ignore(
        sandbox.git(),
        &repository,
        IgnoreIn::Shared,
        &patterns,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(repo.join(".gitignore")).unwrap(),
        b"*.tmp\r\n/target\r\n*.log\r\n/build/\r\n"
    );
    assert_eq!(
        untracked(&sandbox, &repo),
        [".gitignore", "builder/d.o", "keep.txt"]
    );

    // Changed since, it isn't taken back; put back as it was, it is.
    let changed = std::fs::read(repo.join(".gitignore")).unwrap();
    std::fs::write(repo.join(".gitignore"), [&changed[..], b"more\n"].concat()).unwrap();
    assert!(matches!(
        unignore(sandbox.git(), &ignored),
        Err(IgnoreError::Changed(_))
    ));
    std::fs::write(repo.join(".gitignore"), &changed).unwrap();
    unignore(sandbox.git(), &ignored).unwrap();
    assert_eq!(
        std::fs::read(repo.join(".gitignore")).unwrap(),
        b"*.tmp\r\n/target"
    );

    // A .gitignore made for the patterns goes when they are taken back.
    std::fs::remove_file(repo.join(".gitignore")).unwrap();
    let made = ignore(
        sandbox.git(),
        &repository,
        IgnoreIn::Shared,
        &patterns[..1],
        &cancel,
    )
    .unwrap();
    assert!(made.created);
    assert_eq!(std::fs::read(repo.join(".gitignore")).unwrap(), b"*.log\n");
    unignore(sandbox.git(), &made).unwrap();
    assert!(!repo.join(".gitignore").exists());
}
