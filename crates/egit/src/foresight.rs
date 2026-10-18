//! Conflicts foreseen before commits are brought in: a merge, a rebase, a cherry-pick, or a
//! revert worked out (`egit_git::foresee_*`) in a job the first time a menu offering it is
//! drawn, a few at a time, and kept by what it is of (the commit `HEAD` is at, and the one
//! brought in), so moving `HEAD` asks again. Never asked in a partial clone, where git would
//! fetch what it merges.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use egit_git::{
    CancelToken, Foreseen, Git, ObjectId, Repository, foresee_merge, foresee_pick, foresee_rebase,
};
use parking_lot::Mutex;

use crate::jobs;

/// The most foreseen kept, the least recently worked out forgotten first.
const KEPT: usize = 64;

/// The most worked out at once (each runs git).
const AT_ONCE: usize = 4;

/// What would be brought into the branch `HEAD` is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Bringing {
    /// A commit merged in.
    Merge(ObjectId),
    /// The branch rebased onto a commit.
    Rebase(ObjectId),
    /// A commit cherry-picked.
    Pick(ObjectId),
    /// A commit reverted.
    Revert(ObjectId),
}

/// How foreseeing it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    /// Being worked out.
    Working,
    /// Worked out.
    Seen(Foreseen),
    /// Not known: git failed, or a root commit has no changes to work out.
    Unknown,
}

/// A repository's foreseen, shared with the jobs working them out (a clone shares them).
#[derive(Clone, Debug, Default)]
pub(crate) struct Foresight {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    seen: HashMap<(ObjectId, Bringing), Option<Foreseen>>,
    /// The order they were worked out in, to forget the oldest.
    order: VecDeque<(ObjectId, Bringing)>,
    /// Those being worked out.
    working: HashSet<(ObjectId, Bringing)>,
}

impl Foresight {
    /// What bringing in would do with `HEAD` at `head`: worked out, or (asked the first time,
    /// when fewer than [`AT_ONCE`] are) being worked out; the window is woken when it is.
    pub(crate) fn look(
        &self,
        ctx: &egui::Context,
        (git, repository): (&Git, &Arc<Repository>),
        head: ObjectId,
        bringing: Bringing,
    ) -> Look {
        let key = (head, bringing);
        let mut state = self.state.lock();
        if let Some(seen) = state.seen.get(&key) {
            return seen.clone().map_or(Look::Unknown, Look::Seen);
        }
        if state.working.contains(&key) || state.working.len() >= AT_ONCE {
            return Look::Working;
        }
        state.working.insert(key);
        drop(state);
        let shared = Arc::clone(&self.state);
        let (git, repository, ctx) = (git.clone(), Arc::clone(repository), ctx.clone());
        jobs::detached(move || {
            let seen = foresee(&git, &repository, head, bringing);
            let mut state = shared.lock();
            state.working.remove(&key);
            state.seen.insert(key, seen);
            state.order.push_back(key);
            while state.order.len() > KEPT {
                if let Some(oldest) = state.order.pop_front() {
                    state.seen.remove(&oldest);
                }
            }
            drop(state);
            ctx.request_repaint();
        });
        Look::Working
    }
}

/// Works out what bringing in would do (`None` when git fails, or for a root commit).
fn foresee(
    git: &Git,
    repository: &Repository,
    head: ObjectId,
    bringing: Bringing,
) -> Option<Foreseen> {
    let cancel = CancelToken::new();
    let foreseen = match bringing {
        Bringing::Merge(other) => foresee_merge(git, repository, head, other, &cancel).map(Some),
        Bringing::Rebase(onto) => foresee_rebase(git, repository, head, onto, &cancel).map(Some),
        Bringing::Pick(commit) => foresee_pick(git, repository, head, (commit, false), &cancel),
        Bringing::Revert(commit) => foresee_pick(git, repository, head, (commit, true), &cancel),
    };
    foreseen.unwrap_or_else(|error| {
        tracing::debug!(%error, ?bringing, "couldn't foresee it");
        None
    })
}
