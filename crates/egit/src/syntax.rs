//! Syntax highlighting: Sublime grammars (syntect, through fancy-regex) for the language a
//! file's name or first line names.
//!
//! A text is parsed once in the background, its parser's state kept every 128 lines. A line on
//! screen is coloured from the nearest state before it (or from the line before, scrolling on),
//! a few milliseconds a frame at most: a line not reached stays uncoloured for a frame or two.
//! The colours of the lines drawn lately are kept. States hold scopes, not colours, so the
//! light and dark themes colour the same parse. A line longer than [`MAX_LINE`] (minified code,
//! data) is skipped: one such line could take the grammars seconds.
//!
//! A diff is two files at once: its lines feed an old and a new parser (an unchanged line both,
//! coloured as the new file's), and a hunk's header starts both over, since the lines between
//! hunks are unknown. With the whole file as context there is one hunk, and the colours are
//! exact.
//!
//! In a browser the interface's thread colours only lines the background parse has been
//! through: syntect compiles each of a grammar's regexes the first time one is used, and a
//! thread meeting one another thread is compiling waits for it, which the page's main thread
//! may never do. Parsing from a kept state is the same work whichever thread does it, so every
//! regex those lines take is compiled by then.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use web_time::Instant;

use egit_git::sync::Mutex;
use egit_text::Text;
use egui::Color32;
use syntect::highlighting::{HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::grid::Span;
use crate::jobs;

/// Syntect's grammars, loaded the first time one is needed.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Syntect's themes, loaded the first time one is needed.
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// The dark and the light theme's highlighters (each sorts its theme's selectors), made once.
static HIGHLIGHTERS: LazyLock<[Highlighter<'static>; 2]> = LazyLock::new(|| {
    [
        Highlighter::new(theme(true)),
        Highlighter::new(theme(false)),
    ]
});

/// Lines between kept states.
const CHECKPOINT: u32 = 128;

/// Loads the grammars, themes, and highlighters now. In a browser: on the page's main thread
/// before its workers start, as both read them, and the main thread may never wait for a
/// worker making them.
#[cfg(target_arch = "wasm32")]
pub(crate) fn prepare() {
    LazyLock::force(&SYNTAXES);
    LazyLock::force(&HIGHLIGHTERS);
    let _ = default_theme();
}

/// The most time colouring takes a frame.
const FRAME_BUDGET: Duration = Duration::from_millis(3);

/// The most lines whose colours are kept.
const KEPT_LINES: usize = 4096;

/// The longest line parsed, in bytes; a longer one is left uncoloured, and the parsers go on
/// past it as if it weren't there.
const MAX_LINE: usize = 4096;

/// What a line feeds the parsers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feed {
    /// The new file's (or a file's only) parser: an added line, or any line of a file.
    New,
    /// The old file's parser: a removed line.
    Old,
    /// Both: an unchanged line, coloured as the new file's.
    Both,
    /// Neither, and both start over: a hunk's header.
    Reset,
    /// Neither: a filler line.
    Skip,
}

/// What each line of a text feeds.
#[derive(Clone, Debug)]
pub(crate) enum Feeds {
    /// Every line the same.
    All(Feed),
    /// Each line its own.
    Each(Arc<[Feed]>),
}

impl Feeds {
    /// What line `line`, `len` bytes long, feeds: nothing when it is longer than [`MAX_LINE`]
    /// (but a hunk's header still starts the parsers over).
    fn of(&self, line: u32, len: usize) -> Feed {
        let feed = match self {
            Self::All(feed) => *feed,
            Self::Each(feeds) => usize::try_from(line)
                .ok()
                .and_then(|line| feeds.get(line))
                .copied()
                .unwrap_or(Feed::Skip),
        };
        if len > MAX_LINE && feed != Feed::Reset {
            Feed::Skip
        } else {
            feed
        }
    }
}

/// One parser's state before a line.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stream {
    parse: ParseState,
    scopes: ScopeStack,
}

impl Stream {
    fn new(syntax: &SyntaxReference) -> Self {
        Self {
            parse: ParseState::new(syntax),
            scopes: ScopeStack::new(),
        }
    }

    /// Parses `line` (ended by `\n`); returns its operations, its scopes before them, and
    /// moves past it. A line the grammar fails on resets the stream.
    fn feed(
        &mut self,
        syntax: &SyntaxReference,
        line: &str,
    ) -> Option<(Vec<(usize, syntect::parsing::ScopeStackOp)>, ScopeStack)> {
        let before = self.scopes.clone();
        let Ok(ops) = self.parse.parse_line(line, &SYNTAXES) else {
            *self = Self::new(syntax);
            return None;
        };
        for (_, op) in &ops {
            if self.scopes.apply(op).is_err() {
                *self = Self::new(syntax);
                return None;
            }
        }
        Some((ops, before))
    }
}

