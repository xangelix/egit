//! A CI/CD job's log, followed while the job runs: a thread of its own asks how the job stands
//! and reads what the log gained since (`egit_forge::read_log`: a range where the host answers
//! ranges, else the whole skipped as far as was read), every [`EVERY`] (longer for a large log
//! sent whole), parses it (`egit_forge::JobLog`, in its forge's way), and lays it out for the
//! text grid ([`LogSource`]: measured, its colours, each line's time, its sections), handing
//! each to the view as it lands. Once the job has ended and its log is read whole, it stops;
//! dropped, it stops at once.
//!
//! GitHub gives a job's log once the job has ended (as a file it points to): while a GitHub job
//! runs, it is asked about (its steps) every [`EVERY`] and its log looked for only every
//! [`GITHUB_LOG_EVERY`]th time; once it has ended, its log is looked for a few times more
//! ([`LOOKS_AFTER_END`]), as GitHub writes it out a little after.

use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use egit_forge::{
    Account, ForgeError, ForgeKind, ForgeRepository, JobLog, JobRun, LogMark, LogParts, LogRead,
    Run, Section, job, read_log, with_steps,
};
use egit_git::sync::Mutex;
use egit_git::{CancelToken, Style};
use egit_text::{Measure, Text};
use egui::Color32;

use crate::grid::{GridSource, Span};
use crate::problem::Problem;
use crate::theme::Palette;

/// How often a running job is asked about, and its log read again.
pub(crate) const EVERY: Duration = Duration::from_secs(2);

/// The longest between reads: a large log is read less often (a second more for each MiB the
/// host sends whole, or for each 4 MiB laid out again).
const LONGEST: Duration = Duration::from_secs(15);

/// How long before trying again after a failure.
const AFTER_FAILING: Duration = Duration::from_secs(10);

/// While a GitHub job runs, its log is looked for every this many times it is asked about.
const GITHUB_LOG_EVERY: u32 = 5;

/// How many times more a GitHub job's log is looked for once the job has ended without one.
const LOOKS_AFTER_END: u32 = 5;

/// A log laid out for the text grid: its text measured, styles, each line's time (where the
/// runner wrote them), its sections (a GitHub job's steps among them, each a line of its own:
/// [`with_steps`]), and the lines its runner marks as errors or warnings. Each section's header
/// has a caret in the gutter (▾, or ▸ once folded: [`crate::log_folds`]) and how long it took at
/// its right.
#[derive(Debug)]
pub(crate) struct LogSource {
    text: Text,
    measure: Measure,
    runs: Vec<Run>,
    times: Vec<i64>,
    pub(crate) sections: Vec<Section>,
    pub(crate) marks: Vec<LogMark>,
    /// Whether the log was longer than egit keeps.
    pub(crate) truncated: bool,
    /// The viewer's time zone, for each line's time.
    zone: jiff::tz::TimeZone,
    /// How it was measured, for its folded views.
    pub(crate) options: egit_text::Options,
}

impl LogSource {
    /// `parts`, measured (on the calling thread: the follower's).
    pub(crate) fn of(parts: LogParts, options: egit_text::Options) -> Option<Self> {
        let text = Text::new(parts.text).ok()?;
        let measure = Measure::all(&text, options);
        Some(Self {
            text,
            measure,
            runs: parts.runs,
            times: parts.times,
            sections: parts.sections,
            marks: parts.marks,
            truncated: parts.truncated,
            zone: crate::format::local_zone(),
            options,
        })
    }

    /// The outermost section whose header is line `line`, if any (the sections are in the
    /// order of their headers).
    pub(crate) fn header(&self, line: u32) -> Option<&Section> {
        self.header_place(line)
            .and_then(|place| self.sections.get(place))
    }

    /// The place of the outermost section whose header is line `line`, if any.
    pub(crate) fn header_place(&self, line: u32) -> Option<usize> {
        let at = self.sections.partition_point(|section| section.line < line);
        self.sections
            .get(at)
            .is_some_and(|section| section.line == line)
            .then_some(at)
    }

    /// The last line section `section` holds: where it ended, or the log's last while it runs.
    pub(crate) fn last_line(&self, section: &Section) -> u32 {
        section
            .ended
            .map_or_else(|| self.measure.lines().saturating_sub(1), |(_, last)| last)
    }

    /// Whether `section` holds lines below its header, which folding it would hide.
    pub(crate) fn folds(&self, section: &Section) -> bool {
        self.last_line(section) > section.line
    }

