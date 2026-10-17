//! A file in conflict on the text grid: what merged, and each conflict's sides under headers
//! saying where they come from, or, once picked, what resolves it.
//!
//! The conflict is read and its sides merged by a job ([`read_conflict`]). The document is made
//! again, in a job, whenever a choice changes ([`build`]): the merged text copied once with each
//! conflict shown or resolved, measured, and highlighted (the current side's lines through one
//! parser, the incoming side's through the other, so neither side's syntax breaks the other's).

use std::fmt::Write as _;
use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, ConflictLines, ConflictSides, Git, IncomingCommit, Merge, MergedText, Operation,
    Pick, Piece, Repository, conflict_sides, incoming_commit, merge_sides,
};
use egit_text::{Measure, Options, Text};
use egui::Color32;

use crate::grid::{GridSource, Span};
use crate::jobs;
use crate::problem::Problem;
use crate::syntax::{Feed, Feeds, Syntax};
use crate::theme::Palette;

/// A line in no conflict.
const NO_CONFLICT: u32 = u32::MAX;

/// A file in conflict: its sides, their merge, and the commit coming in.
#[derive(Debug)]
pub(crate) struct ConflictFile {
    pub(crate) path: BString,
    pub(crate) sides: ConflictSides,
    pub(crate) merge: Merge,
    pub(crate) incoming: Option<IncomingCommit>,
}

impl ConflictFile {
    /// Its two texts merged, if it is two texts.
    pub(crate) const fn text(&self) -> Option<&MergedText> {
        match &self.merge {
            Merge::Text(text) => Some(text),
            Merge::WholeOnly(_) => None,
        }
    }
}

/// Reads the conflict at `path` (`operation` names the incoming commit): `None` once it is
/// resolved.
pub(crate) async fn read_conflict(
    git: Git,
    repository: Arc<Repository>,
    path: BString,
    operation: Option<Operation>,
    cancel: CancelToken,
) -> Result<Option<Arc<ConflictFile>>, Problem> {
    jobs::blocking(move || {
        let failed = |error| Problem::git("The conflict couldn't be read", &error);
        let Some(sides) =
            conflict_sides(&git, &repository, path.as_ref(), &cancel).map_err(failed)?
        else {
            return Ok(None);
        };
        let merge = merge_sides(&git, &repository, &sides, &cancel).map_err(failed)?;
        let incoming = match operation {
            Some(operation) => {
                incoming_commit(&git, &repository, operation, &cancel).map_err(failed)?
            }
            None => None,
        };
        Ok(Some(Arc::new(ConflictFile {
            path,
            sides,
            merge,
            incoming,
        })))
    })
    .await
}

/// How a conflict is resolved, so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Not yet.
    Open,
    /// The current side's lines.
    Current,
    /// The incoming side's.
    Incoming,
    /// The base's: neither side's change.
    Base,
    /// Both, the current side's first.
    CurrentThenIncoming,
    /// Both, the incoming side's first.
    IncomingThenCurrent,
    /// Text of the user's own.
    Edited(Arc<[u8]>),
}

impl Choice {
    /// The pick it makes; `None` while open.
    pub(crate) fn pick(&self) -> Option<Pick<'_>> {
        Some(match self {
            Self::Open => return None,
            Self::Current => Pick::Current,
            Self::Incoming => Pick::Incoming,
            Self::Base => Pick::Base,
            Self::CurrentThenIncoming => Pick::CurrentThenIncoming,
            Self::IncomingThenCurrent => Pick::IncomingThenCurrent,
            Self::Edited(text) => Pick::Text(text),
        })
    }

    /// What it took, for people.
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::Open => "not resolved",
            Self::Current => "the current side",
            Self::Incoming => "the incoming side",
            Self::Base => "the base",
            Self::CurrentThenIncoming => "both, current first",
            Self::IncomingThenCurrent => "both, incoming first",
            Self::Edited(_) => "edited",
        }
    }
}

/// Where the sides come from, as their headers say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Labels {
    /// The current side: `HEAD`, and its branch.
    pub(crate) current: String,
    /// The incoming side: the commit merged, picked, reverted, or replayed.
    pub(crate) incoming: String,
}