/// Both parsers' states before a line.
#[derive(Clone, Debug, PartialEq, Eq)]
struct States {
    old: Stream,
    new: Stream,
}

impl States {
    fn new(syntax: &SyntaxReference) -> Self {
        Self {
            old: Stream::new(syntax),
            new: Stream::new(syntax),
        }
    }

    /// Feeds `line` as `feed` says; returns what colours it, if anything does.
    fn feed(
        &mut self,
        syntax: &SyntaxReference,
        feed: Feed,
        line: &str,
    ) -> Option<(Vec<(usize, syntect::parsing::ScopeStackOp)>, ScopeStack)> {
        match feed {
            Feed::New => self.new.feed(syntax, line),
            Feed::Old => self.old.feed(syntax, line),
            Feed::Both => {
                let _ = self.old.feed(syntax, line);
                self.new.feed(syntax, line)
            }
            Feed::Reset => {
                *self = Self::new(syntax);
                None
            }
            Feed::Skip => None,
        }
    }
}

/// What the background parse shares with the view.
#[derive(Debug, Default)]
struct Walk {
    /// The states before every [`CHECKPOINT`]th line, so far; one equal to the one before
    /// is shared with it (most of a file's lines start at its top level).
    checkpoints: Mutex<Vec<Arc<States>>>,
    /// Asks the parse to stop.
    stop: AtomicBool,
    /// How many lines the parse has been through.
    parsed: AtomicU32,
    /// Whether the parse reached the text's end.
    done: AtomicBool,
}

/// The colours of the lines drawn lately, and where colouring got to.
#[derive(Debug, Default)]
struct Kept {
    /// Which theme `lines` are coloured in (dark or light).
    dark: Option<bool>,
    lines: HashMap<u32, Vec<Span>>,
    /// The states before the line after the last one coloured.
    frontier: Option<(u32, States)>,
    /// The time spent colouring this frame.
    spent: Duration,
    /// Whether a line drawn this frame wasn't coloured.
    missed: bool,
}

/// A text's highlighting.
#[derive(Debug)]
pub(crate) struct Syntax {
    grammar: &'static SyntaxReference,
    feeds: Feeds,
    walk: Arc<Walk>,
    kept: Mutex<Kept>,
}

impl Drop for Syntax {
    fn drop(&mut self) {
        self.walk.stop.store(true, Ordering::Relaxed);
    }
}

impl Syntax {
    /// Highlighting for `text` (whose lines feed as `feeds` says) in the language `path` or
    /// the text's first line names; `None` for plain text. Starts parsing it in the background.
    pub(crate) fn start(path: &[u8], text: &Text, feeds: Feeds) -> Option<Self> {
        let first = text.line(0).unwrap_or_default();
        let grammar = language(path, first)?;
        // Made here, on a job's thread, so that colouring on the interface's never waits.
        LazyLock::force(&HIGHLIGHTERS);
        let walk = Arc::new(Walk::default());
        let bytes = text.shared();
        let lines = text.lines();
        let (shared, walk_feeds) = (Arc::clone(&walk), feeds.clone());
        jobs::detached(async move { parse(grammar, &bytes, lines, &walk_feeds, &shared).await });
        Some(Self {
            grammar,
            feeds,
            walk,
            kept: Mutex::new(Kept::default()),
        })
    }

    /// Starts a frame's colouring over.
    pub(crate) fn begin_frame(&self) {
        let mut kept = self.kept.lock();
        kept.spent = Duration::ZERO;
        kept.missed = false;
    }

    /// Whether a line drawn this frame wasn't coloured yet.
    pub(crate) fn needs_repaint(&self) -> bool {
        self.kept.lock().missed
    }

    /// Whether the background parse has been through the whole text.
    #[cfg(test)]
    pub(crate) fn is_parsed(&self) -> bool {
        self.walk.done.load(Ordering::Relaxed)
    }