    /// The innermost section holding line `line`.
    pub(crate) fn section_at(&self, line: u32) -> Option<usize> {
        let before = self
            .sections
            .partition_point(|section| section.line <= line);
        (0..before).rev().find(|at| {
            self.sections
                .get(*at)
                .is_some_and(|section| self.last_line(section) >= line)
        })
    }

    /// How long `section` took, or has so far (none without times).
    pub(crate) fn took(section: &Section, now: i64) -> Option<u64> {
        if section.started == 0 {
            return None;
        }
        let until = section.ended.map_or(now, |(ended, _)| ended);
        u64::try_from(until.saturating_sub(section.started)).ok()
    }

    /// Line `line`'s gutter: its number, its time where the runner wrote one, and a section's
    /// caret (`folded` or not).
    pub(crate) fn gutter_of(&self, line: u32, folded: bool, out: &mut String) {
        use std::fmt::Write as _;
        let _ = write!(out, "{}", u64::from(line).saturating_add(1));
        let time = usize::try_from(line)
            .ok()
            .and_then(|line| self.times.get(line))
            .copied()
            .filter(|time| *time != 0);
        if let Some(time) = time {
            let offset = i64::from(crate::format::offset_minutes(time, &self.zone));
            let local = time.saturating_add(offset.saturating_mul(60));
            let of_day = local.rem_euclid(86_400);
            let _ = write!(
                out,
                " {:02}:{:02}:{:02}",
                of_day.div_euclid(3_600),
                of_day.div_euclid(60).rem_euclid(60),
                of_day.rem_euclid(60)
            );
        } else if !self.times.is_empty() {
            out.push_str("         ");
        }
        if !self.sections.is_empty() {
            let caret = match self.header(line) {
                Some(section) if self.folds(section) => {
                    if folded {
                        " ▸"
                    } else {
                        " ▾"
                    }
                }
                _ => "  ",
            };
            out.push_str(caret);
        }
    }

    /// What shows at the right of line `line`: a section's header, how long it took.
    pub(crate) fn trailing_of(&self, line: u32) -> Option<String> {
        let section = self.header(line)?;
        let now = i64::try_from(crate::format::now()).unwrap_or(i64::MAX);
        let took = crate::format::duration(Self::took(section, now)?);
        Some(if section.ended.is_some() {
            took
        } else {
            format!("{took} so far")
        })
    }

    /// The style in effect at byte `at`, and where the next run starts (its place).
    fn style_at(&self, at: u32) -> (Style, usize) {
        let next = self.runs.partition_point(|run| run.at <= at);
        let style = next
            .checked_sub(1)
            .and_then(|current| self.runs.get(current))
            .map_or_else(Style::default, |run| run.style);
        (style, next)
    }
}

/// A style's colour on the palette: its own, or the text's; faint, nearer the background.
fn style_color(style: Style, palette: &Palette) -> Option<Color32> {
    let color = style
        .color
        .map(|color| crate::views::terminal_color(color, *palette));
    if style.dim {
        return Some(
            color
                .unwrap_or(palette.text)
                .lerp_to_gamma(palette.background, 0.4),
        );
    }
    color
}

impl GridSource for LogSource {
    fn text(&self) -> &Text {
        &self.text
    }

    fn measure(&self) -> &Measure {
        &self.measure
    }

    fn colors(&self, line: u32, palette: &Palette, out: &mut Vec<Span>) {
        let Some(start) = self.text.start(line) else {
            return;
        };
        let end = self
            .text
            .start(line.saturating_add(1))
            .unwrap_or_else(|| u32::try_from(self.text.bytes().len()).unwrap_or(u32::MAX));
        let (mut style, mut next) = self.style_at(start);
        let mut from = start;
        loop {
            let until = self
                .runs
                .get(next)
                .map_or(end, |run| run.at.min(end))
                .max(from);
            if let Some(color) = style_color(style, palette)
                && until > from
            {
                out.push(Span {
                    bytes: from.saturating_sub(start)..until.saturating_sub(start),
                    color,
                });
            }
            let Some(run) = self.runs.get(next).filter(|run| run.at < end) else {
                break;
            };
            style = run.style;
            from = run.at;
            next = next.saturating_add(1);
        }
    }

    fn background(&self, line: u32, palette: &Palette) -> Option<Color32> {
        // A section's header stands out as GitLab's does: a step's (a depth of none) more.
        self.header(line).map(|section| {
            if section.depth == 0 {
                palette.surface_alt
            } else {
                palette.surface
            }
        })
    }

