//! A repository's hooks: the programs git runs around what changes it (before a commit is
//! made, after a checkout, before a push, …), from its hooks folder: `core.hooksPath` when set
//! (husky's `.husky`, a team's shared folder), else the git directory's `hooks`.

use std::path::PathBuf;

use bstr::ByteSlice as _;

use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// Every hook git runs on the client, in the order a day's work meets them.
pub const KNOWN: [&str; 17] = [
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-rewrite",
    "post-checkout",
    "post-merge",
    "pre-push",
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-index-change",
];

/// The hooks a repository has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hooks {
    /// Where git looks for them.
    pub folder: PathBuf,
    /// Whether `core.hooksPath` chose that folder.
    pub configured: bool,
    /// Those there (and runnable), in [`KNOWN`]'s order.
    pub names: Vec<&'static str>,
}

impl Hooks {
    /// Whether `name` is there.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.names.contains(&name)
    }

    /// Those of `wanted` that are there, in `wanted`'s order.
    #[must_use]
    pub fn of(&self, wanted: &[&'static str]) -> Vec<&'static str> {
        wanted
            .iter()
            .copied()
            .filter(|name| self.has(name))
            .collect()
    }
}

/// The hooks `repository` has.
///
/// The folder git runs them from (`rev-parse --git-path hooks`, which follows
/// `core.hooksPath`), and the known hooks in it that git would run (on Unix, those executable;
/// `.sample` files never count).
///
/// # Errors
///
/// When git fails.
pub fn installed_hooks(git: &Git, repository: &Repository) -> Result<Hooks, GitError> {
    let work_tree = repository.work_tree();
    let found = git
        .read("rev-parse")
        .args(["--git-path", "hooks"])
        .current_dir(work_tree)
        .output()?;
    let folder = work_tree.join(found.stdout.trim_ascii_end().to_path_lossy());
    let configured = git
        .read("config")
        .args(["--get", "core.hooksPath"])
        .current_dir(work_tree)
        .output_unchecked()?
        .status
        .success();
    let paths = KNOWN.map(|name| folder.join(name));
    let runnable = git
        .files()
        .runnable(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>())
        .map_err(|source| GitError::Io {
            subcommand: "rev-parse",
            source,
        })?;
    let names = KNOWN
        .iter()
        .zip(runnable)
        .filter_map(|(name, runnable)| runnable.then_some(*name))
        .collect();
    Ok(Hooks {
        folder,
        configured,
        names,
    })
}
