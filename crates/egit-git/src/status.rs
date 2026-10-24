//! The work tree's and the index's state: what `git status` reports, path by path.
//!
//! It is read with `git status --porcelain=v2 -z` (a format made for programs, unaffected by
//! the user's status settings but renames) and every untracked file is listed on its own. A tree
//! with a forgotten `node_modules` can list hundreds of thousands, so the output is kept as one
//! buffer and each entry is a few byte ranges of it; its modes and object names are checked
//! once, when read, and decoded again only when asked for.

use bstr::{BStr, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::history::TreeEntry;
use crate::object_id::ObjectId;
use crate::repo::Repository;
use crate::span::{Span, SpanSplitter, as_u32, span_bytes};

/// How one side of a path changed: the index against `HEAD` (staged), or the work tree against
/// the index (unstaged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Unchanged.
    Unmodified,
    /// Its contents changed.
    Modified,
    /// It became another kind of file (a file, a symbolic link, a submodule).
    TypeChanged,
    /// Added.
    Added,
    /// Deleted.
    Deleted,
    /// Renamed (staged only).
    Renamed,
    /// Copied (staged only).
    Copied,
    /// Unmerged: a conflict.
    Unmerged,
}

impl Change {
    /// The change a status letter names (`.`, `M`, `T`, `A`, `D`, `R`, `C`, `U`).
    const fn from_letter(letter: u8) -> Option<Self> {
        Some(match letter {
            b'.' => Self::Unmodified,
            b'M' => Self::Modified,
            b'T' => Self::TypeChanged,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'U' => Self::Unmerged,
            _ => return None,
        })
    }
}

/// What kind of entry a path has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A tracked path, changed in the index, the work tree, or both.
    Changed,
    /// A path the index holds under a new name (a staged rename).
    Renamed,
    /// A path the index holds as a copy of another (a staged copy).
    Copied,
    /// A path in conflict, mid-merge (or rebase, cherry-pick, …).
    Unmerged,
    /// A file git doesn't track.
    Untracked,
}

/// How a conflicted path's sides differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Deleted on both sides.
    BothDeleted,
    /// Added on our side only.
    AddedByUs,
    /// Deleted on their side, changed on ours.
    DeletedByThem,
    /// Added on their side only.
    AddedByThem,
    /// Deleted on our side, changed on theirs.
    DeletedByUs,
    /// Added on both sides.
    BothAdded,
    /// Changed on both sides.
    BothModified,
}

/// What changed inside a submodule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubmoduleChanges {
    /// It is at another commit than the one recorded.
    pub commit: bool,
    /// Its tracked files changed.
    pub modified: bool,
    /// It has untracked files.
    pub untracked: bool,
}

/// The state of every path that isn't clean.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// git's output, which the entries point into.
    output: Box<[u8]>,
    entries: Box<[RawEntry]>,
}

/// An entry, as ranges of the output.
#[derive(Clone, Copy, Debug)]
struct RawEntry {
    kind: EntryKind,
    /// The fields before the path (`XY sub mH mI mW hH hI`, and more for some kinds; none for
    /// an untracked file).
    fields: Span,
    path: Span,
    /// A rename's or copy's old path (empty for the rest).
    from: Span,
}

/// One path's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusEntry<'a> {
    /// What kind of entry it is.
    pub kind: EntryKind,
    /// The path, from the top of the work tree.
    pub path: &'a BStr,
    /// A staged rename's or copy's old path.
    pub from: Option<&'a BStr>,
    fields: &'a [u8],
}

impl Status {
    /// Reads the repository's status. Renames are reported as the user's `status.renames`
    /// (or `diff.renames`) says.
    ///
    /// # Errors
    ///
    /// When git fails, is cancelled, or prints something that isn't a status.
    pub async fn read(
        git: &Git,
        repository: &Repository,
        cancel: &CancelToken,
    ) -> Result<Self, GitError> {
        let output = git
            .read("status")
            .args([
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--ignored=no",
            ])
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output()
            .await?;
        Self::parse(output.stdout)
    }

