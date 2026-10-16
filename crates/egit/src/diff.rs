//! A file's diff on the text grid: its lines without their `+`, `-`, or ` `, each on the
//! background of what it is, numbered in the old and new file in the gutter.

use std::fmt::Write as _;
use std::sync::Arc;

use bstr::BString;
use egit_git::{
    CancelToken, DiffLine, DiffLineKind, DiffOptions, FileDiff, Git, ObjectId, Repository,
    file_diff,
};
use egit_text::{Measure, Options, Text};
use egui::Color32;

use crate::grid::{GridSource, Styled};
use crate::jobs;
use crate::problem::Problem;
use crate::theme::Palette;

/// A file's diff, measured, ready to draw.
#[derive(Debug)]
pub(crate) struct DiffDocument {
    text: Text,
    measure: Measure,
    lines: Vec<DiffLine>,
    binary: bool,
    /// Digits of the largest line number.
    digits: u32,
    added: u32,
    removed: u32,
}

impl DiffDocument {
    /// `diff`, measured (on a job's thread, for a large one).
    fn new(diff: FileDiff) -> Self {
        let FileDiff {
            text,
            lines,
            binary,
        } = diff;
        // A patch past 4 GiB shows as nothing.
        let text = Text::new(text).unwrap_or_default();
        let measure = Measure::all(&text, Options::default());
        let largest = lines
            .iter()
            .map(|line| line.old.max(line.new))
            .max()
            .unwrap_or(0);
        let count = |kind| {
            let lines = lines.iter().filter(|line| line.kind == kind).count();
            u32::try_from(lines).unwrap_or(u32::MAX)
        };
        Self {
            added: count(DiffLineKind::Added),
            removed: count(DiffLineKind::Removed),
            text,
            measure,
            lines,
            binary,
            digits: largest.max(1).ilog10().saturating_add(1),
        }
    }

    /// Whether git found the file binary (there are no lines to show then).
    pub(crate) const fn binary(&self) -> bool {
        self.binary
    }

    /// How many lines were added, and removed.
    pub(crate) const fn changed(&self) -> (u32, u32) {
        (self.added, self.removed)
    }

    fn line(&self, line: u32) -> Option<&DiffLine> {
        self.lines.get(usize::try_from(line).ok()?)
    }
}

impl GridSource for DiffDocument {
    fn text(&self) -> &Text {
        &self.text
    }

    fn measure(&self) -> &Measure {
        &self.measure
    }

    fn styles(&self, line: u32, palette: &Palette, out: &mut Vec<Styled>) {
        if self
            .line(line)
            .is_some_and(|line| line.kind == DiffLineKind::Hunk)
        {
            out.push(Styled {
                bytes: 0..u32::MAX,
                color: palette.muted,
            });
        }
    }

    fn background(&self, line: u32, palette: &Palette) -> Option<Color32> {
        match self.line(line)?.kind {
            DiffLineKind::Added => Some(palette.added),
            DiffLineKind::Removed => Some(palette.removed),
            DiffLineKind::Hunk => Some(palette.hunk),
            DiffLineKind::Context => None,
        }
    }

    fn gutter(&self, line: u32, out: &mut String) {
        let Some(line) = self.line(line) else {
            return;
        };
        let width = usize::try_from(self.digits).unwrap_or(1);
        let number = |out: &mut String, number: u32| {
            if number == 0 {
                let _ = write!(out, "{:width$}", "");
            } else {
                let _ = write!(out, "{number:>width$}");
            }
        };
        number(out, line.old);
        out.push(' ');
        number(out, line.new);
        out.push(' ');
        out.push(match line.kind {
            DiffLineKind::Added => '+',
            DiffLineKind::Removed => '-',
            DiffLineKind::Context | DiffLineKind::Hunk => ' ',
        });
    }

    fn gutter_cells(&self) -> u32 {
        self.digits.saturating_mul(2).saturating_add(3)
    }
}

/// Reads the diff of the file at `paths` (a rename's old path first) in `commit`, against
/// `parent`, and measures it; `cancel` stops git.
pub(crate) async fn read_diff(
    git: Git,
    repository: Arc<Repository>,
    commit: ObjectId,
    parent: Option<ObjectId>,
    paths: Vec<BString>,
    options: DiffOptions,
    cancel: CancelToken,
) -> Result<Arc<DiffDocument>, Problem> {
    jobs::blocking(move || {
        let paths: Vec<&bstr::BStr> = paths.iter().map(AsRef::as_ref).collect();
        file_diff(&git, &repository, commit, parent, &paths, &options, &cancel)
            .map(|diff| Arc::new(DiffDocument::new(diff)))
            .map_err(|error| Problem::git("The diff couldn't be read", &error))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: DiffLineKind, old: u32, new: u32) -> DiffLine {
        DiffLine {
            kind,
            missing_newline: false,
            old,
            new,
        }
    }

    fn document() -> DiffDocument {
        DiffDocument::new(FileDiff {
            text: b"@@ -9,2 +9,3 @@\nkeep\nold\nnew\nmore\n".to_vec(),
            lines: vec![
                line(DiffLineKind::Hunk, 0, 0),
                line(DiffLineKind::Context, 9, 9),
                line(DiffLineKind::Removed, 10, 0),
                line(DiffLineKind::Added, 0, 10),
                line(DiffLineKind::Added, 0, 11),
            ],
            binary: false,
        })
    }

    #[test]
    fn numbers_both_sides_in_the_gutter() {
        let document = document();
        assert_eq!(document.gutter_cells(), 7);
        let gutters: Vec<String> = (0..5)
            .map(|line| {
                let mut out = String::new();
                document.gutter(line, &mut out);
                out
            })
            .collect();
        assert_eq!(
            gutters,
            ["       ", " 9  9  ", "10    -", "   10 +", "   11 +"]
        );
        assert_eq!(document.changed(), (2, 1));
    }

    #[test]
    fn lines_take_the_background_of_what_they_are() {
        let document = document();
        let palette = Palette::DARK;
        let backgrounds: Vec<Option<Color32>> = (0..5)
            .map(|line| document.background(line, &palette))
            .collect();
        assert_eq!(
            backgrounds,
            [
                Some(palette.hunk),
                None,
                Some(palette.removed),
                Some(palette.added),
                Some(palette.added)
            ]
        );
    }
}
