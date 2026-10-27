//! A CI/CD job's log: how the job stands (GitLab's `GET /projects/:path/jobs/:id`; GitHub's
//! `GET /repos/:owner/:name/actions/jobs/:id`, with its steps), its log read a part at a time as
//! it grows (GitLab's `…/jobs/:id/trace`; GitHub's, a file it points to, once there is one), and
//! parsed as the forge's own page shows it.
//!
//! GitLab's API sends a log whole. egit asks for only the bytes after those it has (`Range`,
//! uncompressed); where the host sends it all anyway, it skips those as they come (and asks
//! compressed from then on, as a range of compressed bytes is another's), as `glab ci trace`
//! does, never holding the log twice.
//!
//! [`JobLog`] takes what a runner writes: each line's timestamp (`2024-05-14T11:19:20.000000Z
//! 00O `, a `+` in the space's place continuing the line before) taken off and kept, colours
//! (SGR) as style runs, other escapes dropped, a `\r` rewriting the line under way as a terminal
//! does (progress), and sections (`section_start:<time>:<name>[collapsed=true]\r\e[0K`,
//! `section_end:<time>:<name>\r\e[0K`) noted, their markers out of the text. A GitHub job's
//! ([`JobLog::github`]) has each line's time before it (`2024-05-14T11:19:20.1234567Z `), its
//! groups (`##[group]Run make`, `##[endgroup]`) as sections, and the lines its workflow
//! commands make (`##[error]`, `##[warning]`, `##[notice]`, `##[command]`, `##[debug]`) said
//! and coloured as GitHub shows them. At most [`MOST_TEXT`] bytes of text are kept; the rest
//! is said to be on the forge.

use bstr::ByteSlice as _;
use egit_git::{CancelToken, Style, apply_sgr, escape_sequence};
use serde::Deserialize;

use crate::account::Account;
use crate::answer::{parse_time, read_answer};
use crate::error::ForgeError;
use crate::gitlab::encoded;
use crate::host::ForgeKind;
use crate::pipelines::{CiStatus, github_job, github_log, github_step_log};
use crate::repository::ForgeRepository;

/// The most of a log's text kept (GitLab's own limit is 100 MiB by default).
pub const MOST_TEXT: usize = 128 * 1024 * 1024;

/// The most of an answer read at once: a whole log, sent again.
const MOST_READ: u64 = 512 * 1024 * 1024;

/// A job, as it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRun {
    /// Its number.
    pub id: u64,
    /// Its name.
    pub name: String,
    /// Its stage's name.
    pub stage: Option<String>,
    /// Where it stands.
    pub status: CiStatus,
    /// When it started and finished (seconds since the Unix epoch), as far as it has.
    pub started: Option<i64>,
    /// When it finished.
    pub finished: Option<i64>,
    /// How long it ran, in seconds, once it has finished.
    pub duration: Option<f64>,
    /// Why it failed, as GitLab names it (`script_failure`).
    pub failure: Option<String>,
    /// Its page.
    pub url: Option<String>,
    /// Its steps, where the forge says (GitHub's), in order.
    pub steps: Vec<JobStep>,
}

/// A step of a job (GitHub's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobStep {
    /// Its place among the job's steps (from 1).
    pub number: u32,
    /// Its name ("Run actions/checkout@v4").
    pub name: String,
    /// Where it stands.
    pub status: CiStatus,
    /// When it started (seconds since the Unix epoch), if it has.
    pub started: Option<i64>,
    /// When it finished, if it has.
    pub finished: Option<i64>,
}

#[derive(Deserialize)]
struct RestJob {
    id: u64,
    name: Option<String>,
    stage: Option<String>,
    status: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration: Option<f64>,
    failure_reason: Option<String>,
    web_url: Option<String>,
}

/// How job `id` of `repository` stands (a GitHub job's with its steps).
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub async fn job(
    account: &Account,
    repository: &ForgeRepository,
    id: u64,
    cancel: &CancelToken,
) -> Result<JobRun, ForgeError> {
    if repository.host().kind() == ForgeKind::GitHub {
        return github_job(account, repository, id, cancel).await;
    }
    let what = format!("{} job {id}", repository.full_name());
    let url = account.host().rest_url(&format!(
        "/projects/{}/jobs/{id}",
        encoded(repository.path())
    ));
    let answer = account.send(&url, None, cancel).await?;
    let job: RestJob = read_answer(account, &answer, &what)?;
    let time = |text: Option<String>| text.as_deref().and_then(parse_time);
    Ok(JobRun {
        id: job.id,
        name: job.name.unwrap_or_default(),
        stage: job.stage,
        status: job
            .status
            .as_deref()
            .map_or(CiStatus::Unknown, CiStatus::parse),
        started: time(job.started_at),
        finished: time(job.finished_at),
        duration: job.duration,
        failure: job.failure_reason,
        url: job.web_url,
        steps: Vec::new(),
    })
}

/// How reading a log went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogRead {
    /// Read: how many new bytes were handed over, and whether the host sent only those (it
    /// answers ranges).
    Read {
        /// The new bytes handed over.
        new: u64,
        /// Whether the host sent only those.
        ranged: bool,
    },
    /// The host has less than was read before: the log was erased, or begun again; it is to be
    /// read from the start.
    Shorter,
    /// The job has no log (not started yet, or erased).
    Missing,
}