    /// Parses `--porcelain=v2 -z` output.
    fn parse(output: Vec<u8>) -> Result<Self, GitError> {
        if u32::try_from(output.len()).is_err() {
            return Err(GitError::unexpected("status", "a status over 4 GiB"));
        }
        let mut entries = Vec::new();
        let mut records = SpanSplitter::new(&output);
        while let Some(record) = records.next() {
            let bytes = span_bytes(&output, record);
            let (kind, fields_count) = match bytes.first() {
                Some(b'#') => continue,
                Some(b'1') => (EntryKind::Changed, 7),
                Some(b'2') => (EntryKind::Renamed, 8),
                Some(b'u') => (EntryKind::Unmerged, 9),
                Some(b'?') => (EntryKind::Untracked, 0),
                _ => return Err(malformed(bytes)),
            };
            // After the kind and a space: the fields, a space, then the path (which may hold
            // spaces itself).
            let rest = record.start.saturating_add(2);
            let mut path_start = rest;
            for _ in 0..fields_count {
                let after = span_bytes(
                    &output,
                    Span {
                        start: path_start,
                        end: record.end,
                    },
                );
                let space = after.find_byte(b' ').ok_or_else(|| malformed(bytes))?;
                path_start = path_start.saturating_add(as_u32(space)).saturating_add(1);
            }
            let fields = Span {
                start: rest,
                end: path_start.saturating_sub(1).max(rest),
            };
            let path = Span {
                start: path_start,
                end: record.end,
            };
            let (kind, from) = if kind == EntryKind::Renamed {
                // `R100` or `C75`: the last field says which; the old path is the next record.
                let score = span_bytes(&output, fields).rsplit_str(b" ").next();
                let from = records.next().ok_or_else(|| malformed(bytes))?;
                if score.and_then(<[u8]>::first) == Some(&b'C') {
                    (EntryKind::Copied, from)
                } else {
                    (kind, from)
                }
            } else {
                (kind, Span::default())
            };
            let entry = RawEntry {
                kind,
                fields,
                path,
                from,
            };
            if path.start >= path.end
                || (matches!(kind, EntryKind::Renamed | EntryKind::Copied)
                    && from.start >= from.end)
                || !valid_fields(kind, span_bytes(&output, fields))
            {
                return Err(malformed(bytes));
            }
            entries.push(entry);
        }
        Ok(Self {
            output: output.into_boxed_slice(),
            entries: entries.into_boxed_slice(),
        })
    }

    /// How many paths aren't clean.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether every path is clean.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entry `index`, in git's order (by path).
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<StatusEntry<'_>> {
        self.entries.get(index).map(|raw| self.entry_at(raw))
    }

    /// Every entry, in git's order (by path).
    pub fn entries(&self) -> impl ExactSizeIterator<Item = StatusEntry<'_>> {
        self.entries.iter().map(|raw| self.entry_at(raw))
    }

    fn entry_at<'s>(&'s self, raw: &RawEntry) -> StatusEntry<'s> {
        StatusEntry {
            kind: raw.kind,
            path: BStr::new(span_bytes(&self.output, raw.path)),
            from: (raw.from.start < raw.from.end)
                .then(|| BStr::new(span_bytes(&self.output, raw.from))),
            fields: span_bytes(&self.output, raw.fields),
        }
    }
}

impl<'a> StatusEntry<'a> {
    /// How the index differs from `HEAD` ([`Change::Unmodified`] for an untracked file).
    #[must_use]
    pub fn staged(self) -> Change {
        self.letters()
            .and_then(|[staged, _]| Change::from_letter(staged))
            .unwrap_or(Change::Unmodified)
    }

    /// How the work tree differs from the index ([`Change::Added`] for an untracked file).
    #[must_use]
    pub fn unstaged(self) -> Change {
        if self.kind == EntryKind::Untracked {
            return Change::Added;
        }
        self.letters()
            .and_then(|[_, unstaged]| Change::from_letter(unstaged))
            .unwrap_or(Change::Unmodified)
    }

    /// Whether the index holds a change to commit.
    #[must_use]
    pub fn is_staged(self) -> bool {
        !matches!(self.kind, EntryKind::Unmerged | EntryKind::Untracked)
            && self.staged() != Change::Unmodified
    }

    /// Whether the work tree holds a change not in the index (a conflict, or an untracked
    /// file, counts).
    #[must_use]
    pub fn is_unstaged(self) -> bool {
        matches!(self.kind, EntryKind::Unmerged | EntryKind::Untracked)
            || self.unstaged() != Change::Unmodified
    }

    /// What `HEAD` holds at the path (at a rename's old path); `None` when nothing.
    #[must_use]
    pub fn head(self) -> Option<TreeEntry> {
        self.tree_entry(2, 5)
    }

    /// What the index holds at the path; `None` when nothing (or for a conflict, whose index
    /// holds each side apart).
    #[must_use]
    pub fn index(self) -> Option<TreeEntry> {
        self.tree_entry(3, 6)
    }