    /// The colours of line `line` of `text`, in the dark or the light theme (`default` where
    /// the theme has its own default), if they are ready or can be worked out within the
    /// frame's budget.
    pub(crate) fn colors(
        &self,
        text: &Text,
        line: u32,
        dark: bool,
        default: Color32,
        out: &mut Vec<Span>,
    ) {
        let mut kept = self.kept.lock();
        if kept.dark != Some(dark) {
            kept.lines.clear();
            kept.dark = Some(dark);
        }
        if let Some(spans) = kept.lines.get(&line) {
            out.extend_from_slice(spans);
            return;
        }
        if kept.spent >= FRAME_BUDGET || !self.may_colour(line) {
            kept.missed = true;
            return;
        }
        let started = Instant::now();
        let from = match kept.frontier.as_ref() {
            Some((at, states)) if *at <= line && line.saturating_sub(*at) < CHECKPOINT => {
                Some((*at, states.clone()))
            }
            _ => {
                let index = usize::try_from(line.checked_div(CHECKPOINT).unwrap_or(0)).ok();
                let checkpoints = self.walk.checkpoints.lock();
                index
                    .and_then(|index| checkpoints.get(index))
                    .map(|states| {
                        (
                            line.saturating_sub(line.checked_rem(CHECKPOINT).unwrap_or(0)),
                            States::clone(states),
                        )
                    })
            }
        };
        let Some((mut at, mut states)) = from else {
            kept.missed = true;
            return;
        };
        let theme = theme(dark);
        let [dark_highlighter, light_highlighter] = &*HIGHLIGHTERS;
        let highlighter = if dark {
            dark_highlighter
        } else {
            light_highlighter
        };
        let mut buffer = String::new();
        while at <= line {
            let bytes = text.line(at).unwrap_or_default();
            let feed = self.feeds.of(at, bytes.len());
            let mut spans = Vec::new();
            if feed != Feed::Skip {
                let lossy = String::from_utf8_lossy(bytes);
                // A line that isn't UTF-8 is parsed with its bytes replaced, but left
                // uncoloured: the replaced text's offsets aren't the line's.
                let valid = matches!(lossy, Cow::Borrowed(_));
                buffer.clear();
                buffer.push_str(&lossy);
                buffer.push('\n');
                if let Some((ops, scopes)) = states.feed(self.grammar, feed, &buffer)
                    && valid
                {
                    spans = colored(highlighter, theme, scopes, &ops, &buffer, default);
                }
            }
            if kept.lines.len() >= KEPT_LINES {
                kept.lines.clear();
            }
            kept.lines.insert(at, spans);
            at = at.saturating_add(1);
            if started.elapsed().saturating_add(kept.spent) >= FRAME_BUDGET && at <= line {
                kept.missed = true;
                break;
            }
        }
        kept.frontier = Some((at, states));
        kept.spent = kept.spent.saturating_add(started.elapsed());
        if let Some(spans) = kept.lines.get(&line) {
            out.extend_from_slice(spans);
        }
    }
}

impl Syntax {
    /// Whether `line` (and the lines before it) may be coloured now: in a browser, once the
    /// background parse has been through it, every regex it takes compiled.
    #[cfg(target_arch = "wasm32")]
    fn may_colour(&self, line: u32) -> bool {
        line < self.walk.parsed.load(Ordering::Acquire)
    }

    /// Whether `line` may be coloured now: always, here (a thread may wait for another
    /// compiling a regex).
    #[cfg(not(target_arch = "wasm32"))]
    #[expect(clippy::unused_self, reason = "a browser's asks its parse")]
    const fn may_colour(&self, _line: u32) -> bool {
        true
    }
}

/// The most a snippet coloured whole may be ([`highlight_snippet`]), in bytes.
const SNIPPET_LIMIT: usize = 64 << 10;

/// A snippet's lines coloured in `language` (a fence's word: `rust`, `py`, `sh`; a name or an
/// extension), each line's spans where they differ from `default`; `None` for a language
/// unknown or a snippet too long. Small texts only (an agent's code block): parsed whole, at
/// once.
pub(crate) fn highlight_snippet(
    language: &str,
    code: &str,
    dark: bool,
    default: Color32,
) -> Option<Vec<Vec<Span>>> {
    if code.len() > SNIPPET_LIMIT || language.is_empty() {
        return None;
    }
    let grammar = SYNTAXES
        .find_syntax_by_token(language)
        .filter(|grammar| grammar.name != "Plain Text")?;
    let theme = theme(dark);
    let [dark_highlighter, light_highlighter] = &*HIGHLIGHTERS;
    let highlighter = if dark {
        dark_highlighter
    } else {
        light_highlighter
    };
    let mut stream = Stream::new(grammar);
    let mut buffer = String::new();
    let mut lines = Vec::new();
    for line in code.lines() {
        if line.len() > MAX_LINE {
            lines.push(Vec::new());
            continue;
        }
        buffer.clear();
        buffer.push_str(line);
        buffer.push('\n');
        let spans = stream
            .feed(grammar, &buffer)
            .map(|(ops, scopes)| colored(highlighter, theme, scopes, &ops, &buffer, default))
            .unwrap_or_default();
        lines.push(spans);
    }
    Some(lines)
}