/// Reads job `id`'s log after its first `from` bytes, handing the new ones to `sink` as they
/// come.
///
/// GitLab's is asked as a range where `ranges` (the host may answer them), else the whole
/// skipped as far as `from`. A GitHub job's is a file GitHub points to, whole as far as it is
/// written: none before the job ends ([`LogRead::Missing`]), when its steps' are read instead
/// ([`read_step_log`]).
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub async fn read_log(
    account: &Account,
    repository: &ForgeRepository,
    (id, from, ranges): (u64, u64, bool),
    cancel: &CancelToken,
    sink: &mut (dyn FnMut(&[u8]) + Send),
) -> Result<LogRead, ForgeError> {
    if repository.host().kind() == ForgeKind::GitHub {
        return github_log(account, repository, (id, from), cancel, sink).await;
    }
    let url = account.host().rest_url(&format!(
        "/projects/{}/jobs/{id}/trace",
        encoded(repository.path())
    ));
    let asked = (ranges && from > 0).then_some(from);
    // How many of the log's bytes went by (a whole log's), how many were handed over, and
    // whether a range came from elsewhere than asked.
    let (mut seen, mut given, mut misplaced) = (0_u64, 0_u64, false);
    let streamed = account
        .stream(
            &url,
            (asked, MOST_READ),
            &mut |status, content_range, part| {
                let length = u64::try_from(part.len()).unwrap_or(u64::MAX);
                if status == 206 {
                    // `bytes 120-4095/4096`: only bytes from where asked will do.
                    let start = content_range
                        .and_then(|range| range.strip_prefix("bytes "))
                        .and_then(|range| range.split('-').next())
                        .and_then(|start| start.trim().parse::<u64>().ok());
                    if start != Some(from) {
                        misplaced = true;
                        return;
                    }
                    given = given.saturating_add(length);
                    sink(part);
                    return;
                }
                let skip = usize::try_from(from.saturating_sub(seen)).unwrap_or(usize::MAX);
                seen = seen.saturating_add(length);
                if let Some(new) = part.get(skip..).filter(|new| !new.is_empty()) {
                    given = given.saturating_add(u64::try_from(new.len()).unwrap_or(u64::MAX));
                    sink(new);
                }
            },
            cancel,
        )
        .await?;
    match streamed.status {
        206 if misplaced => Ok(LogRead::Shorter),
        206 => Ok(LogRead::Read {
            new: given,
            ranged: true,
        }),
        // Nothing past what was read.
        416 => Ok(LogRead::Read {
            new: 0,
            ranged: true,
        }),
        200..=299 if seen < from => Ok(LogRead::Shorter),
        200..=299 => Ok(LogRead::Read {
            new: given,
            ranged: false,
        }),
        404 => Ok(LogRead::Missing),
        status => Err(ForgeError::Status {
            host: account.host().name().to_owned(),
            status,
            message: crate::answer::reason(&streamed.said),
        }),
    }
}

/// Reads a GitHub job's step's own log as [`read_log`] reads the whole job's.
///
/// Job `id`'s step at `position` (its place among the job's steps, from 0), after its first
/// `from` bytes, the new ones handed to `sink` as they come: a file of the step's own, there
/// once the step has finished, while the whole job's isn't until the job has ended. None for a
/// GitLab job, which has no steps ([`LogRead::Missing`]).
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub async fn read_step_log(
    account: &Account,
    repository: &ForgeRepository,
    (id, position, from): (u64, usize, u64),
    cancel: &CancelToken,
    sink: &mut (dyn FnMut(&[u8]) + Send),
) -> Result<LogRead, ForgeError> {
    if repository.host().kind() != ForgeKind::GitHub {
        return Ok(LogRead::Missing);
    }
    github_step_log(account, repository, (id, position, from), cancel, sink).await
}

/// A style taking over from a byte of the text on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Where, in the text.
    pub at: u32,
    /// The style.
    pub style: Style,
}

/// What a line of a log says of the job, where its runner marks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    /// An error: GitHub's `##[error]`, a GitLab runner's `ERROR:` (its job failing, say).
    Error,
    /// A warning: GitHub's `##[warning]`, a GitLab runner's `WARNING:`.
    Warning,
}

/// A line of a log its runner marks as an error or a warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogMark {
    /// The line.
    pub line: u32,
    /// What it says.
    pub kind: MarkKind,
}

/// A section of a log, as its markers name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Its name (`step_script`).
    pub name: String,
    /// Its header's line (the one it starts on).
    pub line: u32,
    /// When it started (seconds since the Unix epoch).
    pub started: i64,
    /// When it ended, and its last line, once it has.
    pub ended: Option<(i64, u32)>,
    /// Whether its forge shows it folded (GitLab's `[collapsed=true]`, every GitHub group, a
    /// GitHub step that didn't fail).
    pub collapsed: bool,
    /// How many sections it is inside.
    pub depth: u16,
    /// How it went, where its forge says: a GitHub job's step ([`with_steps`]).
    pub status: Option<CiStatus>,
}

/// Whose log it is: how its lines are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Dialect {
    /// A GitLab runner's: `section_start`/`section_end` markers, timestamps with a stream.
    #[default]
    GitLab,
    /// GitHub Actions': timestamps alone, `##[group]` and the other workflow commands.
    GitHub,
}

