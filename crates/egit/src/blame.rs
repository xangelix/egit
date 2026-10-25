//! A file's blame on the text grid: its text (highlighted, as a file is shown), each run of
//! lines from one commit labelled in the gutter (the commit's name, author, and age) and
//! shaded by age, the newest strongest.
//!
//! The blame streams in: a job runs `blame` and hands its runs over to the shared state in
//! batches (every 256 runs, or 50 ms), which the view draws from as they come.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use web_time::Instant;

use bstr::{BStr, BString};
use egit_git::sync::Mutex;
use egit_git::{
    BlameCommit, BlameRun, CancelToken, Git, Moves, ObjectId, Repository, blame, tree_entry,
};
use egit_text::{Measure, Text};
use egui::Color32;

use crate::format;
use crate::grid::{GridSource, Plain, Span};
use crate::history::{FileContent, Objects, read_file, read_work_tree_file};
use crate::jobs;
use crate::problem::Problem;
use crate::theme::Palette;

/// A line no run has reached yet.
const UNKNOWN: u32 = u32::MAX;

/// How many runs (or how long) the job gathers before handing them over.
const BATCH: usize = 256;
const BATCH_TIME: Duration = Duration::from_millis(50);

/// Cells of the gutter's label: a commit's name, its author, and its age.
const LABEL_CELLS: usize = 26;

/// Digits of a commit's name in the gutter.
const ID_DIGITS: usize = 7;

/// The blame found so far.
#[derive(Debug, Default)]
pub(crate) struct BlameState {
    pub(crate) commits: Vec<BlameCommit>,
    /// Each line's commit ([`UNKNOWN`] until found).
    lines: Vec<u32>,
    /// The lines starting a run: their gutter is labelled.
    starts: Vec<bool>,
    /// Lines attributed so far.
    pub(crate) found: u32,
    /// The oldest and newest commits' times, for shading.
    span: Option<(i64, i64)>,
}

impl BlameState {
    /// Takes in runs found, with the commits found so far (only ever more of them: a
    /// commit's details are whole before its first run is handed over).
    fn take(&mut self, commits: &[BlameCommit], runs: &[BlameRun]) {
        let new = commits.get(self.commits.len()..).unwrap_or_default();
        for commit in new.iter().filter(|commit| commit.id.is_some()) {
            self.span = Some(match self.span {
                None => (commit.time, commit.time),
                Some((oldest, newest)) => (oldest.min(commit.time), newest.max(commit.time)),
            });
        }
        self.commits.extend_from_slice(new);
        for run in runs {
            let end = run.line.saturating_add(run.count);
            let Ok(needed) = usize::try_from(end) else {
                continue;
            };
            if self.lines.len() < needed {
                self.lines.resize(needed, UNKNOWN);
                self.starts.resize(needed, false);
            }
            for line in run.line..end {
                if let Ok(at) = usize::try_from(line)
                    && let Some(slot) = self.lines.get_mut(at)
                {
                    *slot = run.commit;
                }
            }
            if let Some(start) = usize::try_from(run.line)
                .ok()
                .and_then(|start| self.starts.get_mut(start))
            {
                *start = true;
            }
            self.found = self.found.saturating_add(run.count);
        }
    }

    /// Line `line`'s commit, once found: its place in [`Self::commits`].
    pub(crate) fn commit_of(&self, line: u32) -> Option<u32> {
        let commit = *self.lines.get(usize::try_from(line).ok()?)?;
        (commit != UNKNOWN).then_some(commit)
    }

    fn starts(&self, line: u32) -> bool {
        usize::try_from(line)
            .ok()
            .and_then(|line| self.starts.get(line))
            .copied()
            .unwrap_or(false)
    }

    /// How new a commit is among those found: 0 the oldest, 1 the newest.
    fn newness(&self, commit: &BlameCommit) -> f32 {
        let Some((oldest, newest)) = self.span else {
            return 0.0;
        };
        let range = newest.saturating_sub(oldest);
        if range <= 0 {
            // All as new as each other: none stands out.
            return 0.0;
        }
        let since = commit.time.saturating_sub(oldest);
        // In thousandths: precision is no concern for a shade.
        let ratio = i128::from(since)
            .saturating_mul(1000)
            .checked_div(i128::from(range))
            .unwrap_or(0)
            .clamp(0, 1000);
        f32::from(u16::try_from(ratio).unwrap_or(0)) / 1000.0
    }
}

/// A blame being found: its state, and whether it is done (or what went wrong).
#[derive(Debug, Default)]
pub(crate) struct Blaming {
    pub(crate) state: Mutex<BlameState>,
    pub(crate) done: AtomicBool,
    pub(crate) problem: Mutex<Option<Problem>>,
}

