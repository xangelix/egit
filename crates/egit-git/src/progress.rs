//! What a long command (clone, fetch, pull, push) reports as it works: git's `--progress` lines
//! (`Receiving objects:  45% (1234/2742), 1.20 MiB | 2.30 MiB/s`), its own and the remote's
//! (`remote: …`), read from standard error as they come. Git rewrites a progress line in place
//! (ending it with `\r`), so each one replaces the last; other lines are kept as notes.

use std::sync::Arc;

use bstr::ByteSlice as _;
use parking_lot::Mutex;

/// The most notes kept (the latest).
const MAX_NOTES: usize = 50;

/// A command's progress, shared between the thread reading git's standard error and whoever
/// shows it.
#[derive(Clone, Debug, Default)]
pub struct Progress(Arc<Mutex<ProgressReport>>);

/// Where a command has got to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgressReport {
    /// What git is doing now (`Receiving objects`), empty before it says.
    pub phase: String,
    /// Whether the remote is the one doing it (counting and compressing what it sends).
    pub remote: bool,
    /// How many of the phase's items are done.
    pub done: u64,
    /// How many there are, when git knows.
    pub total: Option<u64>,
    /// What has moved and how fast (`1.20 MiB | 2.30 MiB/s`), when git says.
    pub transfer: Option<String>,
    /// The other lines git and the remote printed, the latest last (at most fifty).
    pub notes: Vec<String>,
    /// Counts the updates (to notice one).
    pub updates: u64,
}

impl Progress {
    /// Progress with nothing reported yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the command has got to.
    #[must_use]
    pub fn report(&self) -> ProgressReport {
        self.0.lock().clone()
    }

    /// Forgets what was reported (for the next command).
    pub fn clear(&self) {
        *self.0.lock() = ProgressReport::default();
    }

    /// A reader feeding it (the thread draining git's standard error).
    pub(crate) fn reader(&self) -> ProgressReader {
        ProgressReader {
            progress: self.clone(),
            partial: Vec::new(),
        }
    }
}

/// Splits standard error into lines (ended by `\r` or `\n`) and reports each.
#[derive(Debug)]
pub(crate) struct ProgressReader {
    progress: Progress,
    /// A line not ended yet.
    partial: Vec<u8>,
}

impl ProgressReader {
    /// Takes in a chunk of standard error.
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        let mut rest = chunk;
        while let Some(end) = rest.find_byteset(b"\r\n") {
            let (line, after) = rest.split_at(end);
            self.partial.extend_from_slice(line);
            let whole = std::mem::take(&mut self.partial);
            self.line(&whole);
            rest = after.get(1..).unwrap_or_default();
        }
        self.partial.extend_from_slice(rest);
    }

    fn line(&self, line: &[u8]) {
        let text = line.to_str_lossy();
        let text = text.trim_end();
        if text.is_empty() {
            return;
        }
        let (remote, text) = text
            .strip_prefix("remote: ")
            .map_or((false, text), |rest| (true, rest.trim_end()));
        let mut report = self.progress.0.lock();
        report.updates = report.updates.wrapping_add(1);
        if let Some(step) = Step::parse(text) {
            step.phase.clone_into(&mut report.phase);
            report.remote = remote;
            report.done = step.done;
            report.total = step.total;
            report.transfer = step.transfer.map(ToOwned::to_owned);
        } else if !text.trim().is_empty() {
            if report.notes.len() >= MAX_NOTES {
                report.notes.remove(0);
            }
            let note = if remote {
                format!("remote: {text}")
            } else {
                text.to_owned()
            };
            report.notes.push(note);
        }
    }
}

/// A progress line read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step<'a> {
    phase: &'a str,
    done: u64,
    total: Option<u64>,
    transfer: Option<&'a str>,
}

impl<'a> Step<'a> {
    /// Reads `Phase: NN% (done/total)[, transfer][, done.]` or `Phase: count[, done.]`.
    fn parse(text: &'a str) -> Option<Self> {
        let (phase, rest) = text.split_once(": ")?;
        if phase.is_empty() || !phase.chars().all(|c| c.is_alphabetic() || c == ' ') {
            return None;
        }
        let rest = rest.trim_start();
        let (count, after) = rest.split_once(", ").unwrap_or((rest, ""));
        let (done, total) = match count.split_once('%') {
            Some((_, fraction)) => {
                let fraction = fraction.trim().strip_prefix('(')?.strip_suffix(')')?;
                let (done, total) = fraction.split_once('/')?;
                (done.parse().ok()?, Some(total.parse().ok()?))
            }
            None => (count.trim().parse().ok()?, None),
        };
        let after = after.trim();
        let transfer = after
            .strip_suffix("done.")
            .unwrap_or(after)
            .trim_end()
            .trim_end_matches(',')
            .trim();
        Some(Self {
            phase,
            done,
            total,
            transfer: (!transfer.is_empty()).then_some(transfer),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_progress_lines() {
        let step = |text| Step::parse(text);
        assert_eq!(
            step("Receiving objects:  45% (1234/2742), 1.20 MiB | 2.30 MiB/s"),
            Some(Step {
                phase: "Receiving objects",
                done: 1234,
                total: Some(2742),
                transfer: Some("1.20 MiB | 2.30 MiB/s"),
            })
        );
        assert_eq!(
            step("Writing objects: 100% (3/3), 239 bytes | 239.00 KiB/s, done."),
            Some(Step {
                phase: "Writing objects",
                done: 3,
                total: Some(3),
                transfer: Some("239 bytes | 239.00 KiB/s"),
            })
        );
        assert_eq!(
            step("Enumerating objects: 302, done."),
            Some(Step {
                phase: "Enumerating objects",
                done: 302,
                total: None,
                transfer: None,
            })
        );
        assert_eq!(step("Cloning into 'c1'..."), None);
        assert_eq!(step("Total 3 (delta 1), reused 0 (delta 0)"), None);
        assert_eq!(step("error: something: 12"), None);
    }

    #[test]
    fn each_line_replaces_the_last_and_the_rest_are_notes() {
        let progress = Progress::new();
        let mut reader = progress.reader();
        reader.feed(
            b"Cloning into 'c1'...\nremote: Counting objects:   0% (1/302)        \rremote: Count",
        );
        reader.feed(b"ing objects:  50% (151/302)        \r");
        let report = progress.report();
        assert_eq!(report.phase, "Counting objects");
        assert!(report.remote);
        assert_eq!((report.done, report.total), (151, Some(302)));
        assert_eq!(report.notes, ["Cloning into 'c1'..."]);
        reader.feed(b"Receiving objects:  10% (30/302), 1.00 MiB | 2.00 MiB/s\r");
        reader.feed(b"remote: Create a pull request by visiting:        \n");
        let report = progress.report();
        assert!(!report.remote);
        assert_eq!(report.transfer.as_deref(), Some("1.00 MiB | 2.00 MiB/s"));
        assert_eq!(
            report.notes.last().map(String::as_str),
            Some("remote: Create a pull request by visiting:")
        );
        progress.clear();
        assert_eq!(progress.report(), ProgressReport::default());
    }
}