    fn gutter(&self, line: u32, out: &mut String) {
        self.gutter_of(line, false, out);
    }

    fn gutter_cells(&self) -> u32 {
        // At least five digits: a log growing past 9,999 lines doesn't move its text.
        let number = self
            .measure
            .lines()
            .max(1)
            .ilog10()
            .saturating_add(2)
            .max(6);
        let timed = if self.times.is_empty() { 0 } else { 9 };
        let caret = if self.sections.is_empty() { 0 } else { 2 };
        number.saturating_add(timed).saturating_add(caret)
    }

    fn trailing(&self, line: u32) -> Option<String> {
        self.trailing_of(line)
    }
}

/// Where following a job's log stands, as its thread last said.
#[derive(Debug, Default)]
pub(crate) struct Following {
    /// How the job stands.
    pub(crate) job: Option<JobRun>,
    /// Its log as read so far, laid out.
    pub(crate) log: Option<Arc<LogSource>>,
    /// Bumped with each log laid out anew.
    pub(crate) version: u64,
    /// Whether it has no log (not started yet, or erased).
    pub(crate) missing: bool,
    /// Why the last read failed, until one goes through.
    pub(crate) problem: Option<Problem>,
    /// Whether the job has ended and its log was read whole.
    pub(crate) done: bool,
}

/// A job's log followed by a thread of its own; dropped, it stops.
#[derive(Debug)]
pub(crate) struct JobFollower {
    shared: Arc<Mutex<Following>>,
    cancel: CancelToken,
}

impl Drop for JobFollower {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl JobFollower {
    /// Follows job `id` of `repository` through `account`, waking `waker` as each read lands.
    pub(crate) fn start(
        (account, repository): (Arc<Account>, ForgeRepository),
        id: u64,
        waker: crate::repaint::Repaint,
    ) -> Self {
        let shared = Arc::new(Mutex::new(Following::default()));
        let cancel = CancelToken::new();
        let follower = Follower {
            account,
            repository,
            id,
            shared: Arc::clone(&shared),
            cancel: cancel.clone(),
            waker,
            options: crate::preferences::text_options(),
        };
        let started = egit_git::exec::spawn(&format!("job {id}'s log"), follower.run());
        if let Err(error) = started {
            shared.lock().problem = Some(Problem::new(
                "The job's log couldn't be followed",
                error.to_string(),
            ));
        }
        Self { shared, cancel }
    }

    /// Where it stands.
    pub(crate) fn following(&self) -> egit_git::sync::MutexGuard<'_, Following> {
        self.shared.lock()
    }
}

/// The thread's side.
struct Follower {
    account: Arc<Account>,
    repository: ForgeRepository,
    id: u64,
    shared: Arc<Mutex<Following>>,
    cancel: CancelToken,
    waker: crate::repaint::Repaint,
    options: egit_text::Options,
}

/// How long to wait before reading a log again: a large one is read less often, a second more
/// for each MiB sent whole each time (`read` so far), or for each 4 MiB laid out again (sent
/// as `ranges`).
fn pause_after(read: u64, ranges: bool) -> Duration {
    let mib = read.checked_div(1024 * 1024).unwrap_or(0);
    let extra = if ranges {
        mib.checked_div(4).unwrap_or(0)
    } else {
        mib
    };
    EVERY
        .saturating_add(Duration::from_secs(extra))
        .min(LONGEST)
}

