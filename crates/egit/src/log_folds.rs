//! A job's log with some of its sections folded, as GitLab's and GitHub's pages fold them: their
//! lines left out of what the text grid shows, each folded header ending in how many lines it
//! holds ("⋯ 120 lines"), every line's number and time the whole log's, so a fold reads as a jump
//! in the gutter. Which sections are folded is the view's ([`Folds`]): as the user folded or
//! unfolded them, else as the forge shows them (GitLab's `[collapsed=true]`, GitHub's groups, a
//! GitHub step that didn't fail), a section under way never. A log with nothing folded is shown
//! as it is ([`Shown::Whole`]); a folded one is made again as the log grows or folds change
//! ([`Folding`]), on the interface's thread while it is small, else in a job (the last one shown
//! meanwhile).

use std::collections::HashMap;
use std::sync::Arc;

use egit_forge::Section;
use egit_git::sync::Mutex;
use egit_text::{Measure, Text};
use egui::Color32;

use crate::grid::{GridSource, Span};
use crate::job_log::LogSource;
use crate::theme::Palette;

/// The largest log folded on the interface's thread; a larger one is folded in a job.
const FOLDED_AT_ONCE: usize = 8 * 1024 * 1024;

/// Which of a log's sections are folded.
#[derive(Clone, Debug, Default)]
pub(crate) struct Folds {
    /// Sections the user folded (true) or unfolded (false), by their header's line.
    set: HashMap<u32, bool>,
    /// Bumped as one changes.
    version: u64,
}

impl Folds {
    /// Whether `section` of `log` is folded: as the user set it, else as its forge shows it
    /// (one under way never); one holding nothing below its header never is.
    pub(crate) fn is_folded(&self, log: &LogSource, section: &Section) -> bool {
        log.folds(section)
            && self
                .set
                .get(&section.line)
                .copied()
                .unwrap_or_else(|| section.collapsed && section.ended.is_some())
    }

    /// Folds `section` (`folded`) or unfolds it.
    pub(crate) fn set(&mut self, section: &Section, folded: bool) {
        self.set.insert(section.line, folded);
        self.version = self.version.wrapping_add(1);
    }

    /// Folds `section` of `log` if it is open, else unfolds it.
    pub(crate) fn toggle(&mut self, log: &LogSource, section: &Section) {
        let folded = self.is_folded(log, section);
        self.set(section, !folded);
    }

    /// Folds every section of `log` (`folded`), or unfolds them all.
    pub(crate) fn every(&mut self, log: &LogSource, folded: bool) {
        for section in &log.sections {
            self.set.insert(section.line, folded);
        }
        self.version = self.version.wrapping_add(1);
    }

    /// Unfolds every section of `log` holding line `line`, so it shows.
    pub(crate) fn reveal(&mut self, log: &LogSource, line: u32) {
        let mut changed = false;
        for section in &log.sections {
            if section.line < line && log.last_line(section) >= line && self.is_folded(log, section)
            {
                self.set.insert(section.line, false);
                changed = true;
            }
        }
        if changed {
            self.version = self.version.wrapping_add(1);
        }
    }

    /// Bumped as one changes.
    pub(crate) const fn version(&self) -> u64 {
        self.version
    }
}

/// A log with its folded sections' lines left out.
#[derive(Debug)]
pub(crate) struct FoldedLog {
    whole: Arc<LogSource>,
    text: Text,
    measure: Measure,
    /// Runs of lines shown, each `(shown, whole)`: from shown line `shown` on, the whole log's
    /// lines from `whole` on, until the next run's.
    runs: Vec<(u32, u32)>,
    /// The folded headers (shown lines), each with where its note starts in it.
    notes: Vec<(u32, u32)>,
}