/// Starts blaming `path` as `at` has it (`None`: the work tree), in a job; `cancel` stops it.
pub(crate) fn start_blame(
    git: Git,
    repository: Arc<Repository>,
    at: Option<ObjectId>,
    path: BString,
    moves: Moves,
    cancel: CancelToken,
) -> Arc<Blaming> {
    let blaming = Arc::new(Blaming::default());
    let shared = Arc::clone(&blaming);
    jobs::detached(async move {
        let mut runs = Vec::with_capacity(BATCH);
        let mut last = Instant::now();
        // The commits found, copied once each (the last batch is handed over after `blame`).
        let mut commits: Vec<BlameCommit> = Vec::new();
        let done = blame(
            &git,
            &repository,
            at,
            path.as_ref(),
            moves,
            &cancel,
            |found, run| {
                commits.extend_from_slice(found.get(commits.len()..).unwrap_or_default());
                runs.push(run);
                if runs.len() >= BATCH || last.elapsed() >= BATCH_TIME {
                    shared.state.lock().take(&commits, &runs);
                    runs.clear();
                    last = Instant::now();
                }
            },
        )
        .await;
        if !runs.is_empty() {
            shared.state.lock().take(&commits, &runs);
        }
        if let Err(error) = done
            && !cancel.is_cancelled()
        {
            *shared.problem.lock() = Some(Problem::git("The file couldn't be blamed", &error));
        }
        shared.done.store(true, Ordering::Release);
    });
    blaming
}

/// Reads the file blamed: as `at` has it at `path` (or the work tree, without `at`).
pub(crate) async fn read_blamed_file(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    at: Option<ObjectId>,
    path: BString,
) -> Result<Arc<FileContent>, Problem> {
    let Some(at) = at else {
        return read_work_tree_file(git, repository, path).await;
    };
    let (finding, found_path) = (Arc::clone(&repository), path.clone());
    let finder = git.clone();
    let entry = jobs::blocking(async move {
        tree_entry(
            &finder,
            &finding,
            at,
            found_path.as_ref(),
            &CancelToken::new(),
        )
        .await
        .map_err(|error| Problem::git("The file couldn't be found", &error))
    })
    .await?
    .ok_or_else(|| {
        Problem::new(
            "The file isn't there",
            format!("{at} has nothing at {}.", BStr::new(&path)),
        )
    })?;
    read_file(git, repository, objects, entry, path).await
}

/// A blamed file as drawn: its text, the blame found so far, and the commit selected.
pub(crate) struct BlameSource<'a> {
    pub(crate) file: &'a Plain,
    pub(crate) state: &'a BlameState,
    pub(crate) selected: Option<u32>,
    /// Now, in seconds since the Unix epoch (for ages).
    pub(crate) now: u64,
    /// Digits of the largest line number.
    pub(crate) digits: usize,
}

impl GridSource for BlameSource<'_> {
    fn text(&self) -> &Text {
        self.file.text()
    }

    fn measure(&self) -> &Measure {
        self.file.measure()
    }

    fn colors(&self, line: u32, palette: &Palette, out: &mut Vec<Span>) {
        self.file.colors(line, palette, out);
    }

    fn begin_frame(&self) {
        self.file.begin_frame();
    }

    fn needs_repaint(&self) -> bool {
        self.file.needs_repaint()
    }

    fn background(&self, line: u32, palette: &Palette) -> Option<Color32> {
        let commit = self.state.commit_of(line)?;
        if Some(commit) == self.selected {
            return Some(palette.current_header);
        }
        let blamed = self.state.commits.get(usize::try_from(commit).ok()?)?;
        if blamed.id.is_none() {
            return Some(palette.incoming);
        }
        // The newest lines stand out; the oldest blend in.
        let newness = self.state.newness(blamed);
        (newness > 0.0).then(|| palette.resolved.gamma_multiply(newness))
    }

    fn gutter(&self, line: u32, out: &mut String) {
        let label = self
            .state
            .starts(line)
            .then(|| self.state.commit_of(line))
            .flatten()
            .and_then(|commit| self.state.commits.get(usize::try_from(commit).ok()?));
        match label {
            Some(commit) => {
                let _ = write!(out, "{:<LABEL_CELLS$}", label_of(commit, self.now));
            }
            None => {
                let _ = write!(out, "{:LABEL_CELLS$}", "");
            }
        }
        let width = self.digits;
        let _ = write!(out, " {:>width$}", u64::from(line).saturating_add(1));
    }

    fn gutter_cells(&self) -> u32 {
        let cells = LABEL_CELLS.saturating_add(self.digits).saturating_add(2);
        u32::try_from(cells).unwrap_or(u32::MAX)
    }
}

