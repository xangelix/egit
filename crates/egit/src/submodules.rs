//! A repository's submodules as egit keeps them: each one `.gitmodules` declares (read with the
//! tab's other lists), where each stands, nested ones too (`submodule status --recursive`, read
//! alongside where there are any), what is out of step and how to mend it, and the tasks done
//! to them.
//!
//! For whoever meets submodules: each is a repository of its own, checked out inside this one
//! at a commit this one records. Pulling or switching here changes the commits recorded, but not
//! what is checked out in them, unless they are updated (or `submodule.recurse` is set, which
//! makes git do it): until then they show as elsewhere. Committing in one moves it too; recording
//! its new commit here is staging its path.

use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, Checkout, ConfigSnapshot, ConfigTarget, Git, ObjectId, Progress, Repository,
    Submodule, SubmoduleChange, SubmoduleDetails, SubmoduleStatus, UpdateTo, add_submodule,
    change_submodules, remove_submodule, set_config, set_submodule_branch, set_submodule_url,
    submodule_commits, submodule_details, submodule_status, unset_config, update_submodules_to,
};

use crate::jobs;
use crate::problem::Problem;
use crate::tasks::Outcome;

/// Where the submodules stand.
#[derive(Debug, Default)]
pub(crate) struct Standing {
    /// Each one, nested ones too.
    pub(crate) states: Vec<SubmoduleStatus>,
    /// Those at another commit than recorded that lack commits it has: a pull or switch here
    /// recorded a newer one they aren't updated to (those only past it are someone's work, to
    /// record).
    pub(crate) behind: Vec<BString>,
}

/// Where each submodule stands, nested ones too (nothing run where there are no submodules).
pub(crate) async fn read_states(
    git: Git,
    repository: Arc<Repository>,
) -> Result<Arc<Standing>, Problem> {
    jobs::blocking(async move {
        if !git
            .files()
            .is_file(&repository.work_tree().join(".gitmodules"))
            .await
        {
            return Ok(Arc::default());
        }
        let failed = |error| Problem::git("Where the submodules stand couldn't be read", &error);
        let states = submodule_status(&git, &repository, true, &CancelToken::new())
            .await
            .map_err(failed)?;
        let moved: Vec<&SubmoduleStatus> = states
            .iter()
            .filter(|state| state.checkout == Checkout::Elsewhere)
            .collect();
        let mut behind = Vec::new();
        if !moved.is_empty() {
            let modules = egit_git::submodules(&git, &repository)
                .await
                .map_err(failed)?;
            for state in moved {
                // Nested ones record theirs in their parents: not looked into here.
                let Some(module) = modules.iter().find(|module| module.path == state.path) else {
                    continue;
                };
                let details =
                    submodule_details(&git, &repository, state.path.as_ref(), module.recorded)
                        .await
                        .map_err(failed)?;
                if details.behind > 0 || !details.has_recorded {
                    behind.push(state.path.clone());
                }
            }
        }
        Ok(Arc::new(Standing { states, behind }))
    })
    .await
}

/// How many submodules are out of step, and how.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OutOfStep {
    /// Not set up here: their folders are empty.
    pub(crate) not_set_up: usize,
    /// At other commits than this repository records, lacking commits it records (to update).
    pub(crate) behind: usize,
    /// At other commits than recorded, only past them (someone's work, to record).
    pub(crate) past: usize,
    /// In a merge conflict.
    pub(crate) conflicts: usize,
    /// Set up with an address `.gitmodules` no longer gives (sync mends it).
    pub(crate) stale_urls: usize,
}

impl OutOfStep {
    /// Counts what `standing` (and `modules`, for their addresses) shows out of step.
    pub(crate) fn of(
        modules: &[Submodule],
        standing: &Standing,
        config: Option<&ConfigSnapshot>,
    ) -> Self {
        let mut out = Self::default();
        for state in &standing.states {
            match state.checkout {
                Checkout::NotInitialized => out.not_set_up = out.not_set_up.saturating_add(1),
                Checkout::Elsewhere if standing.behind.contains(&state.path) => {
                    out.behind = out.behind.saturating_add(1);
                }
                Checkout::Elsewhere => out.past = out.past.saturating_add(1),
                Checkout::Conflict => out.conflicts = out.conflicts.saturating_add(1),
                Checkout::AtRecorded => {}
            }
        }
        out.stale_urls = modules
            .iter()
            .filter(|module| url_is_stale(module, config))
            .count();
        out
    }