impl FoldedLog {
    /// `whole` with the sections `folds` folds left out; none when nothing is.
    fn build(whole: Arc<LogSource>, folds: &Folds) -> Option<Self> {
        // The whole log's lines left out, as ranges: each folded section not inside another.
        let mut hidden: Vec<(u32, u32)> = Vec::new();
        for section in &whole.sections {
            if hidden.last().is_some_and(|(_, last)| section.line <= *last) {
                continue;
            }
            if folds.is_folded(&whole, section) {
                hidden.push((section.line.saturating_add(1), whole.last_line(section)));
            }
        }
        if hidden.is_empty() {
            return None;
        }
        let all = whole.text().bytes();
        let start = |line: u32| {
            whole
                .text()
                .start(line)
                .and_then(|start| usize::try_from(start).ok())
                .unwrap_or(all.len())
        };
        let mut text = Vec::with_capacity(all.len());
        let mut runs = Vec::with_capacity(hidden.len().saturating_add(1));
        let mut notes = Vec::with_capacity(hidden.len());
        let (mut next, mut shown) = (0_u32, 0_u32);
        for (first, last) in hidden {
            let header = first.saturating_sub(1);
            runs.push((shown, next));
            text.extend_from_slice(all.get(start(next)..start(header)).unwrap_or_default());
            let line = whole.text().line(header).unwrap_or_default();
            text.extend_from_slice(line);
            let count = last.saturating_sub(first).saturating_add(1);
            let note = if count == 1 {
                "  ⋯ 1 line".to_owned()
            } else {
                format!("  ⋯ {} lines", crate::format::count(u64::from(count)))
            };
            text.extend_from_slice(note.as_bytes());
            text.push(b'\n');
            shown = shown.saturating_add(header.saturating_sub(next).saturating_add(1));
            notes.push((
                shown.saturating_sub(1),
                u32::try_from(line.len()).unwrap_or(u32::MAX),
            ));
            next = last.saturating_add(1);
        }
        if next < whole.measure().lines() {
            runs.push((shown, next));
            text.extend_from_slice(all.get(start(next)..).unwrap_or_default());
        }
        let text = Text::new(text).ok()?;
        let measure = Measure::all(&text, whole.options);
        Some(Self {
            whole,
            text,
            measure,
            runs,
            notes,
        })
    }

    /// The run shown line `shown` is in, and how many lines it shows.
    fn run_of(&self, at: usize) -> Option<((u32, u32), u32)> {
        let run = *self.runs.get(at)?;
        let end = self
            .runs
            .get(at.saturating_add(1))
            .map_or_else(|| self.measure.lines(), |next| next.0);
        Some((run, end.saturating_sub(run.0)))
    }

    /// The whole log's line shown as line `shown`.
    fn whole_line(&self, shown: u32) -> u32 {
        let at = self
            .runs
            .partition_point(|(from, _)| *from <= shown)
            .saturating_sub(1);
        self.runs.get(at).map_or(shown, |(from, whole)| {
            whole.saturating_add(shown.saturating_sub(*from))
        })
    }

    /// The line showing the whole log's line `line`: itself, or the header of the folded
    /// section hiding it.
    fn shown_line(&self, line: u32) -> u32 {
        let at = self
            .runs
            .partition_point(|(_, whole)| *whole <= line)
            .saturating_sub(1);
        self.run_of(at).map_or(line, |((shown, whole), length)| {
            let into = line.saturating_sub(whole);
            if into < length {
                shown.saturating_add(into)
            } else {
                shown.saturating_add(length.saturating_sub(1))
            }
        })
    }

    /// The note ending shown line `line`, if it heads a folded section: where it starts.
    fn note(&self, line: u32) -> Option<u32> {
        let at = self.notes.partition_point(|(shown, _)| *shown < line);
        self.notes
            .get(at)
            .filter(|(shown, _)| *shown == line)
            .map(|(_, from)| *from)
    }
}

impl GridSource for FoldedLog {
    fn text(&self) -> &Text {
        &self.text
    }

    fn measure(&self) -> &Measure {
        &self.measure
    }

    fn colors(&self, line: u32, palette: &Palette, out: &mut Vec<Span>) {
        self.whole.colors(self.whole_line(line), palette, out);
        if let Some(from) = self.note(line) {
            // The note is the line's own, past what its colours cover.
            let end = self
                .text
                .line(line)
                .map_or(from, |text| u32::try_from(text.len()).unwrap_or(u32::MAX));
            out.retain(|span| span.bytes.start < from);
            if let Some(last) = out.last_mut() {
                last.bytes.end = last.bytes.end.min(from);
            }
            out.push(Span {
                bytes: from..end,
                color: palette.muted,
            });
        }
    }