/// A log parsed as it comes: its text (lines ending in `\n`), styles, each line's time where
/// the runner wrote one, and sections.
#[derive(Clone, Debug, Default)]
pub struct JobLog {
    dialect: Dialect,
    text: Vec<u8>,
    runs: Vec<Run>,
    /// Each line's time (seconds since the Unix epoch; 0 for none), once any line had one.
    times: Vec<i64>,
    /// Whether any line had one.
    timed: bool,
    sections: Vec<Section>,
    /// The lines its runner marks as errors or warnings, in order.
    marks: Vec<LogMark>,
    /// The sections open, innermost last.
    open: Vec<usize>,
    /// How many lines it holds.
    lines: u32,
    /// The style the text so far ends in.
    style: Style,
    /// The line under way: its bytes so far (no `\n` yet).
    raw: Vec<u8>,
    /// A timestamped line taken off its prefix, kept until the next shows whether it continues
    /// it (`+`): its bytes and time.
    pending: Option<(Vec<u8>, i64)>,
    /// Whether the text reached [`MOST_TEXT`] (the rest was left out).
    truncated: bool,
}

/// A parsed log's parts, for drawing.
#[derive(Clone, Debug, Default)]
pub struct LogParts {
    /// The text: lines, each ending in `\n`.
    pub text: Vec<u8>,
    /// The styles, each from a byte of the text on.
    pub runs: Vec<Run>,
    /// Each line's time (seconds since the Unix epoch; 0 for none); empty where the runner
    /// wrote none.
    pub times: Vec<i64>,
    /// The sections, in the order they start.
    pub sections: Vec<Section>,
    /// The lines its runner marks as errors or warnings, in order.
    pub marks: Vec<LogMark>,
    /// Whether the text reached [`MOST_TEXT`] (the rest was left out).
    pub truncated: bool,
}

impl JobLog {
    /// None yet: a GitLab job's.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// None yet: a GitHub Actions job's.
    #[must_use]
    pub fn github() -> Self {
        Self {
            dialect: Dialect::GitHub,
            ..Self::default()
        }
    }

    /// None yet: a job's of `kind`.
    #[must_use]
    pub fn of(kind: ForgeKind) -> Self {
        match kind {
            ForgeKind::GitLab => Self::new(),
            ForgeKind::GitHub => Self::github(),
        }
    }

