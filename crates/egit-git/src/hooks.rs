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

/// The hooks that judge what is committed, and may refuse it: those `--no-verify` skips
/// (`git commit`'s, `merge`'s, `rebase`'s, `am`'s).
pub const CHECKS: [&str; 6] = [
    "pre-commit",
    "pre-merge-commit",
    "commit-msg",
    "pre-rebase",
    "applypatch-msg",
    "pre-applypatch",
];

/// Whether the hooks that judge what is committed ([`CHECKS`]) run, as git runs them, or are
/// skipped (`--no-verify`).
///
/// For whatever makes commits: a commit, a merge, continuing one stopped at conflicts, a
/// rebase, patches applied as commits. Hooks that only follow along
/// (`prepare-commit-msg`, `post-commit`, `post-merge`, …) run either way, as git runs them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Checks {
    /// As git runs them.
    #[default]
    Run,
    /// Skipped.
    Skip,
}

impl Checks {
    /// Whether they are skipped.
    #[must_use]
    pub const fn skipped(self) -> bool {
        matches!(self, Self::Skip)
    }

    /// The flag skipping them, when they are.
    #[must_use]
    pub const fn flag(self) -> Option<&'static str> {
        match self {
            Self::Run => None,
            Self::Skip => Some("--no-verify"),
        }
    }

    /// Those of `hooks` that run: all of them, or those that aren't checks.
    #[must_use]
    pub fn running(self, hooks: &[&'static str]) -> Vec<&'static str> {
        hooks
            .iter()
            .copied()
            .filter(|hook| !self.skipped() || !CHECKS.contains(hook))
            .collect()
    }
}

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
pub async fn installed_hooks(git: &Git, repository: &Repository) -> Result<Hooks, GitError> {
    let work_tree = repository.work_tree();
    let found = git
        .read("rev-parse")
        .args(["--git-path", "hooks"])
        .current_dir(work_tree)
        .output()
        .await?;
    let folder = work_tree.join(found.stdout.trim_ascii_end().to_path_lossy());
    let configured = git
        .read("config")
        .args(["--get", "core.hooksPath"])
        .current_dir(work_tree)
        .output_unchecked()
        .await?
        .status
        .success();
    let paths = KNOWN.map(|name| folder.join(name));
    let runnable = git
        .files()
        .runnable(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>())
        .await
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