    /// Whether anything needs mending (those only past the commits recorded are someone's
    /// work, not a fault).
    pub(crate) const fn any(self) -> bool {
        self.not_set_up > 0 || self.behind > 0 || self.conflicts > 0 || self.stale_urls > 0
    }

    /// What is, in words ("2 not set up · 1 at another commit").
    pub(crate) fn words(self) -> String {
        let mut said = Vec::new();
        if self.not_set_up > 0 {
            said.push(format!("{} not set up", self.not_set_up));
        }
        if self.behind > 0 {
            said.push(if self.behind == 1 {
                "1 behind the commit recorded".to_owned()
            } else {
                format!("{} behind the commits recorded", self.behind)
            });
        }
        if self.past > 0 {
            said.push(if self.past == 1 {
                "1 moved past the commit recorded, to record".to_owned()
            } else {
                format!("{} moved past the commits recorded, to record", self.past)
            });
        }
        if self.conflicts > 0 {
            said.push(format!("{} in conflict", self.conflicts));
        }
        if self.stale_urls > 0 {
            said.push(if self.stale_urls == 1 {
                "1 address out of date".to_owned()
            } else {
                format!("{} addresses out of date", self.stale_urls)
            });
        }
        said.join(" · ")
    }
}

/// Whether `module` is set up with another address than `.gitmodules` now gives (a relative one
/// resolved against the remote this repository fetches from, as git resolves it).
pub(crate) fn url_is_stale(module: &Submodule, config: Option<&ConfigSnapshot>) -> bool {
    let (Some(declared), Some(configured)) = (&module.url, &module.configured_url) else {
        return false;
    };
    if declared.starts_with(b"./") || declared.starts_with(b"../") {
        let base = config
            .and_then(|config| config.get("remote.origin.url"))
            .and_then(|entry| entry.value);
        return base.is_some_and(|base| resolve_relative(base, declared) != *configured);
    }
    declared != configured
}

/// A relative address (`../lib.git`) resolved against `base` (the superproject's remote), as
/// git resolves it: each `../` drops a part of `base`'s path, `./` none.
fn resolve_relative(base: &[u8], relative: &[u8]) -> BString {
    let mut base = base.trim_end_with(|c| c == '/').to_vec();
    let mut rest = relative;
    loop {
        if let Some(after) = rest.strip_prefix(b"./") {
            rest = after;
        } else if let Some(after) = rest.strip_prefix(b"../") {
            rest = after;
            let cut = base
                .iter()
                .rposition(|&byte| byte == b'/' || byte == b':')
                .unwrap_or(0);
            // An scp-like address keeps its host's colon: `host:org` up to `host:`.
            let keep = if base.get(cut) == Some(&b':') {
                cut.saturating_add(1)
            } else {
                cut
            };
            base.truncate(keep);
        } else {
            break;
        }
    }
    let separator = if base.ends_with(b":") { "" } else { "/" };
    let mut resolved = BString::from(base);
    resolved.extend_from_slice(separator.as_bytes());
    resolved.extend_from_slice(rest);
    resolved
}

/// What a submodule's selected row shows of it, read when selected.
pub(crate) async fn read_details(
    git: Git,
    repository: Arc<Repository>,
    path: BString,
    recorded: ObjectId,
) -> Result<Arc<(SubmoduleDetails, Option<Vec<(ObjectId, BString)>>)>, Problem> {
    jobs::blocking(async move {
        let failed = |error| Problem::git("The submodule couldn't be read", &error);
        let details = submodule_details(&git, &repository, path.as_ref(), recorded)
            .await
            .map_err(failed)?;
        let commits = match details.head {
            Some(head) if head != recorded && details.has_recorded => submodule_commits(
                &git,
                &repository,
                path.as_ref(),
                (Some(recorded), head),
                100,
            )
            .await
            .map_err(failed)?,
            _ => None,
        };
        Ok(Arc::new((details, commits)))
    })
    .await
}

/// A submodule's move from one commit to another (`to`: `None` for its `HEAD` now): the
/// commits it brings, `None` when it isn't here to tell.
pub(crate) async fn read_move(
    git: Git,
    repository: Arc<Repository>,
    path: BString,
    (from, to): (Option<ObjectId>, Option<ObjectId>),
) -> Result<Arc<Moved>, Problem> {
    jobs::blocking(async move {
        let failed = |error| Problem::git("The submodule's commits couldn't be read", &error);
        let to = match to {
            Some(to) => Some(to),
            None => match from {
                Some(from) => {
                    submodule_details(&git, &repository, path.as_ref(), from)
                        .await
                        .map_err(failed)?
                        .head
                }
                None => None,
            },
        };
        let commits = match to {
            Some(to) => submodule_commits(&git, &repository, path.as_ref(), (from, to), 200)
                .await
                .map_err(failed)?,
            None => None,
        };
        Ok(Arc::new(Moved { to, commits }))
    })
    .await
}

