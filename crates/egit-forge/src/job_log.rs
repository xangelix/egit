//! A GitLab job's log: how the job stands (`GET /projects/:path/jobs/:id`), its log read a part
//! at a time as it grows (`…/jobs/:id/trace`), and parsed as GitLab's own page shows it.
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
//! `section_end:<time>:<name>\r\e[0K`) noted, their markers out of the text. At most
//! [`MOST_TEXT`] bytes of text are kept; the rest is said to be on GitLab.

use bstr::ByteSlice as _;
use egit_git::{CancelToken, Style, apply_sgr, escape_sequence};
use serde::Deserialize;

use crate::account::Account;
use crate::answer::{parse_time, read_answer};
use crate::error::ForgeError;
use crate::gitlab::encoded;
use crate::pipelines::{CiStatus, has_pipelines};
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

/// How job `id` of `repository` stands.
///
/// # Errors
///
/// Any of [`ForgeError`]'s; [`ForgeError::NotFound`] on a forge without pipelines.
pub fn job(
    account: &Account,
    repository: &ForgeRepository,
    id: u64,
    cancel: &CancelToken,
) -> Result<JobRun, ForgeError> {
    let what = format!("{} job {id}", repository.full_name());
    if !has_pipelines(repository.host().kind()) {
        return Err(ForgeError::NotFound {
            host: account.host().name().to_owned(),
            what,
        });
    }
    let url = account.host().rest_url(&format!(
        "/projects/{}/jobs/{id}",
        encoded(repository.path())
    ));
    let answer = account.send(&url, None, cancel)?;
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
/// come: asked as a range where `ranges` (the host may answer them), else the whole skipped
/// as far as `from`.
///
/// # Errors
///
/// Any of [`ForgeError`]'s.
pub fn read_log(
    account: &Account,
    repository: &ForgeRepository,
    (id, from, ranges): (u64, u64, bool),
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&[u8]),
) -> Result<LogRead, ForgeError> {
    let url = account.host().rest_url(&format!(
        "/projects/{}/jobs/{id}/trace",
        encoded(repository.path())
    ));
    let asked = (ranges && from > 0).then_some(from);
    // How many of the log's bytes went by (a whole log's), how many were handed over, and
    // whether a range came from elsewhere than asked.
    let (mut seen, mut given, mut misplaced) = (0_u64, 0_u64, false);
    let streamed = account.stream(
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
    )?;
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

/// A style taking over from a byte of the text on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Where, in the text.
    pub at: u32,
    /// The style.
    pub style: Style,
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
    /// Whether GitLab shows it folded (`[collapsed=true]`).
    pub collapsed: bool,
    /// How many sections it is inside.
    pub depth: u16,
}

/// A log parsed as it comes: its text (lines ending in `\n`), styles, each line's time where
/// the runner wrote one, and sections.
#[derive(Clone, Debug, Default)]
pub struct JobLog {
    text: Vec<u8>,
    runs: Vec<Run>,
    /// Each line's time (seconds since the Unix epoch; 0 for none), once any line had one.
    times: Vec<i64>,
    /// Whether any line had one.
    timed: bool,
    sections: Vec<Section>,
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
    /// Whether the text reached [`MOST_TEXT`] (the rest was left out).
    pub truncated: bool,
}

impl JobLog {
    /// None yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// One whole line, as a terminal shows it: its sections' markers noted, the text after its
    /// last `\r` kept (the rest's colours still taking effect), escapes made styles.
    fn commit(&mut self, content: &[u8], time: i64) {
        if self.truncated {
            return;
        }
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let line = self.lines;
        let marked = self.sections_in(content, (line, time));
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
    fn the_line_under_way_shows_and_is_taken_back_up() {
        let mut log = JobLog::new();
        log.feed(b"one\ntw");
        assert_eq!(text(&log.parts()), "one\ntw\n");
        log.feed(b"o\n");
        assert_eq!(text(&log.parts()), "one\ntwo\n");
        assert_eq!(log.len(), 8);
    }
}