    fn background(&self, line: u32, palette: &Palette) -> Option<Color32> {
        self.whole.background(self.whole_line(line), palette)
    }

    fn gutter(&self, line: u32, out: &mut String) {
        self.whole
            .gutter_of(self.whole_line(line), self.note(line).is_some(), out);
    }

    fn gutter_cells(&self) -> u32 {
        self.whole.gutter_cells()
    }

    fn trailing(&self, line: u32) -> Option<String> {
        self.whole.trailing_of(self.whole_line(line))
    }
}

/// What the grid shows of a log: all of it, or its folded sections left out.
#[derive(Clone, Debug)]
pub(crate) enum Shown {
    /// All of it.
    Whole(Arc<LogSource>),
    /// Some of its sections folded.
    Folded(Arc<FoldedLog>),
}

impl Shown {
    /// What the grid draws.
    pub(crate) fn source(&self) -> &dyn GridSource {
        match self {
            Self::Whole(whole) => &**whole,
            Self::Folded(folded) => &**folded,
        }
    }

    /// The whole log.
    pub(crate) fn whole(&self) -> &LogSource {
        match self {
            Self::Whole(whole) => whole,
            Self::Folded(folded) => &folded.whole,
        }
    }

    /// The whole log's line shown as line `shown`.
    pub(crate) fn whole_line(&self, shown: u32) -> u32 {
        match self {
            Self::Whole(_) => shown,
            Self::Folded(folded) => folded.whole_line(shown),
        }
    }

    /// The line showing the whole log's line `line` (a folded header for one it hides).
    pub(crate) fn shown_line(&self, line: u32) -> u32 {
        match self {
            Self::Whole(_) => line,
            Self::Folded(folded) => folded.shown_line(line),
        }
    }
}

/// Where a folding done in a job lands: what it made (none: nothing is folded), once done.
type Landing = Arc<Mutex<Option<Option<FoldedLog>>>>;

/// What is shown of a log as its folds and versions change, folded in a job when it is large.
#[derive(Debug, Default)]
pub(crate) struct Folding {
    /// Which sections are folded.
    pub(crate) folds: Folds,
    /// What was shown last: the log's version and the folds' it was made for.
    shown: Option<((u64, u64), Shown)>,
    /// A folding under way in a job: what it is for, and where it lands.
    working: Option<((u64, u64), Landing)>,
}

impl Folding {
    /// What to show of `whole` (its version `version`), made again as it or the folds change;
    /// and whether it changed since the last frame. A large log's folding goes on in a job,
    /// what was shown before showing meanwhile (`waker` woken as it lands).
    pub(crate) fn shown(
        &mut self,
        whole: &Arc<LogSource>,
        version: u64,
        waker: &egui::Context,
    ) -> (Shown, bool) {
        let key = (version, self.folds.version());
        let mut changed = false;
        if let Some((working, landed)) = &self.working {
            let done = landed.lock().take();
            if let Some(folded) = done {
                let working = *working;
                self.working = None;
                self.shown = Some((working, Self::of(whole, folded)));
                changed = true;
            }
        }
        let current = self.shown.as_ref().is_some_and(|(at, _)| *at == key);
        let asked = self.working.as_ref().is_some_and(|(at, _)| *at == key);
        if !current && !asked {
            if whole.text().bytes().len() <= FOLDED_AT_ONCE || self.shown.is_none() {
                let folded = FoldedLog::build(Arc::clone(whole), &self.folds);
                self.shown = Some((key, Self::of(whole, folded)));
                changed = true;
            } else {
                let landed = Arc::new(Mutex::new(None));
                let (into, whole, folds, waker) = (
                    Arc::clone(&landed),
                    Arc::clone(whole),
                    self.folds.clone(),
                    crate::repaint::Repaint::of(waker),
                );
                crate::jobs::detached(async move {
                    *into.lock() = Some(FoldedLog::build(whole, &folds));
                    waker.request_repaint();
                });
                self.working = Some((key, landed));
            }
        }
        let shown = self.shown.as_ref().map_or_else(
            || Shown::Whole(Arc::clone(whole)),
            |(_, shown)| shown.clone(),
        );
        (shown, changed)
    }

