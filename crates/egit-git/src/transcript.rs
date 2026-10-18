//! What the commands that change a repository print as they run (hooks above all: a
//! `pre-commit` check's output, a `pre-push` hook's), kept as lines for a terminal-like view.
//!
//! A [`Git`](crate::Git) given a transcript (`Git::with_transcript`) writes into it each command
//! that may write: its command line, then its standard output and error as they come, then how it
//! ended. A `\r` rewrites the line under way, as a terminal does (progress counters); colours
//! (SGR escapes, which tools print when asked: `PRE_COMMIT_COLOR=always`) become styled spans,
//! other escapes are dropped. At most [`MAX_LINES`] are kept, the oldest dropped first, each at
//! most [`MAX_LINE`] bytes.

use std::collections::VecDeque;
use std::sync::Arc;

use bstr::ByteSlice as _;
use parking_lot::Mutex;

/// The most lines kept.
pub const MAX_LINES: usize = 20_000;

/// The longest line kept, in bytes (the rest is cut, with an ellipsis).
pub const MAX_LINE: usize = 16 * 1024;

/// Where a line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// A command started (`git commit --file=-`).
    Command,
    /// Its standard output.
    Out,
    /// Its standard error: git's messages and progress, and every hook's output (git sends
    /// hooks' standard output there too).
    Err,
    /// How it ended (`exit status 1`, `cancelled`).
    Ended {
        /// Whether it succeeded.
        success: bool,
    },
}

/// A colour a tool asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// One of the terminal's 256 (0–7 the usual, 8–15 their bright ones).
    Indexed(u8),
    /// An exact one.
    Rgb(u8, u8, u8),
}

/// How a span of text is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// Its colour, if not the default.
    pub color: Option<Color>,
    /// Bold.
    pub bold: bool,
    /// Faint.
    pub dim: bool,
}

/// Some of a line's text, styled: byte offsets into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Where it starts.
    pub start: u32,
    /// Where it ends.
    pub end: u32,
    /// How it is drawn.
    pub style: Style,
}

/// One line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Where it came from.
    pub kind: LineKind,
    /// Its text, escapes taken out.
    pub text: String,
    /// Its styled parts (none for plain text).
    pub spans: Vec<Span>,
}

impl Line {
    const fn plain(kind: LineKind, text: String) -> Self {
        Self {
            kind,
            text,
            spans: Vec::new(),
        }
    }
}

/// The output stream a chunk came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Out,
    Err,
}

/// One stream's line under way: its bytes so far and the style it starts in.
#[derive(Debug, Default)]
struct Pending {
    bytes: Vec<u8>,
    /// The style the stream is in at the line's start (a colour can last across lines).
    style: Style,
}

/// The lines kept.
#[derive(Debug, Default)]
pub struct Log {
    lines: VecDeque<Line>,
    /// How many lines were dropped from the front.
    dropped: u64,
    pending: [Pending; 2],
    /// Each stream's line under way, as it shows now.
    live: [Option<Line>; 2],
    updates: u64,
}

impl Log {
    /// How many lines there are, those under way included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines
            .len()
            .saturating_add(self.live.iter().flatten().count())
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Line `at` (from the oldest kept), those under way last.
    #[must_use]
    pub fn line(&self, at: usize) -> Option<&Line> {
        self.lines.get(at).or_else(|| {
            self.live
                .iter()
                .flatten()
                .nth(at.saturating_sub(self.lines.len()))
        })
    }

    /// How many lines were dropped from the front, past [`MAX_LINES`].
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Counts every change (to notice one).
    #[must_use]
    pub const fn updates(&self) -> u64 {
        self.updates
    }