    /// Takes in `bytes`, the log's next.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some(end) = rest.find_byte(b'\n') {
            let (line, after) = rest.split_at(end);
            self.raw.extend_from_slice(line);
            let raw = std::mem::take(&mut self.raw);
            self.raw_line(&raw);
            self.raw = raw;
            self.raw.clear();
            rest = after.get(1..).unwrap_or_default();
        }
        self.raw.extend_from_slice(rest);
    }

    /// Takes in what is under way as ended (the log is whole).
    pub fn finish(&mut self) {
        if !self.raw.is_empty() {
            let raw = std::mem::take(&mut self.raw);
            self.raw_line(&raw);
        }
        if let Some((content, time)) = self.pending.take() {
            self.commit(&content, time);
        }
    }

    /// Ends a part of the log read on its own (a GitHub step's: [`read_step_log`]) so the next
    /// starts apart: its line under way taken as ended, and the groups it left open (a step
    /// read as it ran) ended at its last line.
    pub fn end_part(&mut self) {
        self.finish();
        let last = self.lines.saturating_sub(1);
        let time = self.times.last().copied().unwrap_or(0);
        for open in self.open.drain(..) {
            if let Some(section) = self.sections.get_mut(open) {
                section.ended = Some((time.max(section.started), last.max(section.line)));
            }
        }
    }

    /// How many lines it holds, the line under way left out.
    #[must_use]
    pub const fn lines(&self) -> u32 {
        self.lines
    }

    /// Its parts as they stand, the line under way taken as ended.
    #[must_use]
    pub fn parts(&self) -> LogParts {
        let mut whole = self.clone();
        whole.finish();
        LogParts {
            text: whole.text,
            runs: whole.runs,
            times: whole.times,
            sections: whole.sections,
            marks: whole.marks,
            truncated: whole.truncated,
        }
    }

    /// How many bytes of text it holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.text.len()
    }

    /// Whether it holds no text.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// One line as the runner wrote it (without its `\n`): its timestamp taken off, and joined
    /// to the one before when it continues it.
    fn raw_line(&mut self, raw: &[u8]) {
        if self.dialect == Dialect::GitHub {
            self.github_line(raw);
            return;
        }
        match timestamp(raw) {
            Some((time, true, content)) => {
                if let Some((pending, _)) = &mut self.pending {
                    pending.extend_from_slice(content);
                } else {
                    self.pending = Some((content.to_vec(), time));
                }
            }
            Some((time, false, content)) => {
                if let Some((pending, at)) = self.pending.take() {
                    self.commit(&pending, at);
                }
                self.pending = Some((content.to_vec(), time));
            }
            None => {
                if let Some((pending, at)) = self.pending.take() {
                    self.commit(&pending, at);
                }
                self.commit(raw, 0);
            }
        }
    }

    /// One line of a GitHub job's log: its time taken off; a group's start a section (its line
    /// the group's name), its end closing it; a workflow command's line said as GitHub says it.
    fn github_line(&mut self, raw: &[u8]) {
        // The file starts with a byte order mark.
        let raw = raw.strip_prefix("\u{feff}".as_bytes()).unwrap_or(raw);
        let (time, content) = github_timestamp(raw).unwrap_or((0, raw));
        if let Some(name) = content.strip_prefix(b"##[group]") {
            let depth = u16::try_from(self.open.len()).unwrap_or(u16::MAX);
            self.open.push(self.sections.len());
            self.sections.push(Section {
                name: String::from_utf8_lossy(name).trim().to_owned(),
                line: self.lines,
                started: time,
                ended: None,
                collapsed: true,
                depth,
                status: None,
            });
            self.commit(name, time);
            return;
        }
        if content.starts_with(b"##[endgroup]") {
            if let Some(open) = self.open.pop()
                && let Some(section) = self.sections.get_mut(open)
            {
                let last = self.lines.saturating_sub(1).max(section.line);
                section.ended = Some((time, last));
            }
            return;
        }
        let said: [Command; 5] = [
            (b"##[error]", b"Error: ", b"\x1b[31m", Some(MarkKind::Error)),
            (
                b"##[warning]",
                b"Warning: ",
                b"\x1b[33m",
                Some(MarkKind::Warning),
            ),
            (b"##[notice]", b"Notice: ", b"\x1b[36m", None),
            (b"##[command]", b"", b"\x1b[34m", None),
            (b"##[debug]", b"", b"\x1b[2m", None),
        ];
        for (command, words, colour, mark) in said {
            if let Some(rest) = content.strip_prefix(command) {
                let line = [colour, words, rest, b"\x1b[0m"].concat();
                let at = self.lines;
                self.commit(&line, time);
                if let Some(kind) = mark
                    && self.lines > at
                {
                    self.marks.push(LogMark { line: at, kind });
                }
                return;
            }
        }
        self.commit(content, time);
    }

    /// One whole line, as a terminal shows it: its sections' markers noted, the text after its
    /// last `\r` kept (the rest's colours still taking effect), escapes made styles.
    fn commit(&mut self, content: &[u8], time: i64) {
        if self.truncated {
            return;
        }
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let line = self.lines;
        let marked = self.dialect == Dialect::GitLab && self.sections_in(content, (line, time));
        let (dropped, shown) = content.rfind_byte(b'\r').map_or((&[][..], content), |at| {
            (
                content.get(..at).unwrap_or_default(),
                content.get(at.saturating_add(1)..).unwrap_or_default(),
            )
        });
        self.style = styles(dropped, self.style, |_, _| {});
        let start = self.text.len();
        let mut style = self.style;
        let mut runs = Vec::new();
        let mut text = Vec::with_capacity(shown.len());
        style = styles(shown, style, |piece, now| {
            if piece.is_empty() {
                return;
            }
            let at = start.saturating_add(text.len());
            runs.push((at, now));
            text.extend_from_slice(piece);
        });
        // A line holding only markers (a section's end, the next's start) isn't one.
        if marked && text.iter().all(u8::is_ascii_whitespace) {
            self.style = style;
            return;
        }
        if start.saturating_add(text.len()) >= MOST_TEXT {
            self.truncated = true;
            return;
        }
        for (at, now) in runs {
            let current = self
                .runs
                .last()
                .map_or_else(Style::default, |run| run.style);
            if now != current {
                self.runs.push(Run {
                    at: u32::try_from(at).unwrap_or(u32::MAX),
                    style: now,
                });
            }
        }
        // A GitLab runner's own errors and warnings ("ERROR: Job failed: exit code 1").
        if self.dialect == Dialect::GitLab {
            let said = if text.starts_with(b"ERROR: ") {
                Some(MarkKind::Error)
            } else if text.starts_with(b"WARNING: ") {
                Some(MarkKind::Warning)
            } else {
                None
            };
            if let Some(kind) = said {
                self.marks.push(LogMark { line, kind });
            }
        }
        self.text.extend_from_slice(&text);
        // The line ends in the default style: a newline has none.
        if self
            .runs
            .last()
            .is_some_and(|run| run.style != Style::default())
        {
            self.runs.push(Run {
                at: u32::try_from(self.text.len()).unwrap_or(u32::MAX),
                style: Style::default(),
            });
        }
        self.text.push(b'\n');
        self.lines = self.lines.saturating_add(1);
        self.style = style;
        if time != 0 && !self.timed {
            self.timed = true;
            self.times = vec![0; usize::try_from(line).unwrap_or(0)];
        }
        if self.timed {
            self.times.push(time);
        }
    }

    /// Notes the sections `content` starts and ends (on line `line`, the one it would be,
    /// written at `time`); returns whether it held any marker.
    fn sections_in(&mut self, content: &[u8], (line, time): (u32, i64)) -> bool {
        let mut marked = false;
        let mut rest = content;
        while let Some(at) = rest.find(b"section_") {
            let after = rest.get(at..).unwrap_or_default();
            let (taken, said) = marker(after);
            rest = after.get(taken.max(1)..).unwrap_or_default();
            let Some(said) = said else {
                continue;
            };
            marked = true;
            let depth = u16::try_from(self.open.len()).unwrap_or(u16::MAX);
            match said {
                Marker::Start {
                    name,
                    at: started,
                    collapsed,
                } => {
                    self.open.push(self.sections.len());
                    self.sections.push(Section {
                        name,
                        line,
                        started: if started == 0 { time } else { started },
                        ended: None,
                        collapsed,
                        depth,
                        status: None,
                    });
                }
                Marker::End { name, at: ended } => {
                    // The innermost open one of that name, and those inside it.
                    if let Some(place) = self.open.iter().rposition(|open| {
                        self.sections
                            .get(*open)
                            .is_some_and(|section| section.name == name)
                    }) {
                        let last = line.saturating_sub(1);
                        for open in self.open.drain(place..) {
                            if let Some(section) = self.sections.get_mut(open) {
                                section.ended = Some((ended, last.max(section.line)));
                            }
                        }
                    }
                }
            }
        }
        marked
    }
}