impl Follower {
    /// Asks, reads, lays out, and waits, until the job has ended and its log is whole (or it
    /// is dropped).
    async fn run(self) {
        let github = self.repository.host().kind() == ForgeKind::GitHub;
        let mut log = JobLog::of(self.repository.host().kind());
        let (mut read, mut ranges) = (0_u64, true);
        // GitHub's: how many times it was asked about, and its log looked for once it ended.
        let (mut asked, mut looked_after) = (0_u32, 0_u32);
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            let ended = match job(&self.account, &self.repository, self.id, &self.cancel).await {
                Ok(run) => {
                    let ended = !run.status.is_moving();
                    self.shared.lock().job = Some(run);
                    ended
                }
                Err(ForgeError::Cancelled) => return,
                Err(error) => {
                    self.failed(&error);
                    if !self.wait(AFTER_FAILING).await {
                        return;
                    }
                    continue;
                }
            };
            asked = asked.wrapping_add(1);
            if github && !ended && asked.checked_rem(GITHUB_LOG_EVERY) != Some(1) {
                // Its steps moved on; its log is looked for now and then.
                self.waker.request_repaint();
                if !self.wait(EVERY).await {
                    return;
                }
                continue;
            }
            let mut sink = |bytes: &[u8]| log.feed(bytes);
            let took = read_log(
                &self.account,
                &self.repository,
                (self.id, read, ranges),
                &self.cancel,
                &mut sink,
            )
            .await;
            match took {
                Ok(LogRead::Read { new, ranged }) => {
                    // A host sending it whole, asked for a range: asked whole (compressed)
                    // from then on.
                    if read > 0 && !ranged {
                        ranges = false;
                    }
                    read = read.saturating_add(new);
                    if new > 0 || ended || self.shared.lock().log.is_none() {
                        if ended {
                            log.finish();
                        }
                        self.lay_out(&log, ended);
                    }
                }
                Ok(LogRead::Shorter) => {
                    log = JobLog::of(self.repository.host().kind());
                    read = 0;
                    continue;
                }
                Ok(LogRead::Missing) => {
                    // GitHub writes an ended job's log out a little after.
                    let again = github && ended && looked_after < LOOKS_AFTER_END;
                    looked_after = looked_after.saturating_add(u32::from(ended));
                    {
                        let mut shared = self.shared.lock();
                        shared.missing = read == 0;
                        shared.problem = None;
                        shared.done = ended && !again;
                    }
                    self.waker.request_repaint();
                    if again {
                        if !self.wait(EVERY).await {
                            return;
                        }
                        continue;
                    }
                }
                Err(ForgeError::Cancelled) => return,
                Err(error) => {
                    self.failed(&error);
                    if !self.wait(AFTER_FAILING).await {
                        return;
                    }
                    continue;
                }
            }
            if ended || !self.wait(pause_after(read, ranges)).await {
                return;
            }
        }
    }

    /// Lays `log` out for the grid (`whole`: the job ended) and hands it over, a GitHub job's
    /// steps made its sections.
    fn lay_out(&self, log: &JobLog, whole: bool) {
        let steps = self
            .shared
            .lock()
            .job
            .as_ref()
            .map_or_default(|run| run.steps.clone());
        let mut parts = log.parts();
        if !steps.is_empty() {
            parts = with_steps(parts, &steps);
        }
        let source = LogSource::of(parts, self.options);
        let mut shared = self.shared.lock();
        if let Some(source) = source {
            shared.log = Some(Arc::new(source));
            shared.version = shared.version.wrapping_add(1);
        }
        shared.missing = false;
        shared.problem = None;
        shared.done = whole;
        drop(shared);
        self.waker.request_repaint();
    }

    /// Notes why a read failed.
    fn failed(&self, error: &ForgeError) {
        self.shared.lock().problem = Some(Problem::new(
            "The job's log couldn't be read",
            error.to_string(),
        ));
        self.waker.request_repaint();
    }

    /// Waits `pause`, unless dropped meanwhile (then false).
    async fn wait(&self, pause: Duration) -> bool {
        let until = Instant::now().checked_add(pause);
        while until.is_some_and(|until| Instant::now() < until) {
            if self.cancel.is_cancelled() {
                return false;
            }
            egit_git::exec::sleep(Duration::from_millis(100)).await;
        }
        !self.cancel.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use egit_git::Color;

    use super::*;

    #[test]
    fn a_line_s_colours_follow_its_runs() {
        let mut log = JobLog::new();
        log.feed(b"plain \x1b[31mred\x1b[0m end\n\x1b[2mfaint\n");
        let source = LogSource::of(log.parts(), egit_text::Options::default()).unwrap();
        let palette = Palette::DARK;
        let mut spans = Vec::new();
        source.colors(0, &palette, &mut spans);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].bytes, 6..9);
        assert_eq!(
            spans[0].color,
            crate::views::terminal_color(Color::Indexed(1), palette)
        );
        spans.clear();
        source.colors(1, &palette, &mut spans);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].bytes, 0..5);
    }

    #[test]
    fn the_gutter_holds_each_line_s_time() {
        let mut log = JobLog::new();
        log.feed(
            b"2024-05-14T11:19:20.000000Z 00O first\n2024-05-14T11:19:21.000000Z 00O second\n",
        );
        let mut source = LogSource::of(log.parts(), egit_text::Options::default()).unwrap();
        source.zone = jiff::tz::TimeZone::UTC;
        let mut gutter = String::new();
        source.gutter(1, &mut gutter);
        assert_eq!(gutter, "2 11:19:21");
        // At least five digits' room: a log growing past 9,999 lines doesn't move its text.
        assert_eq!(source.gutter_cells(), 15);
    }
}