    /// Every line's text, a line each (to copy).
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::new();
        for at in 0..self.len() {
            if let Some(line) = self.line(at) {
                text.push_str(&line.text);
                text.push('\n');
            }
        }
        text
    }

    fn push(&mut self, line: Line) {
        if self.lines.len() >= MAX_LINES {
            self.lines.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.lines.push_back(line);
        self.updates = self.updates.wrapping_add(1);
    }

    const fn index(stream: Stream) -> usize {
        match stream {
            Stream::Out => 0,
            Stream::Err => 1,
        }
    }

    fn feed(&mut self, stream: Stream, chunk: &[u8]) {
        let at = Self::index(stream);
        let kind = match stream {
            Stream::Out => LineKind::Out,
            Stream::Err => LineKind::Err,
        };
        let Some(pending) = self.pending.get_mut(at) else {
            return;
        };
        pending.bytes.extend_from_slice(chunk);
        let mut ended = Vec::new();
        while let Some(end) = pending.bytes.find_byte(b'\n') {
            let mut whole: Vec<u8> = pending.bytes.drain(..=end).collect();
            whole.pop();
            let (line, style) = parse(kind, last_rewrite(&whole), pending.style);
            pending.style = style;
            ended.push(line);
        }
        // What is under way: its last rewrite, once there is one.
        let live = (!pending.bytes.is_empty()).then(|| {
            let shown = last_rewrite(&pending.bytes);
            parse(kind, shown, pending.style).0
        });
        // A long line under way without an end is kept as it goes.
        if pending.bytes.len() > MAX_LINE.saturating_mul(4) {
            pending.bytes = last_rewrite(&pending.bytes).to_vec();
        }
        for line in ended {
            self.push(line);
        }
        if let Some(slot) = self.live.get_mut(at) {
            *slot = live.filter(|line| !line.text.is_empty());
        }
        self.updates = self.updates.wrapping_add(1);
    }

    fn finish(&mut self, stream: Stream) {
        let at = Self::index(stream);
        if let Some(slot) = self.live.get_mut(at) {
            *slot = None;
        }
        let Some(pending) = self.pending.get_mut(at) else {
            return;
        };
        let rest = std::mem::take(&mut pending.bytes);
        let style = std::mem::take(&mut pending.style);
        let kind = match stream {
            Stream::Out => LineKind::Out,
            Stream::Err => LineKind::Err,
        };
        let shown = last_rewrite(&rest);
        if !shown.is_empty() {
            let (line, _) = parse(kind, shown, style);
            self.push(line);
        }
    }
}

/// A line's text after its last `\r` (a terminal rewrites the line from there), a `\r` ending
/// it ignored.
fn last_rewrite(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    bytes
        .rfind_byte(b'\r')
        .and_then(|at| bytes.get(at.saturating_add(1)..))
        .unwrap_or(bytes)
}

/// A line's bytes parsed: escapes taken out, colours turned into spans, starting in `style`;
/// returns the line and the style it ends in.
fn parse(kind: LineKind, bytes: &[u8], mut style: Style) -> (Line, Style) {
    let mut text = String::with_capacity(bytes.len().min(MAX_LINE));
    let mut spans: Vec<Span> = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let (plain, escape) = rest.find_byte(0x1b).map_or((rest, None), |at| {
            (rest.get(..at).unwrap_or_default(), rest.get(at..))
        });
        if !plain.is_empty() && text.len() < MAX_LINE {
            let start = text.len();
            text.push_str(&plain.to_str_lossy());
            if style != Style::default() {
                let offset = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);
                spans.push(Span {
                    start: offset(start),
                    end: offset(text.len()),
                    style,
                });
            }
        }
        let Some(escape) = escape else {
            break;
        };
        let (taken, said) = escape_sequence(escape);
        if let Some(parameters) = said {
            style = apply_sgr(style, parameters);
        }
        rest = escape.get(taken..).unwrap_or_default();
    }
    if text.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !text.is_char_boundary(cut) {
            cut = cut.saturating_sub(1);
        }
        text.truncate(cut);
        text.push('…');
    }
    (Line { kind, text, spans }, style)
}