/// A GitHub job's log with its steps made its sections, as GitHub shows a job.
///
/// Each step is a line of its own (its name) heading what it wrote, from the first line written
/// once it started (by the lines' times) to the next step's, its time and how it went its own;
/// the groups inside it a step in. A step that didn't fail is folded, as GitHub folds them. A
/// step that wrote nothing, or a log without times, adds nothing.
#[must_use]
pub fn with_steps(parts: LogParts, steps: &[JobStep]) -> LogParts {
    let headed = step_lines(&parts.times, steps);
    with_steps_at(parts, &headed)
}

/// [`with_steps`], each step's first line known (`headed`: in order, each line once), as where
/// the log is its steps' own logs one after another, rather than found by the lines' times.
#[must_use]
pub fn with_steps_at(parts: LogParts, headed: &[(u32, &JobStep)]) -> LogParts {
    let starts = line_starts(&parts.text);
    let lines = u32::try_from(starts.len()).unwrap_or(u32::MAX);
    if headed.is_empty() {
        return parts;
    }
    let (text, inserted) = with_names(&parts.text, &starts, headed);
    let mut times = Vec::with_capacity(parts.times.len().saturating_add(headed.len()));
    let mut names = headed.iter().peekable();
    for (line, time) in parts.times.iter().enumerate() {
        if let Some((_, step)) = names.next_if(|(at, _)| usize::try_from(*at).ok() == Some(line)) {
            times.push(step.started.unwrap_or(0));
        }
        times.push(*time);
    }
    // A line's place once the names are in: each inserted at it or before moves it down one.
    let moved = |line: u32| -> u32 {
        let before = headed.partition_point(|(at, _)| *at <= line);
        line.saturating_add(u32::try_from(before).unwrap_or(u32::MAX))
    };
    let runs = parts
        .runs
        .iter()
        .map(|run| {
            let shift = inserted
                .iter()
                .take_while(|(at, _)| *at <= run.at)
                .map(|(_, length)| *length)
                .fold(0_u32, u32::saturating_add);
            Run {
                at: run.at.saturating_add(shift),
                style: run.style,
            }
        })
        .collect();
    let total = lines.saturating_add(u32::try_from(headed.len()).unwrap_or(u32::MAX));
    // Each step's name's line: just before its first line, as it now is.
    let headers: Vec<u32> = headed
        .iter()
        .map(|(line, _)| moved(*line).saturating_sub(1))
        .collect();
    let mut sections: Vec<Section> = headed
        .iter()
        .zip(&headers)
        .enumerate()
        .map(|(at, ((_, step), header))| {
            let last = headers
                .get(at.saturating_add(1))
                .map_or(total, |next| *next)
                .saturating_sub(1)
                .max(*header);
            Section {
                name: step.name.clone(),
                line: *header,
                started: step.started.unwrap_or(0),
                ended: step.finished.map(|finished| (finished, last)),
                collapsed: step.status != CiStatus::Failed,
                depth: 0,
                status: Some(step.status),
            }
        })
        .collect();
    sections.extend(parts.sections.into_iter().map(|section| Section {
        line: moved(section.line),
        ended: section.ended.map(|(time, last)| (time, moved(last))),
        depth: section.depth.saturating_add(1),
        ..section
    }));
    sections.sort_by_key(|section| (section.line, section.depth));
    let marks = parts
        .marks
        .into_iter()
        .map(|mark| LogMark {
            line: moved(mark.line),
            ..mark
        })
        .collect();
    LogParts {
        text,
        runs,
        times,
        sections,
        marks,
        truncated: parts.truncated,
    }
}

/// `text` with each step's name (one of `headed`, at its first line) inserted before its
/// first line (`starts`: where each line starts); and where each went in `text` (a byte) and
/// how long it is, for what points into the text to move past it.
fn with_names(
    text: &[u8],
    starts: &[usize],
    headed: &[(u32, &JobStep)],
) -> (Vec<u8>, Vec<(u32, u32)>) {
    let mut named = Vec::with_capacity(text.len().saturating_add(headed.len().saturating_mul(32)));
    let mut inserted: Vec<(u32, u32)> = Vec::with_capacity(headed.len());
    let mut copied = 0_usize;
    for (line, step) in headed {
        let start = starts
            .get(usize::try_from(*line).unwrap_or(usize::MAX))
            .copied()
            .unwrap_or(text.len());
        named.extend_from_slice(text.get(copied..start).unwrap_or_default());
        let name: String = step.name.chars().filter(|c| *c != '\n').collect();
        named.extend_from_slice(name.as_bytes());
        named.push(b'\n');
        inserted.push((
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(name.len().saturating_add(1)).unwrap_or(u32::MAX),
        ));
        copied = start;
    }
    named.extend_from_slice(text.get(copied..).unwrap_or_default());
    (named, inserted)
}