/// What a line of the document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Merged: the same whatever is picked.
    Merged,
    /// A side's header.
    Header(Side),
    /// A side's line, in an open conflict.
    Side(Side),
    /// A resolved conflict's line.
    Resolved,
}

/// A side of a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Current,
    Base,
    Incoming,
}

/// A line of the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Line {
    kind: Kind,
    /// Its conflict ([`NO_CONFLICT`] for merged lines).
    conflict: u32,
    /// Its number in the resolved file (0 for lines that won't be in it as they are).
    number: u32,
}

/// A conflicted file shown: what merged, each open conflict's sides, each resolved one's lines.
#[derive(Debug)]
pub(crate) struct MergeDocument {
    text: Text,
    measure: Measure,
    lines: Vec<Line>,
    /// Each conflict's first line (where a resolved conflict without lines would be).
    starts: Vec<u32>,
    /// Digits of the largest line number.
    digits: u32,
    syntax: Option<Syntax>,
}

/// Makes the document showing `file` resolved as far as `choices` go (the base shown in open
/// conflicts with `show_base`), in a job.
pub(crate) async fn build(
    file: Arc<ConflictFile>,
    choices: Arc<[Choice]>,
    show_base: bool,
    labels: Labels,
) -> Result<Arc<MergeDocument>, Problem> {
    jobs::blocking(move || {
        let text = file.text().ok_or_else(|| {
            Problem::new(
                "There is nothing to merge",
                "The file's sides aren't two texts.",
            )
        })?;
        Ok(Arc::new(MergeDocument::new(
            text,
            file.path.as_ref(),
            &choices,
            show_base,
            &labels,
        )))
    })
    .await
}

/// The text and lines of a document, as they are made.
#[derive(Default)]
struct Builder {
    bytes: Vec<u8>,
    lines: Vec<Line>,
    feeds: Vec<Feed>,
    number: u32,
}

impl Builder {
    /// Adds `text`'s lines (a last one without a newline gets one, to end its line here).
    fn add(&mut self, text: &[u8], kind: Kind, conflict: u32) {
        let feed = match kind {
            Kind::Merged | Kind::Resolved => Feed::Both,
            Kind::Side(Side::Current) => Feed::Old,
            Kind::Side(Side::Incoming) => Feed::New,
            Kind::Side(Side::Base) | Kind::Header(_) => Feed::Skip,
        };
        for line in text.lines_with_terminator() {
            self.bytes.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                self.bytes.push(b'\n');
            }
            let number = if matches!(kind, Kind::Merged | Kind::Resolved) {
                self.number = self.number.saturating_add(1);
                self.number
            } else {
                0
            };
            self.lines.push(Line {
                kind,
                conflict,
                number,
            });
            self.feeds.push(feed);
        }
    }

    /// The line the next one added will be.
    fn next_line(&self) -> u32 {
        u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
    }
}

impl MergeDocument {
    /// The document showing `merged` resolved as far as `choices` go, highlighted as the file at
    /// `path` (on a job's thread: it copies and measures the whole file).
    pub(crate) fn new(
        merged: &MergedText,
        path: &BStr,
        choices: &[Choice],
        show_base: bool,
        labels: &Labels,
    ) -> Self {
        let count = merged.conflicts();
        let size: usize = merged
            .pieces()
            .iter()
            .map(|piece| match piece {
                Piece::Merged(range) => range.len(),
                Piece::Conflict(lines) => lines.current.len().saturating_add(lines.incoming.len()),
            })
            .sum();
        let mut builder = Builder {
            bytes: Vec::with_capacity(size),
            ..Builder::default()
        };
        let mut starts = Vec::with_capacity(count);
        let mut index = 0_usize;
        for piece in merged.pieces() {
            match piece {
                Piece::Merged(range) => builder.add(merged.bytes(range), Kind::Merged, NO_CONFLICT),
                Piece::Conflict(lines) => {
                    let conflict = u32::try_from(index).unwrap_or(NO_CONFLICT);
                    starts.push(builder.next_line());
                    let choice = choices.get(index).unwrap_or(&Choice::Open);
                    if let Some(resolved) =
                        choice.pick().and_then(|pick| merged.picked(index, pick))
                    {
                        builder.add(&resolved, Kind::Resolved, conflict);
                    } else {
                        let order = (index.saturating_add(1), count);
                        open(
                            &mut builder,
                            merged,
                            lines,
                            conflict,
                            order,
                            show_base,
                            labels,
                        );
                    }
                    index = index.saturating_add(1);
                }
            }
        }
        let Builder {
            bytes,
            lines,
            feeds,
            number,
        } = builder;
        // A file past 4 GiB shows as nothing.
        let text = Text::new(bytes).unwrap_or_default();
        let measure = Measure::all(&text, Options::default());
        Self {
            syntax: Syntax::start(path, &text, Feeds::Each(feeds.into())),
            text,
            measure,
            lines,
            starts,
            digits: number.max(1).ilog10().saturating_add(1),
        }
    }