/// A run's label: `1a2b3c4 Ada Lovelace  3y`, fitted to [`LABEL_CELLS`].
fn label_of(commit: &BlameCommit, now: u64) -> String {
    let Some(id) = commit.id else {
        return "not committed".to_owned();
    };
    let age =
        u64::try_from(commit.time).map_or_else(|_| String::new(), |time| compact_age(time, now));
    let name = id.to_string();
    let name = name.get(..ID_DIGITS).unwrap_or(&name);
    // The author gets what the name and age leave (a cell between each).
    let room = LABEL_CELLS
        .saturating_sub(ID_DIGITS)
        .saturating_sub(age.chars().count())
        .saturating_sub(2);
    let author: String = if commit.author.chars().count() > room {
        let mut short: String = commit.author.chars().take(room.saturating_sub(1)).collect();
        short.push('…');
        short
    } else {
        commit.author.clone()
    };
    format!("{name} {author:<room$} {age}")
}

/// How long ago `then` was, in a few cells: `now`, `5m`, `3h`, `2d`, `6w`, `4mo`, `3y`.
pub(crate) fn compact_age(then: u64, now: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    const TWO_MONTHS: u64 = 2 * MONTH;
    let ago = now.saturating_sub(then);
    let (unit, name) = match ago {
        0..MINUTE => return "now".to_owned(),
        MINUTE..HOUR => (MINUTE, "m"),
        HOUR..DAY => (HOUR, "h"),
        DAY..WEEK => (DAY, "d"),
        WEEK..TWO_MONTHS => (WEEK, "w"),
        TWO_MONTHS..YEAR => (MONTH, "mo"),
        _ => (YEAR, "y"),
    };
    format!("{}{name}", ago.checked_div(unit).unwrap_or(0))
}

/// Digits of the largest line number of `file`.
pub(crate) fn digits_of(file: &Plain) -> usize {
    usize::try_from(file.measure().lines().max(1).ilog10().saturating_add(1)).unwrap_or(1)
}

/// What `when` is, for people, in the details of a blamed line.
pub(crate) fn when(commit: &BlameCommit) -> String {
    u64::try_from(commit.time)
        .map_or_else(|_| String::new(), |time| format::ago(time, format::now()))
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn commit(byte: char, author: &str, time: i64) -> BlameCommit {
        BlameCommit {
            id: ObjectId::from_hex(byte.to_string().repeat(40).as_bytes()),
            author: author.to_owned(),
            time,
            ..BlameCommit::default()
        }
    }

    #[test]
    fn runs_mark_their_lines_and_first_line() {
        let commits = [commit('a', "Ada", 100), commit('b', "Bea", 300)];
        let mut state = BlameState::default();
        state.take(
            &commits,
            &[
                BlameRun {
                    commit: 1,
                    line: 2,
                    count: 2,
                    original: 0,
                },
                BlameRun {
                    commit: 0,
                    line: 0,
                    count: 2,
                    original: 0,
                },
            ],
        );
        assert_eq!(state.found, 4);
        assert_eq!(
            (0..5).map(|line| state.commit_of(line)).collect::<Vec<_>>(),
            [Some(0), Some(0), Some(1), Some(1), None]
        );
        assert!(state.starts(0) && !state.starts(1) && state.starts(2));
        assert_eq!(state.newness(&commits[0]), 0.0);
        assert_eq!(state.newness(&commits[1]), 1.0);
    }

    #[test]
    fn a_label_fits_its_cells() {
        let now = 400 * 86_400;
        let label = label_of(
            &commit('a', "Ada Lovelace-Byron of Ockham", 30 * 86_400),
            now,
        );
        assert_eq!(label.chars().count(), LABEL_CELLS);
        assert!(label.starts_with("aaaaaaa Ada Lovelace"), "{label}");
        assert!(label.ends_with("… 1y"), "{label}");
        let short = label_of(&commit('b', "Bo", now.cast_signed() - 120), now);
        assert_eq!(short.chars().count(), LABEL_CELLS);
        assert!(short.ends_with(" 2m"), "{short}");
        let uncommitted = BlameCommit::default();
        assert_eq!(label_of(&uncommitted, now), "not committed");
    }

    #[test]
    fn ages_are_short() {
        let day = 86_400;
        for (ago, shown) in [
            (30, "now"),
            (600, "10m"),
            (7200, "2h"),
            (3 * day, "3d"),
            (20 * day, "2w"),
            (90 * day, "3mo"),
            (800 * day, "2y"),
        ] {
            assert_eq!(compact_age(1_000_000_000 - ago, 1_000_000_000), shown);
        }
    }
}