/// A submodule moved: where to (its `HEAD`, when asked for it), and the commits that brings
/// (none known when it isn't here).
#[derive(Debug)]
pub(crate) struct Moved {
    pub(crate) to: Option<ObjectId>,
    pub(crate) commits: Option<Vec<(ObjectId, BString)>>,
}

/// What the user asks of submodules (each at `paths`, all when empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SubmoduleTask {
    /// Bring them where `to` says, setting up those not here, theirs too.
    Update { paths: Vec<BString>, to: UpdateTo },
    /// Bring their addresses here to what `.gitmodules` says.
    Sync(Vec<BString>),
    /// Everything that usually mends them: sync the addresses, then set up and update every
    /// one to the commits recorded, nested ones too.
    Mend,
    /// Forget one here (`force`: with its changes).
    Deinit { path: BString, force: bool },
    /// Add a repository as a submodule.
    Add {
        url: String,
        path: BString,
        branch: Option<String>,
    },
    /// Remove one (`force`: with its changes).
    Remove { path: BString, force: bool },
    /// Point one at another address.
    SetUrl { path: BString, url: String },
    /// Have one follow a branch (`None`: its remote's default).
    SetBranch {
        path: BString,
        branch: Option<String>,
    },
    /// Have git update them on every switch and pull (`submodule.recurse`), or not.
    Recurse(bool),
}

/// The words for a few paths: one's path, else how many.
fn which(paths: &[BString]) -> String {
    match paths {
        [] => "the submodules".to_owned(),
        [path] => path.to_str_lossy().into_owned(),
        paths => format!("{} submodules", paths.len()),
    }
}

impl SubmoduleTask {
    /// What it is doing, for people.
    pub(crate) const fn doing(&self) -> &'static str {
        match self {
            Self::Update { .. } => "Updating the submodules",
            Self::Sync(_) => "Syncing the submodules' addresses",
            Self::Mend => "Mending the submodules",
            Self::Deinit { .. } => "Forgetting the submodule",
            Self::Add { .. } => "Adding the submodule",
            Self::Remove { .. } => "Removing the submodule",
            Self::SetUrl { .. } => "Changing the submodule's address",
            Self::SetBranch { .. } => "Changing the submodule's branch",
            Self::Recurse(_) => "Changing how submodules follow",
        }
    }

    /// How it ended, in words, with what to do next.
    pub(crate) fn done(&self) -> String {
        match self {
            Self::Update {
                paths,
                to: UpdateTo::Recorded,
            } => format!("Brought {} to the commits recorded here.", which(paths)),
            Self::Update {
                paths,
                to: UpdateTo::Latest,
            } => format!(
                "Brought {} to their branches' latest: record the new commits here (stage \
                 them) to keep them.",
                which(paths)
            ),
            Self::Sync(paths) => format!("Synced {}' addresses with .gitmodules.", which(paths)),
            Self::Mend => "Mended the submodules: addresses synced, each set up and at the \
                           commit recorded."
                .to_owned(),
            Self::Deinit { path, .. } => format!(
                "Forgot {} here: its folder is empty. Set it up again to bring it back.",
                path.to_str_lossy()
            ),
            Self::Add { path, .. } => format!(
                "Added {}: .gitmodules and it are staged, to commit.",
                path.to_str_lossy()
            ),
            Self::Remove { path, .. } => format!(
                "Removed {}: commit to share it. Its repository stays in .git/modules.",
                path.to_str_lossy()
            ),
            Self::SetUrl { path, .. } => format!(
                "Changed {}'s address: commit .gitmodules to share it.",
                path.to_str_lossy()
            ),
            Self::SetBranch { path, .. } => format!(
                "Changed the branch {} follows: commit .gitmodules to share it.",
                path.to_str_lossy()
            ),
            Self::Recurse(true) => {
                "Switching and pulling now update the submodules too.".to_owned()
            }
            Self::Recurse(false) => {
                "Switching and pulling leave the submodules where they are.".to_owned()
            }
        }
    }
}