    /// Whether what is shown is made for the log's version `version` and the folds as they
    /// are (nothing being folded in a job).
    pub(crate) fn is_settled(&self, version: u64) -> bool {
        self.working.is_none()
            && self
                .shown
                .as_ref()
                .is_some_and(|(at, _)| *at == (version, self.folds.version()))
    }

    /// `folded`, or `whole` as it is when nothing is.
    fn of(whole: &Arc<LogSource>, folded: Option<FoldedLog>) -> Shown {
        folded.map_or_else(
            || Shown::Whole(Arc::clone(whole)),
            |folded| Shown::Folded(Arc::new(folded)),
        )
    }
}

#[cfg(test)]
mod tests {
    use egit_forge::LogParts;

    use super::*;

    fn section(name: &str, (line, last): (u32, u32), collapsed: bool, depth: u16) -> Section {
        Section {
            name: name.to_owned(),
            line,
            started: 1,
            ended: Some((2, last)),
            collapsed,
            depth,
            status: None,
        }
    }

    /// Ten lines: section a (lines 1–4, b inside it at 2–3), and c (6–8), folded by its forge.
    fn log() -> Arc<LogSource> {
        let text = (0..10).fold(String::new(), |mut text, line| {
            use std::fmt::Write as _;
            let _ = writeln!(text, "line {line}");
            text
        });
        let parts = LogParts {
            text: text.into_bytes(),
            sections: vec![
                section("a", (1, 4), false, 0),
                section("b", (2, 3), false, 1),
                section("c", (6, 8), true, 0),
            ],
            ..LogParts::default()
        };
        Arc::new(LogSource::of(parts, egit_text::Options::default()).unwrap())
    }

    fn text(shown: &Shown) -> String {
        String::from_utf8_lossy(shown.source().text().bytes()).into_owned()
    }

    #[test]
    fn a_section_folded_by_its_forge_starts_folded_and_its_lines_map_back() {
        let whole = log();
        let mut folding = Folding::default();
        let waker = egui::Context::default();
        let (shown, changed) = folding.shown(&whole, 1, &waker);
        assert!(changed);
        assert_eq!(
            text(&shown),
            "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6  ⋯ 2 lines\nline 9\n"
        );
        assert_eq!(shown.whole_line(7), 9);
        assert_eq!(shown.shown_line(9), 7);
        // A hidden line shows as its folded header.
        assert_eq!(shown.shown_line(7), 6);
        let mut gutter = String::new();
        shown.source().gutter(6, &mut gutter);
        assert!(gutter.ends_with('▸'), "{gutter}");
        gutter.clear();
        shown.source().gutter(1, &mut gutter);
        assert!(gutter.ends_with('▾'), "{gutter}");
        // Nothing changed: nothing made again.
        assert!(!folding.shown(&whole, 1, &waker).1);
    }

    #[test]
    fn folding_a_section_hides_what_it_holds_and_unfolding_shows_it_again() {
        let whole = log();
        let mut folding = Folding::default();
        let waker = egui::Context::default();
        let a = whole.sections.first().unwrap().clone();
        folding.folds.toggle(&whole, &a);
        let (shown, _) = folding.shown(&whole, 1, &waker);
        assert_eq!(
            text(&shown),
            "line 0\nline 1  ⋯ 3 lines\nline 5\nline 6  ⋯ 2 lines\nline 9\n"
        );
        assert_eq!(shown.whole_line(2), 5);
        // Going to line 7 unfolds c, and only c.
        folding.folds.reveal(&whole, 7);
        let (shown, changed) = folding.shown(&whole, 1, &waker);
        assert!(changed);
        assert_eq!(
            text(&shown),
            "line 0\nline 1  ⋯ 3 lines\nline 5\nline 6\nline 7\nline 8\nline 9\n"
        );
        folding.folds.every(&whole, false);
        let (shown, _) = folding.shown(&whole, 1, &waker);
        assert!(matches!(shown, Shown::Whole(_)));
        assert!(folding.is_settled(1));
    }
}