/// The theme for the dark or the light look.
fn theme(dark: bool) -> &'static Theme {
    let name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    THEMES
        .themes
        .get(name)
        .or_else(|| THEMES.themes.values().next())
        .unwrap_or_else(|| default_theme())
}

/// A theme without colours, should syntect ship none.
fn default_theme() -> &'static Theme {
    static EMPTY: LazyLock<Theme> = LazyLock::new(Theme::default);
    &EMPTY
}

/// The colours of `line` (`scopes` before its `ops`), where they differ from the theme's
/// default.
fn colored(
    highlighter: &Highlighter<'_>,
    theme: &Theme,
    scopes: ScopeStack,
    ops: &[(usize, syntect::parsing::ScopeStackOp)],
    line: &str,
    default: Color32,
) -> Vec<Span> {
    let plain = theme.settings.foreground;
    let mut state = HighlightState::new(highlighter, scopes);
    let mut spans: Vec<Span> = Vec::new();
    let mut offset = 0_u32;
    for (style, piece) in HighlightIterator::new(&mut state, ops, line, highlighter) {
        let end = offset.saturating_add(u32::try_from(piece.len()).unwrap_or(u32::MAX));
        let color = Some(style.foreground)
            .filter(|color| Some(*color) != plain)
            .map_or(default, |color| {
                Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
            });
        match spans.last_mut() {
            Some(last) if last.color == color && last.bytes.end == offset => last.bytes.end = end,
            _ => spans.push(Span {
                bytes: offset..end,
                color,
            }),
        }
        offset = end;
    }
    spans
}

/// Parses `lines` lines of `bytes` in order, keeping the states every [`CHECKPOINT`] lines.
async fn parse(syntax: &SyntaxReference, bytes: &[u8], lines: u32, feeds: &Feeds, walk: &Walk) {
    let mut states = States::new(syntax);
    let mut buffer = String::new();
    let mut rest = bytes;
    // In a browser, the page draws meanwhile (the walk has its thread elsewhere).
    let mut pace = jobs::Pace::new();
    for line in 0..lines {
        if walk.stop.load(Ordering::Relaxed) {
            return;
        }
        pace.step().await;
        if line.checked_rem(CHECKPOINT) == Some(0) {
            let mut checkpoints = walk.checkpoints.lock();
            let kept = match checkpoints.last() {
                Some(last) if **last == states => Arc::clone(last),
                _ => Arc::new(states.clone()),
            };
            checkpoints.push(kept);
        }
        let end = memchr::memchr(b'\n', rest).unwrap_or(rest.len());
        let (text, after) = rest.split_at(end);
        rest = after.get(1..).unwrap_or_default();
        let feed = feeds.of(line, text.len());
        if feed != Feed::Skip {
            buffer.clear();
            buffer.push_str(&String::from_utf8_lossy(text));
            buffer.push('\n');
            let _ = states.feed(syntax, feed, &buffer);
        }
        walk.parsed.store(line.saturating_add(1), Ordering::Release);
    }
    walk.done.store(true, Ordering::Relaxed);
}