/// Each step's first line (by `times`, each line's): the first written once it started, after
/// the step before's; two starting on one line, the later one has it.
fn step_lines<'a>(times: &[i64], steps: &'a [JobStep]) -> Vec<(u32, &'a JobStep)> {
    let mut headed: Vec<(u32, &JobStep)> = Vec::new();
    let mut from = 0_usize;
    let mut ordered: Vec<&JobStep> = steps.iter().collect();
    ordered.sort_by_key(|step| step.number);
    for step in ordered {
        let Some(started) = step.started else {
            continue;
        };
        let Some(first) = times
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, time)| **time != 0 && **time >= started)
            .map(|(line, _)| line)
        else {
            continue;
        };
        if headed
            .last()
            .is_some_and(|(line, _)| usize::try_from(*line).ok() == Some(first))
        {
            headed.pop();
        }
        headed.push((u32::try_from(first).unwrap_or(u32::MAX), step));
        from = first;
    }
    headed
}

/// Where each line of `text` starts.
fn line_starts(text: &[u8]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut at = 0_usize;
    while at < text.len() {
        starts.push(at);
        at = text
            .get(at..)
            .and_then(|rest| rest.find_byte(b'\n'))
            .map_or(text.len(), |end| at.saturating_add(end).saturating_add(1));
    }
    starts
}

/// A GitHub workflow command a log's line starts with, as egit says it: the command, the words
/// before what follows it, its colour, and what it marks the line as.
type Command = (
    &'static [u8],
    &'static [u8],
    &'static [u8],
    Option<MarkKind>,
);

/// A section's marker.
enum Marker {
    Start {
        name: String,
        at: i64,
        collapsed: bool,
    },
    End {
        name: String,
        at: i64,
    },
}

/// The marker at the start of `bytes` (`section_start:<time>:<name>[options]`, or
/// `section_end:<time>:<name>`), and how many bytes it takes; none where it isn't one.
fn marker(bytes: &[u8]) -> (usize, Option<Marker>) {
    let (start, rest) = if let Some(rest) = bytes.strip_prefix(b"section_start:") {
        (true, rest)
    } else if let Some(rest) = bytes.strip_prefix(b"section_end:") {
        (false, rest)
    } else {
        return (0, None);
    };
    let head = bytes.len().saturating_sub(rest.len());
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let at = rest
        .get(..digits)
        .and_then(|digits| digits.to_str().ok())
        .and_then(|digits| digits.parse::<i64>().ok());
    let Some(at) = at.filter(|_| rest.get(digits) == Some(&b':')) else {
        return (head, None);
    };
    let named = rest.get(digits.saturating_add(1)..).unwrap_or_default();
    let length = named
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(byte))
        .count();
    let Some(name) = named.get(..length).filter(|name| !name.is_empty()) else {
        return (head, None);
    };
    let name = String::from_utf8_lossy(name).into_owned();
    let mut taken = head
        .saturating_add(digits)
        .saturating_add(1)
        .saturating_add(length);
    let options = named.get(length..).unwrap_or_default();
    let mut collapsed = false;
    if options.first() == Some(&b'[')
        && let Some(end) = options.find_byte(b']')
    {
        collapsed = options
            .get(1..end)
            .is_some_and(|options| options.find(b"collapsed=true").is_some());
        taken = taken.saturating_add(end).saturating_add(1);
    }
    let marker = if start {
        Marker::Start {
            name,
            at,
            collapsed,
        }
    } else {
        Marker::End { name, at }
    };
    (taken, Some(marker))
}

/// A runner's timestamp at the start of `line` (`2024-05-14T11:19:20.000000Z 00O `): its time,
/// whether the line continues the one before (`+` in place of the space), and the rest.
fn timestamp(line: &[u8]) -> Option<(i64, bool, &[u8])> {
    // The date and time, its fraction (any digits), `Z`.
    let seconds = line.get(..19)?.to_str().ok()?;
    let time = parse_time(seconds)?;
    let after = line.get(19..)?;
    let fraction = after.strip_prefix(b".").map_or(0, |rest| {
        rest.iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
            .saturating_add(1)
    });
    let after = after.get(fraction..)?.strip_prefix(b"Z ")?;
    // The stream (two hex digits), its kind (out or error), and the line's kind.
    let (stream, after) = (after.get(..2)?, after.get(2..)?);
    if !stream.iter().all(u8::is_ascii_hexdigit) || !matches!(after.first(), Some(b'O' | b'E')) {
        return None;
    }
    let continued = match after.get(1) {
        Some(b'+') => true,
        Some(b' ') => false,
        _ => return None,
    };
    Some((time, continued, after.get(2..).unwrap_or_default()))
}

/// A GitHub Actions line's time (`2024-05-14T11:19:20.1234567Z `), and the rest.
fn github_timestamp(line: &[u8]) -> Option<(i64, &[u8])> {
    let time = parse_time(line.get(..19)?.to_str().ok()?)?;
    let after = line.get(19..)?;
    let fraction = after.strip_prefix(b".").map_or(0, |rest| {
        rest.iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
            .saturating_add(1)
    });
    let rest = after.get(fraction..)?.strip_prefix(b"Z")?;
    Some((time, rest.strip_prefix(b" ").unwrap_or(rest)))
}

/// Walks `bytes`' text and escapes from `style`: each piece of text with the style it is in
/// handed to `piece`; returns the style it ends in.
fn styles(bytes: &[u8], mut style: Style, mut piece: impl FnMut(&[u8], Style)) -> Style {
    let mut rest = bytes;
    while !rest.is_empty() {
        let Some(at) = rest.find_byte(0x1b) else {
            piece(rest, style);
            break;
        };
        piece(rest.get(..at).unwrap_or_default(), style);
        let escape = rest.get(at..).unwrap_or_default();
        let (taken, sgr) = escape_sequence(escape);
        if let Some(parameters) = sgr {
            style = apply_sgr(style, parameters);
        }
        rest = escape.get(taken.max(1)..).unwrap_or_default();
    }
    style
}

