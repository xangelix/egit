//! A file's diff on the text grid: its lines without their `+`, `-`, or ` `, each on the
//! background of what it is, numbered in the old and new file in the gutter.
//!
//! In a block of removed lines followed by added ones, the first removed line pairs with the
//! first added, and so on; the words that changed between a pair are marked. Pairs are found
//! when the diff is read; their words are compared the first time they are drawn, then kept
//! (comparing every pair of a large rewrite up front could take seconds).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use bstr::BString;
use egit_git::{
    CancelToken, DiffLine, DiffLineKind, DiffOptions, FileDiff, Git, ObjectId, Repository,
    file_diff,
};
use egit_text::{ChangedWords, Measure, Options, Text, changed_words};
use egui::Color32;
use parking_lot::Mutex;

use crate::grid::{GridSource, Styled};
use crate::jobs;
use crate::problem::Problem;
use crate::theme::Palette;

/// The most pairs whose changed words are kept; past it, they are compared again.
const KEPT_WORDS: usize = 4096;

/// A line without a partner.
const NO_PARTNER: u32 = u32::MAX;

/// A file's diff, measured, ready to draw.
#[derive(Debug)]
pub(crate) struct DiffDocument {
    text: Text,
    measure: Measure,
    lines: Vec<DiffLine>,
    /// Each line's partner: a removed line's added one and the other way round
    /// ([`NO_PARTNER`] for the rest).
    partners: Vec<u32>,
    /// The words that changed between pairs drawn so far, by the pair's removed line (`None`
    /// where the lines share too little to mark).
    words: Mutex<HashMap<u32, Option<ChangedWords>>>,
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
            partners: partners(&lines),
            words: Mutex::new(HashMap::new()),
            text,
            measure,
            lines,
            binary,
            digits: largest.max(1).ilog10().saturating_add(1),
        }
    }

    /// The words that changed in line `line`, if it is half of a pair (compared once).
    fn changed_in(&self, line: u32, out: &mut Vec<std::ops::Range<u32>>) {
        let Some(&partner) = usize::try_from(line)
            .ok()
            .and_then(|line| self.partners.get(line))
        else {
            return;
        };
        if partner == NO_PARTNER {
            return;
        }
        let removed_first = self
            .line(line)
            .is_some_and(|line| line.kind == DiffLineKind::Removed);
        let (old, new) = if removed_first {
            (line, partner)
        } else {
            (partner, line)
        };
        let mut words = self.words.lock();
        if words.len() >= KEPT_WORDS && !words.contains_key(&old) {
            words.clear();
        }
        let changed = words.entry(old).or_insert_with(|| {
            let text = |line| self.text.line(line).unwrap_or_default();
            changed_words(text(old), text(new))
        });
        if let Some(changed) = changed {
            out.extend_from_slice(if removed_first {
                &changed.old
            } else {
                &changed.new
            });
        }
        drop(words);
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

/// Pairs each block's removed lines with its added ones, in order.
fn partners(lines: &[DiffLine]) -> Vec<u32> {
    let mut partners = vec![NO_PARTNER; lines.len()];
    let kind = |index: usize| lines.get(index).map(|line| line.kind);
    let mut index = 0;
    while index < lines.len() {
        if kind(index) != Some(DiffLineKind::Removed) {
            index = index.saturating_add(1);
            continue;
        }
        let removed = index;
        while kind(index) == Some(DiffLineKind::Removed) {
            index = index.saturating_add(1);
        }
        let added = index;
        while kind(index) == Some(DiffLineKind::Added) {
            index = index.saturating_add(1);
        }
        let pairs = (added.saturating_sub(removed)).min(index.saturating_sub(added));
        for pair in 0..pairs {
            let (old, new) = (removed.saturating_add(pair), added.saturating_add(pair));
            let number = |index: usize| u32::try_from(index).unwrap_or(NO_PARTNER);
            if let Some(partner) = partners.get_mut(old) {
                *partner = number(new);
            }
            if let Some(partner) = partners.get_mut(new) {
                *partner = number(old);
            }
        }
    }
    partners
}

impl GridSource for DiffDocument {
    fn text(&self) -> &Text {
        &self.text
    }

    fn measure(&self) -> &Measure {
        &self.measure
    }

    fn styles(&self, line: u32, palette: &Palette, out: &mut Vec<Styled>) {
        let background = match self.line(line).map(|line| line.kind) {
            Some(DiffLineKind::Hunk) => {
                out.push(Styled {
                    bytes: 0..u32::MAX,
                    color: palette.muted,
                    background: None,
                });
                return;
            }
            Some(DiffLineKind::Added) => palette.added_word,
            Some(DiffLineKind::Removed) => palette.removed_word,
            Some(DiffLineKind::Context) | None => return,
        };
        let mut changed = Vec::new();
        self.changed_in(line, &mut changed);
        out.extend(changed.into_iter().map(|bytes| Styled {
            bytes,
            color: palette.text,
            background: Some(background),
        }));
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
            text: b"@@ -9,2 +9,3 @@\nkeep\nlet a = 1;\nlet a = 2;\nmore\n".to_vec(),
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
    fn removed_and_added_lines_pair_up_in_order() {
        let lines = [
            line(DiffLineKind::Hunk, 0, 0),
            line(DiffLineKind::Removed, 1, 0),
            line(DiffLineKind::Removed, 2, 0),
            line(DiffLineKind::Added, 0, 1),
            line(DiffLineKind::Context, 3, 2),
            line(DiffLineKind::Added, 0, 3),
            line(DiffLineKind::Removed, 4, 0),
            line(DiffLineKind::Added, 0, 4),
            line(DiffLineKind::Added, 0, 5),
        ];
        let none = NO_PARTNER;
        assert_eq!(partners(&lines), [none, 3, none, 1, none, none, 7, 6, none]);
    }

    #[test]
    fn a_pair_marks_its_changed_words() {
        let document = document();
        let palette = Palette::DARK;
        let mut styles = Vec::new();
        document.styles(2, &palette, &mut styles);
        assert_eq!(
            styles,
            [Styled {
                bytes: 8..9,
                color: palette.text,
                background: Some(palette.removed_word)
            }]
        );
        styles.clear();
        document.styles(3, &palette, &mut styles);
        assert_eq!(styles[0].bytes, 8..9);
        assert_eq!(styles[0].background, Some(palette.added_word));
        styles.clear();
        document.styles(4, &palette, &mut styles);
        assert_eq!(styles, [], "the second added line has no partner");
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