/// The grammar for a file named `path` (by its extension, or its whole name: `Makefile`), or
/// for its first line (`#!/bin/sh`); `None` for plain text.
fn language(path: &[u8], first_line: &[u8]) -> Option<&'static SyntaxReference> {
    let syntaxes = &*SYNTAXES;
    let name = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    let name = std::str::from_utf8(name).ok();
    name.and_then(|name| {
        name.rsplit_once('.')
            .and_then(|(_, extension)| syntaxes.find_syntax_by_extension(extension))
            .or_else(|| syntaxes.find_syntax_by_extension(name))
    })
    .or_else(|| {
        std::str::from_utf8(first_line)
            .ok()
            .and_then(|line| syntaxes.find_syntax_by_first_line(line))
    })
    .filter(|syntax| syntax.name != "Plain Text")
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::ops::Range;

    use super::*;

    /// The byte ranges of `spans`.
    fn ranges(spans: &[Span]) -> Vec<Range<u32>> {
        spans.iter().map(|span| span.bytes.clone()).collect()
    }

    #[test]
    fn snippets_are_coloured_by_their_fence_word() {
        let default = Color32::WHITE;
        let lines =
            highlight_snippet("rust", "fn main() {\n    let x = 1;\n}", true, default).unwrap();
        assert_eq!(lines.len(), 3);
        // `fn` and `let` coloured apart from the default.
        assert!(lines[0].iter().any(|span| span.color != default));
        assert!(lines[1].iter().any(|span| span.color != default));
        assert!(highlight_snippet("not-a-language", "x", true, default).is_none());
        assert!(highlight_snippet("", "x", true, default).is_none());
    }

    #[test]
    fn languages_come_from_names_and_first_lines() {
        let name = |path: &str, first: &str| {
            language(path.as_bytes(), first.as_bytes()).map(|syntax| syntax.name.as_str())
        };
        assert_eq!(name("src/main.rs", ""), Some("Rust"));
        assert_eq!(name("kernel/sched/core.c", ""), Some("C"));
        assert_eq!(name("Makefile", ""), Some("Makefile"));
        assert_eq!(
            name("bin/tool", "#!/bin/sh"),
            Some("Bourne Again Shell (bash)")
        );
        assert_eq!(name("notes.txt", "just words"), None);
        assert_eq!(name("LICENSE", "MIT License"), None);
    }

    /// Waits for the background parse to keep `count` states.
    fn parsed(syntax: &Syntax, count: usize) {
        let started = Instant::now();
        while syntax.walk.checkpoints.lock().len() < count {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the parse stalled"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Colours `line` as frames do, a frame's budget at a time, until it is done.
    fn colour(syntax: &Syntax, text: &Text, line: u32) -> Vec<Span> {
        let started = Instant::now();
        let mut spans = Vec::new();
        loop {
            spans.clear();
            syntax.begin_frame();
            syntax.colors(text, line, true, Color32::WHITE, &mut spans);
            if !syntax.needs_repaint() {
                return spans;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "colouring stalled"
            );
        }
    }

    #[test]
    fn colours_a_line_from_the_nearest_state() {
        let mut source = String::from("fn main() {\n    let s = \"/* not a comment */\";\n");
        for line in 0..300 {
            let _ = writeln!(source, "    let x{line} = {line};");
        }
        source.push_str("}\n");
        let text = Text::new(source.into_bytes()).unwrap_or_default();
        let syntax = Syntax::start(b"main.rs", &text, Feeds::All(Feed::New)).unwrap();
        parsed(&syntax, 3);
        let spans = colour(&syntax, &text, 1);
        // `let`, the name, the string: coloured apart from the default.
        assert!(spans.len() >= 3, "{spans:?}");
        assert!(spans.iter().any(|span| span.color != Color32::WHITE));
        // A line far down starts from its checkpoint and matches a walk from the top.
        let far = ranges(&colour(&syntax, &text, 290));
        let fresh = Syntax::start(b"main.rs", &text, Feeds::All(Feed::New)).unwrap();
        parsed(&fresh, 1);
        let mut walked = Vec::new();
        for line in 0..=290 {
            walked = colour(&fresh, &text, line);
        }
        assert_ne!(far, []);
        assert_eq!(far, ranges(&walked));
    }

    #[test]
    fn a_frame_stops_colouring_at_its_budget() {
        let source = "fn f() { let a = 1; }\n".repeat(20_000);
        let text = Text::new(source.into_bytes()).unwrap_or_default();
        let syntax = Syntax::start(b"lib.rs", &text, Feeds::All(Feed::New)).unwrap();
        parsed(&syntax, 1);
        // A frame whose budget is spent colours nothing, and asks for another.
        let mut spans = Vec::new();
        syntax.begin_frame();
        syntax.kept.lock().spent = FRAME_BUDGET;
        syntax.colors(&text, 127, true, Color32::WHITE, &mut spans);
        assert_eq!(spans, []);
        assert!(syntax.needs_repaint());
        // Later frames carry on from where the last one stopped, until the line is reached.
        assert_ne!(colour(&syntax, &text, 127), []);
    }

    #[test]
    fn a_line_too_long_to_parse_is_left_plain_and_passed_over() {
        // The long line opens a string it never closes: parsed, the next line would be in it.
        let long = format!("let s = \"{}\n", "x".repeat(MAX_LINE));
        let source = format!("fn first() {{}}\n{long}fn after() {{}}\n");
        let text = Text::new(source.into_bytes()).unwrap_or_default();
        let syntax = Syntax::start(b"lib.rs", &text, Feeds::All(Feed::New)).unwrap();
        parsed(&syntax, 1);
        assert_eq!(colour(&syntax, &text, 1), []);
        let (first, after) = (colour(&syntax, &text, 0), colour(&syntax, &text, 2));
        assert!(first.len() >= 2, "{first:?}");
        assert_eq!(after, first);
    }

    #[test]
    fn plain_text_isnt_highlighted() {
        let text = Text::new(b"hello\n".to_vec()).unwrap_or_default();
        assert!(Syntax::start(b"README", &text, Feeds::All(Feed::New)).is_none());
    }
}