    /// Conflict `index`'s first line.
    pub(crate) fn start(&self, index: usize) -> Option<u32> {
        self.starts.get(index).copied()
    }

    /// The conflict line `line` is in, if any.
    pub(crate) fn conflict_at(&self, line: u32) -> Option<usize> {
        let line = self.lines.get(usize::try_from(line).ok()?)?;
        (line.conflict != NO_CONFLICT)
            .then(|| usize::try_from(line.conflict).ok())
            .flatten()
    }

    fn line(&self, line: u32) -> Option<&Line> {
        self.lines.get(usize::try_from(line).ok()?)
    }
}

/// Adds an open conflict: each side under its header (the base's only with `show_base`).
fn open(
    builder: &mut Builder,
    merged: &MergedText,
    lines: &ConflictLines,
    conflict: u32,
    (number, count): (usize, usize),
    show_base: bool,
    labels: &Labels,
) {
    let sides = [
        Some((Side::Current, &lines.current, labels.current.as_str())),
        show_base.then_some((Side::Base, &lines.base, "what both started from")),
        Some((Side::Incoming, &lines.incoming, labels.incoming.as_str())),
    ];
    for (side, range, label) in sides.into_iter().flatten() {
        let bytes = merged.bytes(range);
        let mut header = String::new();
        if side == Side::Current {
            let _ = write!(header, "Conflict {number} of {count} · ");
        }
        header.push_str(match side {
            Side::Current => "current",
            Side::Base => "base",
            Side::Incoming => "incoming",
        });
        if !label.is_empty() {
            let _ = write!(header, ": {label}");
        }
        if bytes.is_empty() {
            header.push_str(" (no lines)");
        }
        builder.add(header.as_bytes(), Kind::Header(side), conflict);
        builder.add(bytes, Kind::Side(side), conflict);
    }
}

/// A document as shown, its selected conflict marked.
pub(crate) struct Shown<'a> {
    pub(crate) document: &'a MergeDocument,
    pub(crate) selected: Option<usize>,
}

impl Shown<'_> {
    fn is_selected(&self, line: &Line) -> bool {
        self.selected
            .is_some_and(|selected| usize::try_from(line.conflict).ok() == Some(selected))
    }
}