/// An escape sequence at the start of `bytes`: how many bytes it takes, and an SGR's
/// parameters (`ESC [ … m`). Other CSI and OSC sequences are skipped whole; a lone `ESC`, one
/// byte.
fn escape_sequence(bytes: &[u8]) -> (usize, Option<&[u8]>) {
    match bytes.get(1) {
        Some(b'[') => {
            // Parameters and intermediates, then a final byte in `@`…`~`.
            let body = bytes.get(2..).unwrap_or_default();
            body.iter()
                .position(|byte| (0x40..=0x7e).contains(byte))
                .map_or((bytes.len(), None), |end| {
                    let sgr = (body.get(end) == Some(&b'm')).then(|| body.get(..end));
                    (end.saturating_add(3), sgr.flatten())
                })
        }
        Some(b']') => {
            // Up to a BEL, or `ESC \`.
            let body = bytes.get(2..).unwrap_or_default();
            let end = body
                .iter()
                .position(|byte| *byte == 0x07)
                .map(|at| at.saturating_add(3))
                .or_else(|| body.find(b"\x1b\\").map(|at| at.saturating_add(4)))
                .unwrap_or(bytes.len());
            (end, None)
        }
        Some(_) => (2, None),
        None => (1, None),
    }
}

/// `style` after an SGR with `parameters` (`1;31`, `38;5;208`, `38;2;255;128;0`, `0`).
fn apply_sgr(mut style: Style, parameters: &[u8]) -> Style {
    let mut numbers = parameters
        .split(|byte| *byte == b';' || *byte == b':')
        .map(|number| {
            number
                .to_str()
                .ok()
                .and_then(|number| number.parse::<u16>().ok())
                .unwrap_or(0)
        });
    let byte = |number: Option<u16>| u8::try_from(number.unwrap_or(0)).unwrap_or(u8::MAX);
    let mut any = false;
    while let Some(number) = numbers.next() {
        any = true;
        style = match number {
            0 => Style::default(),
            1 => Style {
                bold: true,
                ..style
            },
            2 => Style { dim: true, ..style },
            22 => Style {
                bold: false,
                dim: false,
                ..style
            },
            30..=37 => Style {
                color: Some(Color::Indexed(byte(number.checked_sub(30)))),
                ..style
            },
            90..=97 => Style {
                color: Some(Color::Indexed(byte(number.checked_sub(82)))),
                ..style
            },
            39 => Style {
                color: None,
                ..style
            },
            38 => match numbers.next() {
                Some(5) => Style {
                    color: Some(Color::Indexed(byte(numbers.next()))),
                    ..style
                },
                Some(2) => {
                    let (red, green, blue) = (
                        byte(numbers.next()),
                        byte(numbers.next()),
                        byte(numbers.next()),
                    );
                    Style {
                        color: Some(Color::Rgb(red, green, blue)),
                        ..style
                    }
                }
                _ => style,
            },
            // A background colour's own parameters, skipped.
            48 => {
                match numbers.next() {
                    Some(5) => {
                        numbers.next();
                    }
                    Some(2) => {
                        numbers.next();
                        numbers.next();
                        numbers.next();
                    }
                    _ => {}
                }
                style
            }
            _ => style,
        };
    }
    // `ESC [ m` alone resets.
    if any { style } else { Style::default() }
}

/// What commands printed, shared between the threads reading them and whoever shows it.
#[derive(Clone, Debug, Default)]
pub struct Transcript(Arc<Mutex<Log>>);

impl Transcript {
    /// A transcript with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes a command starting, as a terminal shows it (`git commit --file=-`).
    pub fn command(&self, line: String) {
        self.0.lock().push(Line::plain(LineKind::Command, line));
    }

    /// Notes how a command ended.
    pub fn ended(&self, success: bool, said: String) {
        self.0
            .lock()
            .push(Line::plain(LineKind::Ended { success }, said));
    }