/// Runs a submodule task.
pub(crate) async fn run(
    git: &Git,
    repository: &Repository,
    task: &SubmoduleTask,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    fn paths(paths: &[BString]) -> Vec<&BStr> {
        paths.iter().map(AsRef::as_ref).collect()
    }
    let result = match task {
        SubmoduleTask::Update { paths: chosen, to } => {
            update_submodules_to(git, repository, &paths(chosen), *to, progress, cancel).await
        }
        SubmoduleTask::Sync(chosen) => {
            change_submodules(git, repository, &paths(chosen), &SubmoduleChange::Sync).await
        }
        SubmoduleTask::Mend => {
            match change_submodules(git, repository, &[], &SubmoduleChange::Sync).await {
                Ok(()) => {
                    let all = UpdateTo::Recorded;
                    update_submodules_to(git, repository, &[], all, progress, cancel).await
                }
                Err(error) => Err(error),
            }
        }
        SubmoduleTask::Deinit { path, force } => {
            change_submodules(
                git,
                repository,
                &[path.as_ref()],
                &SubmoduleChange::Deinit { force: *force },
            )
            .await
        }
        SubmoduleTask::Add { url, path, branch } => {
            add_submodule(
                git,
                repository,
                url,
                path.as_ref(),
                branch.as_deref(),
                progress,
                cancel,
            )
            .await
        }
        SubmoduleTask::Remove { path, force } => {
            remove_submodule(git, repository, path.as_ref(), *force).await
        }
        SubmoduleTask::SetUrl { path, url } => {
            set_submodule_url(git, repository, path.as_ref(), url).await
        }
        SubmoduleTask::SetBranch { path, branch } => {
            set_submodule_branch(git, repository, path.as_ref(), branch.as_deref()).await
        }
        SubmoduleTask::Recurse(on) => {
            let target = ConfigTarget::Repository(repository.work_tree());
            if *on {
                set_config(git, target, "submodule.recurse", "true").await
            } else {
                unset_config(git, target, "submodule.recurse").await
            }
        }
    };
    result.map(|()| Outcome::Done).map_err(|error| {
        let title = match task {
            SubmoduleTask::Update { .. } | SubmoduleTask::Mend => "The submodules weren't updated",
            SubmoduleTask::Sync(_) => "The addresses weren't synced",
            SubmoduleTask::Deinit { .. } => "The submodule wasn't forgotten",
            SubmoduleTask::Add { .. } => "The submodule wasn't added",
            SubmoduleTask::Remove { .. } => "The submodule wasn't removed",
            SubmoduleTask::SetUrl { .. } => "The address wasn't changed",
            SubmoduleTask::SetBranch { .. } => "The branch wasn't changed",
            SubmoduleTask::Recurse(_) => "The setting wasn't changed",
        };
        Problem::git(title, &error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_addresses_resolve_as_git_resolves_them() {
        let resolve = |base: &str, relative: &str| {
            resolve_relative(base.as_bytes(), relative.as_bytes()).to_string()
        };
        assert_eq!(
            resolve("https://example.com/org/app.git", "../lib.git"),
            "https://example.com/org/lib.git"
        );
        assert_eq!(
            resolve("git@example.com:org/app.git", "../../other/lib.git"),
            "git@example.com:other/lib.git"
        );
        assert_eq!(
            resolve("https://example.com/org/app", "./sub"),
            "https://example.com/org/app/sub"
        );
    }

    #[test]
    fn what_is_out_of_step_is_counted_and_said() {
        let state = |checkout| SubmoduleStatus {
            path: "a".into(),
            checkout,
            commit: ObjectId::from_hex(b"87523ccdc7076cf5e94b51236f4682cdf5f48a95").unwrap(),
            describe: None,
        };
        let mut behind = state(Checkout::Elsewhere);
        behind.path = "behind".into();
        let standing = Standing {
            states: vec![
                state(Checkout::NotInitialized),
                behind,
                state(Checkout::Elsewhere),
                state(Checkout::AtRecorded),
            ],
            behind: vec!["behind".into()],
        };
        let out = OutOfStep::of(&[], &standing, None);
        assert!(out.any());
        assert_eq!(
            out.words(),
            "1 not set up · 1 behind the commit recorded · 1 moved past the commit recorded, to record"
        );
        assert!(!OutOfStep::default().any());
        // Only past the commit recorded: someone's work, not a fault.
        let past = Standing {
            states: vec![state(Checkout::Elsewhere)],
            behind: Vec::new(),
        };
        assert!(!OutOfStep::of(&[], &past, None).any());
    }
}