impl GridSource for Shown<'_> {
    fn text(&self) -> &Text {
        &self.document.text
    }

    fn measure(&self) -> &Measure {
        &self.document.measure
    }

    fn colors(&self, line: u32, palette: &Palette, out: &mut Vec<Span>) {
        let Some(at) = self.document.line(line) else {
            return;
        };
        if let Kind::Header(_) = at.kind {
            // The selected conflict's headers stand out (on stronger colours: in the text's).
            let color = if self.is_selected(at) {
                palette.text
            } else {
                palette.muted
            };
            out.push(Span {
                bytes: 0..u32::MAX,
                color,
            });
        } else if let Some(syntax) = &self.document.syntax {
            syntax.colors(&self.document.text, line, palette.dark, palette.text, out);
        }
    }

    fn begin_frame(&self) {
        if let Some(syntax) = &self.document.syntax {
            syntax.begin_frame();
        }
    }

    fn needs_repaint(&self) -> bool {
        self.document
            .syntax
            .as_ref()
            .is_some_and(Syntax::needs_repaint)
    }

    fn background(&self, line: u32, palette: &Palette) -> Option<Color32> {
        let at = self.document.line(line)?;
        let selected = self.is_selected(at);
        match at.kind {
            Kind::Merged => None,
            Kind::Header(Side::Current) if selected => Some(palette.current_header),
            Kind::Header(Side::Incoming) if selected => Some(palette.incoming_header),
            Kind::Header(Side::Current) | Kind::Side(Side::Current) => Some(palette.current),
            Kind::Header(Side::Incoming) | Kind::Side(Side::Incoming) => Some(palette.incoming),
            Kind::Header(Side::Base) | Kind::Side(Side::Base) => Some(palette.hunk),
            Kind::Resolved => Some(palette.resolved),
        }
    }

    fn gutter(&self, line: u32, out: &mut String) {
        let Some(at) = self.document.line(line) else {
            return;
        };
        let width = usize::try_from(self.document.digits).unwrap_or(1);
        if at.number == 0 {
            let _ = write!(out, "{:width$}", "");
        } else {
            let _ = write!(out, "{:>width$}", at.number);
        }
        out.push(' ');
        out.push(match at.kind {
            Kind::Side(Side::Current) => '<',
            Kind::Side(Side::Base) => '|',
            Kind::Side(Side::Incoming) => '>',
            Kind::Merged | Kind::Header(_) | Kind::Resolved => ' ',
        });
    }

    fn gutter_cells(&self) -> u32 {
        self.document.digits.saturating_add(3)
    }
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// The document's lines' text, each with its kind, conflict, and number.
    fn shown(document: &MergeDocument) -> Vec<(String, Kind, u32, u32)> {
        (0..document.measure.lines())
            .map(|index| {
                let line = document.line(index).unwrap();
                let text = document
                    .text
                    .line(index)
                    .unwrap()
                    .to_str_lossy()
                    .into_owned();
                (text, line.kind, line.conflict, line.number)
            })
            .collect()
    }

    fn labels() -> Labels {
        Labels {
            current: "HEAD (main)".to_owned(),
            incoming: "1234abcd Fix it".to_owned(),
        }
    }

    #[test]
    fn open_conflicts_show_their_sides_and_resolved_ones_their_lines() {
        let merged = MergedText::parse(
            b"a\n<<<<<<< c\nB1\n||||||| b\nb\n=======\n>>>>>>> i\nc\n".to_vec(),
            7,
        )
        .unwrap();
        let open = MergeDocument::new(&merged, BStr::new("f"), &[Choice::Open], true, &labels());
        let lines = shown(&open);
        assert_eq!(
            lines.iter().map(|line| line.0.as_str()).collect::<Vec<_>>(),
            [
                "a",
                "Conflict 1 of 1 · current: HEAD (main)",
                "B1",
                "base: what both started from",
                "b",
                "incoming: 1234abcd Fix it (no lines)",
                "c",
            ]
        );
        assert_eq!(lines[2].1, Kind::Side(Side::Current));
        assert_eq!((lines[2].2, lines[2].3), (0, 0));
        // Numbered as the resolved file will be, without the open conflict.
        assert_eq!((lines[0].3, lines[6].3), (1, 2));
        assert_eq!(open.start(0), Some(1));
        assert_eq!(open.conflict_at(4), Some(0));
        assert_eq!(open.conflict_at(6), None);

        let picked = MergeDocument::new(
            &merged,
            BStr::new("f"),
            &[Choice::CurrentThenIncoming],
            false,
            &labels(),
        );
        let lines = shown(&picked);
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.0.as_str(), line.3))
                .collect::<Vec<_>>(),
            [("a", 1), ("B1", 2), ("c", 3)]
        );
        assert_eq!(lines[1].1, Kind::Resolved);
        // Resolved to nothing: it starts where it would be.
        let none = MergeDocument::new(
            &merged,
            BStr::new("f"),
            &[Choice::Incoming],
            false,
            &labels(),
        );
        assert_eq!(none.start(0), Some(1));
        assert_eq!(shown(&none).len(), 2);
    }
}