    /// Takes in a chunk of what a command printed.
    pub(crate) fn feed(&self, stream: Stream, chunk: &[u8]) {
        self.0.lock().feed(stream, chunk);
    }

    /// Ends a stream: its line under way, if any, is kept as a line.
    pub(crate) fn finish(&self, stream: Stream) {
        self.0.lock().finish(stream);
    }

    /// Reads the lines (held while `read` runs: keep it short, only the lines shown).
    pub fn read<R>(&self, read: impl FnOnce(&Log) -> R) -> R {
        read(&self.0.lock())
    }

    /// Counts every change (to notice one).
    #[must_use]
    pub fn updates(&self) -> u64 {
        self.0.lock().updates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(transcript: &Transcript) -> Vec<String> {
        transcript.read(|log| {
            (0..log.len())
                .filter_map(|at| log.line(at))
                .map(|line| line.text.clone())
                .collect()
        })
    }

    #[test]
    fn lines_come_as_chunks_split_them_and_rewrites_keep_the_last() {
        let transcript = Transcript::new();
        transcript.command("git commit --file=-".to_owned());
        transcript.feed(Stream::Err, b"check one...");
        assert_eq!(lines(&transcript), ["git commit --file=-", "check one..."]);
        transcript.feed(Stream::Err, b"ok\nprogress 10%\rprogress 50%\r");
        assert_eq!(
            lines(&transcript),
            ["git commit --file=-", "check one...ok", "progress 50%"],
            "the line under way shows its last rewrite"
        );
        transcript.feed(Stream::Err, b"progress 100%\r\n[main abc] Done\n");
        transcript.finish(Stream::Err);
        transcript.ended(true, "done".to_owned());
        assert_eq!(
            lines(&transcript),
            [
                "git commit --file=-",
                "check one...ok",
                "progress 100%",
                "[main abc] Done",
                "done"
            ]
        );
    }

    #[test]
    fn colours_become_spans_and_other_escapes_go() {
        let transcript = Transcript::new();
        transcript.feed(
            Stream::Err,
            b"\x1b[1;31mFailed\x1b[0m - hook id: \x1b]8;;https://x\x07lint\x1b]8;;\x07 \x1b[2K\x1b[38;5;208mok\n",
        );
        transcript.read(|log| {
            let line = log.line(0).unwrap();
            assert_eq!(line.text, "Failed - hook id: lint ok");
            assert_eq!(
                line.spans,
                [
                    Span {
                        start: 0,
                        end: 6,
                        style: Style {
                            color: Some(Color::Indexed(1)),
                            bold: true,
                            dim: false,
                        },
                    },
                    Span {
                        start: 23,
                        end: 25,
                        style: Style {
                            color: Some(Color::Indexed(208)),
                            ..Style::default()
                        },
                    },
                ]
            );
        });
        // A colour lasts across lines until reset; bright colours are 8–15.
        transcript.feed(Stream::Out, b"\x1b[92mgreen\nstill\x1b[m plain\n");
        transcript.read(|log| {
            assert_eq!(
                log.line(1).unwrap().spans[0].style.color,
                Some(Color::Indexed(10))
            );
            assert_eq!(log.line(2).unwrap().spans.len(), 1);
            assert_eq!(log.line(2).unwrap().text, "still plain");
        });
    }

    #[test]
    fn only_so_many_lines_are_kept() {
        let transcript = Transcript::new();
        let many = "x\n".repeat(MAX_LINES + 5);
        transcript.feed(Stream::Out, many.as_bytes());
        transcript.read(|log| {
            assert_eq!(log.len(), MAX_LINES);
            assert_eq!(log.dropped(), 5);
        });
        let long = "y".repeat(MAX_LINE * 2);
        transcript.feed(Stream::Out, format!("{long}\n").as_bytes());
        transcript.read(|log| {
            let last = log.line(log.len() - 1).unwrap();
            assert!(last.text.ends_with('…') && last.text.len() <= MAX_LINE + 3);
        });
    }
}