#[cfg(test)]
mod tests {
    use egit_git::Color;

    use super::*;

    fn parsed(chunks: &[&str]) -> LogParts {
        let mut log = JobLog::new();
        for chunk in chunks {
            log.feed(chunk.as_bytes());
        }
        log.parts()
    }

    fn text(parts: &LogParts) -> &str {
        std::str::from_utf8(&parts.text).unwrap()
    }

    #[test]
    fn colours_become_runs_and_progress_keeps_its_last_rewrite() {
        let parts = parsed(&["plain\n\x1b[31;1mred\x1b[0m then\n", "10%\r20%\r", "done\n"]);
        assert_eq!(text(&parts), "plain\nred then\ndone\n");
        assert_eq!(
            parts.runs,
            [
                Run {
                    at: 6,
                    style: Style {
                        color: Some(Color::Indexed(1)),
                        bold: true,
                        dim: false
                    }
                },
                Run {
                    at: 9,
                    style: Style::default()
                }
            ]
        );
        assert_eq!(parts.times, Vec::<i64>::new());
    }

    #[test]
    fn sections_are_noted_and_their_markers_left_out_whatever_the_chunks() {
        let log = "\x1b[0Ksection_start:1700000000:prepare[collapsed=true]\r\x1b[0K\x1b[36;1mPreparing\x1b[0;m\n\
                   Using docker\n\
                   \x1b[0Ksection_end:1700000005:prepare\r\x1b[0K\x1b[0Ksection_start:1700000005:step_script\r\x1b[0K\x1b[36;1mExecuting\x1b[0;m\n\
                   $ make\n\
                   ok\n\
                   \x1b[0Ksection_end:1700000065:step_script\r\x1b[0K\n";
        // Fed whole, and a byte at a time: alike.
        let whole = parsed(&[log]);
        let mut bytes = JobLog::new();
        for byte in log.as_bytes() {
            bytes.feed(std::slice::from_ref(byte));
        }
        let bytes = bytes.parts();
        assert_eq!(whole.text, bytes.text);
        assert_eq!(whole.sections, bytes.sections);
        assert_eq!(
            text(&whole),
            "Preparing\nUsing docker\nExecuting\n$ make\nok\n"
        );
        assert_eq!(
            whole
                .sections
                .iter()
                .map(|section| (
                    section.name.as_str(),
                    section.line,
                    section.ended,
                    section.collapsed
                ))
                .collect::<Vec<_>>(),
            [
                ("prepare", 0, Some((1_700_000_005, 1)), true),
                ("step_script", 2, Some((1_700_000_065, 4)), false)
            ]
        );
    }

    #[test]
    fn timestamps_are_taken_off_and_continued_lines_joined() {
        let parts = parsed(&[
            "2024-05-14T11:19:20.000000Z 00O first\n",
            "2024-05-14T11:19:21.123456Z 01E sec",
            "ond, \n2024-05-14T11:19:21.200000Z 01E+continued\n",
            "2024-05-14T11:19:22.000000Z 00O last",
        ]);
        assert_eq!(text(&parts), "first\nsecond, continued\nlast\n");
        assert_eq!(parts.times, [1_715_685_560, 1_715_685_561, 1_715_685_562]);
    }

    #[test]
    fn a_line_without_a_timestamp_is_as_written() {
        for line in [
            "2024-05-14 11:19:20 not a runner's",
            "2024-05-14T11:19:20.000000Z 0O short",
            "2024-05-14T11:19:20.000000Z 00X kind",
        ] {
            let parts = parsed(&[line, "\n"]);
            assert_eq!(text(&parts), format!("{line}\n"));
        }
    }

    #[test]
    fn a_github_log_s_groups_are_sections_and_its_commands_said() {
        let mut log = JobLog::github();
        log.feed(
            "\u{feff}2024-05-14T11:19:20.1234567Z ##[group]Run actions/checkout@v4\n\
             2024-05-14T11:19:21.0000000Z with: v4\n\
             2024-05-14T11:19:22.0000000Z ##[endgroup]\n\
             2024-05-14T11:19:23.0000000Z ##[command]/usr/bin/git version\n\
             2024-05-14T11:19:24.0000000Z ##[error]Process completed with exit code 1.\n\
             2024-05-14T11:19:25.0000000Z section_start:1:not GitLab's\n"
                .as_bytes(),
        );
        let parts = log.parts();
        assert_eq!(
            text(&parts),
            "Run actions/checkout@v4\nwith: v4\n/usr/bin/git version\n\
             Error: Process completed with exit code 1.\nsection_start:1:not GitLab's\n"
        );
        assert_eq!(
            parts
                .sections
                .iter()
                .map(|section| (section.name.as_str(), section.line, section.ended))
                .collect::<Vec<_>>(),
            [("Run actions/checkout@v4", 0, Some((1_715_685_562, 1)))]
        );
        assert_eq!(parts.times.first(), Some(&1_715_685_560));
        // The error's line in red.
        let red = parts
            .runs
            .iter()
            .find(|run| run.style.color == Some(Color::Indexed(1)))
            .unwrap();
        assert_eq!(red.at, 54);
        assert_eq!(
            parts.marks,
            [LogMark {
                line: 3,
                kind: MarkKind::Error
            }]
        );
    }

