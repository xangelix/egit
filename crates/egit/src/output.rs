//! A tab's output: what the commands its tasks and commits ran printed, hooks' output above all
//! (a `pre-commit` check's, a `pre-push` hook's), as runs: each a task or commit, its
//! transcript (`egit_git::Transcript`, written as git prints), the hooks it was expected to run,
//! and how it ended. `views/output.rs` shows them in a panel sliding up from the tab's bottom,
//! which opens by itself while hooks run (past a moment) or when a run fails saying something,
//! and closes again after a run it opened for went well.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egit_git::{CancelToken, Transcript};

/// The most runs kept (the oldest forgotten).
const KEPT: usize = 16;

/// How long hooks run before the panel opens for them: a quick one never flashes it.
const OPEN_AFTER: Duration = Duration::from_millis(400);

/// How long after a run went well the panel it opened for closes.
const CLOSE_AFTER: Duration = Duration::from_millis(1500);

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    Done,
    /// Refused or stopped part-way, as git may (conflicts, a push refused): what to do next is
    /// shown elsewhere.
    Stopped,
    /// Stopped to ask what to do (diverged branches, changes in the way, a branch not merged):
    /// a dialog asks.
    Asked,
    Failed,
    Cancelled,
}

/// One task or commit, and what its commands printed.
#[derive(Debug)]
pub(crate) struct Run {
    pub(crate) id: u64,
    /// What it was, as the task bar says it ("Committing", "Pushing").
    pub(crate) title: String,
    pub(crate) transcript: Transcript,
    /// The hooks it was expected to run (those the repository has).
    pub(crate) hooks: Vec<&'static str>,
    pub(crate) started: Instant,
    pub(crate) ended: Option<(Instant, Ended)>,
    /// Its way to stop, while it runs.
    pub(crate) cancel: Option<CancelToken>,
}

impl Run {
    /// How long it ran, or has been running.
    pub(crate) fn elapsed(&self, now: Instant) -> Duration {
        let end = self.ended.map_or(now, |(at, _)| at);
        end.saturating_duration_since(self.started)
    }

    /// Whether it printed anything beyond its commands' lines.
    pub(crate) fn said_something(&self) -> bool {
        self.transcript.read(|log| {
            (0..log.len()).any(|at| {
                log.line(at)
                    .is_some_and(|line| line.kind != egit_git::LineKind::Command)
            })
        })
    }
}

/// A tab's runs, and its panel's state.
#[derive(Debug, Default)]
pub(crate) struct Output {
    runs: VecDeque<Run>,
    next: u64,
    /// Whether the panel is open.
    pub(crate) open: bool,
    /// The last run egit opened it for (it never opens twice for one: closed, it stays so).
    opened_for: u64,
    /// The run whose going well closes it again (`None` once the user opened or closed it).
    closes_after: Option<u64>,
    /// The run picked to show; `None`: the latest.
    pub(crate) picked: Option<u64>,
}

impl Output {
    /// Starts a run titled `title`, expected to run `hooks`, stopped by `cancel`; returns its id
    /// and the transcript its commands print into.
    pub(crate) fn begin(
        &mut self,
        title: &str,
        hooks: Vec<&'static str>,
        cancel: Option<CancelToken>,
    ) -> (u64, Transcript) {
        self.next = self.next.wrapping_add(1);
        let transcript = Transcript::new();
        if self.runs.len() >= KEPT {
            self.runs.pop_front();
        }
        self.runs.push_back(Run {
            id: self.next,
            title: title.to_owned(),
            transcript: transcript.clone(),
            hooks,
            started: Instant::now(),
            ended: None,
            cancel,
        });
        // A new run shows (unless the user picked an older one to read).
        if self.picked.is_some_and(|picked| self.run(picked).is_none()) {
            self.picked = None;
        }
        (self.next, transcript)
    }