    /// The file's mode in the work tree; `None` when it is deleted there.
    #[must_use]
    pub fn worktree_mode(self) -> Option<u32> {
        let at = match self.kind {
            EntryKind::Changed | EntryKind::Renamed | EntryKind::Copied => 4,
            EntryKind::Unmerged => 5,
            EntryKind::Untracked => return None,
        };
        mode(self.field(at)?).filter(|&mode| mode != 0)
    }

    /// How alike a rename's or copy's paths are, in percent.
    #[must_use]
    pub fn similarity(self) -> Option<u8> {
        if !matches!(self.kind, EntryKind::Renamed | EntryKind::Copied) {
            return None;
        }
        self.field(7)?.get(1..)?.to_str().ok()?.parse().ok()
    }

    /// How a conflicted path's sides differ.
    #[must_use]
    pub fn conflict(self) -> Option<Conflict> {
        if self.kind != EntryKind::Unmerged {
            return None;
        }
        Some(match self.letters()? {
            [b'D', b'D'] => Conflict::BothDeleted,
            [b'A', b'U'] => Conflict::AddedByUs,
            [b'U', b'D'] => Conflict::DeletedByThem,
            [b'U', b'A'] => Conflict::AddedByThem,
            [b'D', b'U'] => Conflict::DeletedByUs,
            [b'A', b'A'] => Conflict::BothAdded,
            _ => Conflict::BothModified,
        })
    }

    /// What changed inside it, when it is a submodule.
    #[must_use]
    pub fn submodule(self) -> Option<SubmoduleChanges> {
        let field = self.field(1)?;
        let [kind, commit, modified, untracked] = <[u8; 4]>::try_from(field).ok()?;
        (kind == b'S').then_some(SubmoduleChanges {
            commit: commit == b'C',
            modified: modified == b'M',
            untracked: untracked == b'U',
        })
    }

    /// The `XY` letters (none for an untracked file).
    fn letters(self) -> Option<[u8; 2]> {
        <[u8; 2]>::try_from(self.field(0)?).ok()
    }

    /// Field `index` of those before the path.
    fn field(self, index: usize) -> Option<&'a [u8]> {
        if self.fields.is_empty() {
            return None;
        }
        self.fields.split_str(b" ").nth(index)
    }

    /// The tree entry the mode in field `mode` and the object in field `id` make; `None` for
    /// mode zero (nothing there) and for kinds without them.
    fn tree_entry(self, mode_at: usize, id_at: usize) -> Option<TreeEntry> {
        if !matches!(
            self.kind,
            EntryKind::Changed | EntryKind::Renamed | EntryKind::Copied
        ) {
            return None;
        }
        let mode = mode(self.field(mode_at)?).filter(|&mode| mode != 0)?;
        let id = ObjectId::from_hex(self.field(id_at)?)?;
        Some(TreeEntry { mode, id })
    }
}

/// An octal mode.
fn mode(field: &[u8]) -> Option<u32> {
    u32::from_str_radix(field.to_str().ok()?, 8).ok()
}

/// Whether `fields` are what an entry of `kind` has before its path.
fn valid_fields(kind: EntryKind, fields: &[u8]) -> bool {
    let fields: Vec<&[u8]> = if fields.is_empty() {
        Vec::new()
    } else {
        fields.split_str(b" ").collect()
    };
    let letters = |field: &[u8]| {
        field.len() == 2
            && field
                .iter()
                .all(|&letter| Change::from_letter(letter).is_some())
    };
    let submodule = |field: &[u8]| field.len() == 4 && matches!(field.first(), Some(b'N' | b'S'));
    let modes = |range: std::ops::Range<usize>| {
        fields
            .get(range)
            .is_some_and(|modes| modes.iter().all(|field| mode(field).is_some()))
    };
    let ids = |range: std::ops::Range<usize>| {
        fields
            .get(range)
            .is_some_and(|ids| ids.iter().all(|field| ObjectId::from_hex(field).is_some()))
    };
    match (kind, fields.as_slice()) {
        (EntryKind::Untracked, []) => true,
        (EntryKind::Changed, [xy, sub, ..]) if fields.len() == 7 => {
            letters(xy) && submodule(sub) && modes(2..5) && ids(5..7)
        }
        (EntryKind::Renamed | EntryKind::Copied, [xy, sub, .., score]) if fields.len() == 8 => {
            letters(xy)
                && submodule(sub)
                && modes(2..5)
                && ids(5..7)
                && matches!(score.first(), Some(b'R' | b'C'))
                && score
                    .get(1..)
                    .and_then(|score| score.to_str().ok()?.parse::<u8>().ok())
                    .is_some()
        }
        (EntryKind::Unmerged, [xy, sub, ..]) if fields.len() == 9 => {
            letters(xy) && submodule(sub) && modes(2..6) && ids(6..9)
        }
        _ => false,
    }
}