    #[test]
    fn a_github_job_s_steps_head_what_they_wrote() {
        let mut log = JobLog::github();
        log.feed(
            "2024-05-14T11:19:20.0000000Z Current runner version: '2.316.1'\n\
             2024-05-14T11:19:22.0000000Z ##[group]Run make test\n\
             2024-05-14T11:19:22.0000000Z make test\n\
             2024-05-14T11:19:22.0000000Z ##[endgroup]\n\
             2024-05-14T11:19:30.0000000Z FAIL parse\n\
             2024-05-14T11:19:31.0000000Z ##[error]Process completed with exit code 2.\n"
                .as_bytes(),
        );
        let step = |number, name: &str, status, (started, finished)| JobStep {
            number,
            name: name.to_owned(),
            status,
            started: Some(started),
            finished: Some(finished),
        };
        let steps = [
            step(
                2,
                "Run make test",
                CiStatus::Failed,
                (1_715_685_562, 1_715_685_571),
            ),
            step(
                1,
                "Set up job",
                CiStatus::Success,
                (1_715_685_559, 1_715_685_561),
            ),
            // It wrote nothing: no section.
            step(
                3,
                "Post job cleanup",
                CiStatus::Skipped,
                (1_715_685_600, 1_715_685_600),
            ),
        ];
        let parts = with_steps(log.parts(), &steps);
        assert_eq!(
            text(&parts),
            "Set up job\nCurrent runner version: '2.316.1'\nRun make test\nRun make test\n\
             make test\nFAIL parse\nError: Process completed with exit code 2.\n"
        );
        assert_eq!(
            parts
                .sections
                .iter()
                .map(|section| (
                    section.name.as_str(),
                    section.line,
                    section.ended.map(|(_, last)| last),
                    section.depth,
                    section.collapsed,
                    section.status
                ))
                .collect::<Vec<_>>(),
            [
                ("Set up job", 0, Some(1), 0, true, Some(CiStatus::Success)),
                (
                    "Run make test",
                    2,
                    Some(6),
                    0,
                    false,
                    Some(CiStatus::Failed)
                ),
                ("Run make test", 3, Some(4), 1, true, None),
            ]
        );
        // Each line keeps its time, a step's name its start; the error its mark.
        assert_eq!(parts.times.len(), 7);
        assert_eq!(parts.times.get(2), Some(&1_715_685_562));
        assert_eq!(parts.times.get(5), Some(&1_715_685_570));
        assert_eq!(
            parts.marks,
            [LogMark {
                line: 6,
                kind: MarkKind::Error
            }]
        );
        // The error's colour moved with its line.
        let red = parts
            .runs
            .iter()
            .find(|run| run.style.color == Some(Color::Indexed(1)))
            .unwrap();
        assert_eq!(
            parts
                .text
                .get(usize::try_from(red.at).unwrap()..)
                .unwrap()
                .get(..6),
            Some(&b"Error:"[..])
        );
    }

    #[test]
    fn steps_read_one_by_one_head_their_own_lines() {
        let step = |number, name: &str, status| JobStep {
            number,
            name: name.to_owned(),
            status,
            started: Some(1_715_685_560),
            finished: None,
        };
        let steps = [
            step(1, "Set up job", CiStatus::Success),
            step(2, "Run make", CiStatus::Running),
        ];
        // Each step's own log one after another, as read while the job runs: the first left
        // without its last line ended, the second (under way) inside a group not yet ended.
        let mut log = JobLog::github();
        let mut headed = Vec::new();
        for (place, part) in [
            "2024-05-14T11:19:20.0000000Z Runner 2.316.1",
            "2024-05-14T11:19:21.0000000Z ##[group]Run make\n\
             2024-05-14T11:19:21.0000000Z make all\n",
        ]
        .into_iter()
        .enumerate()
        {
            headed.push((log.lines(), &steps[place]));
            log.feed(part.as_bytes());
            log.end_part();
        }
        assert_eq!(log.lines(), 3);
        let parts = with_steps_at(log.parts(), &headed);
        assert_eq!(
            text(&parts),
            "Set up job\nRunner 2.316.1\nRun make\nRun make\nmake all\n"
        );
        assert_eq!(
            parts
                .sections
                .iter()
                .map(|section| (
                    section.name.as_str(),
                    section.line,
                    section.ended.map(|(_, last)| last),
                    section.depth
                ))
                .collect::<Vec<_>>(),
            [
                ("Set up job", 0, None, 0),
                ("Run make", 2, None, 0),
                // The group the running step left open, ended where its part ends.
                ("Run make", 3, Some(4), 1),
            ]
        );
    }

    #[test]
    fn a_gitlab_runner_s_errors_are_marked() {
        let mut log = JobLog::new();
        log.feed(b"$ make\n\x1b[0;31mERROR: Job failed: exit code 1\x1b[0;m\n");
        assert_eq!(
            log.parts().marks,
            [LogMark {
                line: 1,
                kind: MarkKind::Error
            }]
        );
    }

    #[test]
    fn the_line_under_way_shows_and_is_taken_back_up() {
        let mut log = JobLog::new();
        log.feed(b"one\ntw");
        assert_eq!(text(&log.parts()), "one\ntw\n");
        log.feed(b"o\n");
        assert_eq!(text(&log.parts()), "one\ntwo\n");
        assert_eq!(log.len(), 8);
    }
}