    /// Gives run `id` its way to stop.
    pub(crate) fn cancel_with(&mut self, id: u64, cancel: CancelToken) {
        if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
            run.cancel = Some(cancel);
        }
    }

    /// Ends run `id` as `ended`.
    pub(crate) fn end(&mut self, id: u64, ended: Ended) {
        if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
            run.ended = Some((Instant::now(), ended));
            run.cancel = None;
        }
    }

    /// Run `id`, if kept.
    pub(crate) fn run(&self, id: u64) -> Option<&Run> {
        self.runs.iter().find(|run| run.id == id)
    }

    /// The latest run.
    pub(crate) fn latest(&self) -> Option<&Run> {
        self.runs.back()
    }

    /// The run shown: the one picked, else the latest.
    pub(crate) fn shown(&self) -> Option<&Run> {
        self.picked
            .and_then(|picked| self.run(picked))
            .or_else(|| self.latest())
    }

    /// Every run, the newest first.
    pub(crate) fn runs(&self) -> impl Iterator<Item = &Run> {
        self.runs.iter().rev()
    }

    /// Forgets the runs that ended.
    pub(crate) fn clear(&mut self) {
        self.runs.retain(|run| run.ended.is_none());
        self.picked = None;
    }

    /// Opens the panel, or closes it (the user's doing: egit won't close it after).
    pub(crate) const fn toggle(&mut self) {
        self.open = !self.open;
        self.closes_after = None;
    }

    /// Each frame: opens the panel (when `automatic`) for the latest run while its hooks have
    /// run past a moment, or once it failed saying something; closes it after a run it opened
    /// for went well. Returns how soon to look again, while something waits on time.
    pub(crate) fn follow(&mut self, now: Instant, automatic: bool) -> Option<Duration> {
        let latest = self.runs.back()?;
        let id = latest.id;
        let fresh = automatic && !self.open && self.opened_for != id;
        match latest.ended {
            None if fresh && !latest.hooks.is_empty() => {
                let ran = now.saturating_duration_since(latest.started);
                if ran >= OPEN_AFTER {
                    self.open_for(id);
                    self.closes_after = Some(id);
                    return None;
                }
                Some(OPEN_AFTER.saturating_sub(ran))
            }
            Some((_, Ended::Failed)) if fresh => {
                if latest.said_something() {
                    self.open_for(id);
                }
                None
            }
            Some((at, Ended::Done)) if self.open && self.closes_after == Some(id) => {
                let since = now.saturating_duration_since(at);
                if since >= CLOSE_AFTER {
                    self.open = false;
                    self.closes_after = None;
                    return None;
                }
                Some(CLOSE_AFTER.saturating_sub(since))
            }
            _ => None,
        }
    }

    /// Opens the panel for run `id`, showing it.
    const fn open_for(&mut self, id: u64) {
        self.open = true;
        self.opened_for = id;
        self.picked = None;
    }
}

/// The hooks git may run for a commit: `pre-commit` and `commit-msg` unless skipped.
pub(crate) const fn commit_hooks(skipped: bool) -> &'static [&'static str] {
    if skipped {
        &["prepare-commit-msg", "post-commit", "reference-transaction"]
    } else {
        &[
            "pre-commit",
            "prepare-commit-msg",
            "commit-msg",
            "post-commit",
            "reference-transaction",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_opens_for_hooks_and_failures_and_closes_after_success() {
        let mut output = Output::default();
        // Hooks: open past a moment.
        let (first, _) = output.begin("Committing", vec!["pre-commit"], None);
        let start = output.latest().unwrap().started;
        assert!(output.follow(start, true).is_some(), "waits a moment");
        assert!(!output.open);
        assert_eq!(output.follow(start + OPEN_AFTER, true), None);
        assert!(output.open);
        // Gone well: closed after a while.
        output.end(first, Ended::Done);
        let done = output.latest().unwrap().ended.unwrap().0;
        assert!(output.follow(done, true).is_some());
        output.follow(done + CLOSE_AFTER, true);
        assert!(!output.open);
        // No hooks: nothing opens while it runs; failing silently, nothing either.
        let (second, _) = output.begin("Pushing", Vec::new(), None);
        output.follow(start + Duration::from_secs(10), true);
        assert!(!output.open);
        output.end(second, Ended::Failed);
        output.follow(start + Duration::from_secs(10), true);
        assert!(!output.open, "it said nothing");
        // Failing saying something: open, and it stays.
        let (third, transcript) = output.begin("Pushing", Vec::new(), None);
        transcript.ended(false, "rejected".to_owned());
        output.end(third, Ended::Failed);
        output.follow(start, true);
        assert!(output.open);
        // Opened by the user: never closed by egit.
        output.toggle();
        output.toggle();
        let (fourth, _) = output.begin("Committing", vec!["pre-commit"], None);
        output.end(fourth, Ended::Done);
        output.follow(start + Duration::from_secs(60), true);
        assert!(output.open);
        // Not automatic: only the user opens it.
        let mut quiet = Output::default();
        quiet.begin("Committing", vec!["pre-commit"], None);
        quiet.follow(start + Duration::from_secs(5), false);
        assert!(!quiet.open);
    }
}