/// The error for a record that isn't a status entry.
fn malformed(record: &[u8]) -> GitError {
    GitError::unexpected(
        "status",
        format!("{} isn't a status entry", BStr::new(record)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
    const B: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn status(output: &str) -> Status {
        Status::parse(output.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn reads_every_kind_of_entry() {
        let status = status(&format!(
            "1 .M N... 100644 100644 100644 {A} {A} src/lib.rs\0\
             1 M. N... 100644 100755 100755 {A} {B} run me.sh\0\
             1 D. N... 100644 000000 000000 {A} {ZERO} gone\0\
             1 A. N... 000000 100644 100644 {ZERO} {B} new\0\
             2 R. N... 100644 100644 100644 {A} {A} R100 new name\0old name\0\
             2 C. N... 100644 100644 100644 {A} {A} C75 copy\0original\0\
             u UU N... 100644 100644 100644 100644 {A} {B} {A} both\0\
             1 .M SC.U 160000 160000 160000 {A} {A} lib/sub\0\
             ? notes/todo.txt\0"
        ));
        assert_eq!(status.len(), 9);
        let entry = |index| status.entry(index).unwrap();

        let modified = entry(0);
        assert_eq!(modified.kind, EntryKind::Changed);
        assert_eq!(modified.path, "src/lib.rs");
        assert_eq!(
            (modified.staged(), modified.unstaged()),
            (Change::Unmodified, Change::Modified)
        );
        assert!(modified.is_unstaged() && !modified.is_staged());
        assert_eq!(modified.index().unwrap().id.to_string(), A);

        let executable = entry(1);
        assert_eq!(executable.path, "run me.sh");
        assert!(executable.is_staged() && !executable.is_unstaged());
        assert_eq!(executable.index().unwrap().mode, 0o100_755);
        assert_eq!(executable.head().unwrap().mode, 0o100_644);

        let deleted = entry(2);
        assert_eq!(deleted.staged(), Change::Deleted);
        assert_eq!((deleted.index(), deleted.worktree_mode()), (None, None));
        assert_eq!(entry(3).head(), None);

        let renamed = entry(4);
        assert_eq!(renamed.kind, EntryKind::Renamed);
        assert_eq!(
            (renamed.path, renamed.from),
            (BStr::new("new name"), Some(BStr::new("old name")))
        );
        assert_eq!(renamed.similarity(), Some(100));
        assert_eq!(entry(5).kind, EntryKind::Copied);
        assert_eq!(entry(5).similarity(), Some(75));

        let conflict = entry(6);
        assert_eq!(conflict.conflict(), Some(Conflict::BothModified));
        assert!(conflict.is_unstaged() && !conflict.is_staged());
        assert_eq!(conflict.index(), None);
        assert_eq!(conflict.worktree_mode(), Some(0o100_644));

        assert_eq!(
            entry(7).submodule(),
            Some(SubmoduleChanges {
                commit: true,
                modified: false,
                untracked: true
            })
        );
        assert_eq!(entry(0).submodule(), None);

        let untracked = entry(8);
        assert_eq!(untracked.kind, EntryKind::Untracked);
        assert_eq!(untracked.path, "notes/todo.txt");
        assert_eq!(untracked.unstaged(), Change::Added);
        assert!(untracked.is_unstaged());
    }

    #[test]
    fn a_clean_tree_has_no_entries() {
        assert!(status("").is_empty());
        assert!(status("# stash 2\0").is_empty());
    }

    #[test]
    fn refuses_what_isnt_a_status() {
        let refused = |output: String| Status::parse(output.into_bytes()).is_err();
        assert!(refused(format!("1 .M N... 100644 100644 100644 {A} {A}\0")));
        assert!(refused(format!(
            "1 .X N... 100644 100644 100644 {A} {A} p\0"
        )));
        assert!(refused(format!(
            "1 .M N... 1006x4 100644 100644 {A} {A} p\0"
        )));
        assert!(refused(format!(
            "1 .M N... 100644 100644 100644 {A} short p\0"
        )));
        assert!(refused(format!(
            "2 R. N... 100644 100644 100644 {A} {A} R100 only-new\0"
        )));
        assert!(refused("? \0".to_owned()));
        assert!(refused("x what\0".to_owned()));
    }
}
