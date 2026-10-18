//! Discarding changes not staged: the work tree put back as the index has it, at whole files
//! (`git restore --worktree`) or at some of one file's changed lines, and untracked files
//! removed (`git clean`).
//!
//! Each file a discard changes is kept before and after it as a blob of its bytes as they are
//! (`hash-object -w --no-filters`: never through filters), so [`put_back`] can write either side
//! again: the discard undone, then done again. Nothing refers to those blobs; git keeps
//! unreachable objects two weeks (`gc.pruneExpire`), far longer than an undo lasts. git has no
//! command writing a blob's bytes to a path as they are, so [`put_back`] writes them here, read
//! through `cat-file`, by a rename over the file. A discard of many files, or of a large one,
//! keeps nothing ([`KEEP_FILES`], [`KEEP_BYTES`]): writing them all into the object store would
//! cost more than an undo is worth.
//!
//! Some of a file's lines are discarded as staging does the opposite ([`apply_lines`]): the
//! index's content with every change of the diff applied but those picked, written through the
//! file's filters (`cat-file --filters`, as git checks a file out), once the index holds what
//! the diff was read against and the work tree's file is still what it showed (its blob as git
//! would store it, against the diff's `index` line).

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::path::Path;

use bstr::{BStr, BString, ByteSlice as _, ByteVec as _};

use crate::cancel::CancelToken;
use crate::diff::FileDiff;
use crate::error::GitError;
use crate::git::Git;
use crate::history::TreeEntry;
use crate::object_id::ObjectId;
use crate::objects::ObjectReader;
use crate::patch::{Direction, LinesError, apply_lines, index_entry_at};
use crate::repo::Repository;

/// The most files a discard keeps to put back: past it, it keeps none.
pub const KEEP_FILES: usize = 1_000;

/// The largest file a discard keeps: one larger leaves it keeping none.
pub const KEEP_BYTES: u64 = 64 << 20;

/// A file's mode, as git records it: a regular file, an executable one, a symbolic link (its
/// bytes its target), a submodule.
const REGULAR: u32 = 0o100_644;
const EXECUTABLE: u32 = 0o100_755;
const LINK: u32 = 0o120_000;
const SUBMODULE: u32 = 0o160_000;

/// The most bytes of paths one `git clean` names, so its command line stays short (Windows
/// allows 32,767 characters).
const CLEAN_BYTES: usize = 16 * 1024;

/// The largest blob read back (files the diff viewer shows are 256 MiB at most).
const BLOB_LIMIT: usize = 1 << 30;

/// A work tree's file, kept: a blob of its bytes as they are, and its mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeptFile {
    /// The blob of its bytes (a symbolic link's: its target).
    pub id: ObjectId,
    /// `0o100644`, `0o100755` (executable), or `0o120000` (a symbolic link).
    pub mode: u32,
}

/// A file a discard changed: as it was, and as the discard left it (`None`: no file there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscardedFile {
    /// Its path, from the work tree's top.
    pub path: BString,
    /// The file before the discard.
    pub before: Option<KeptFile>,
    /// The file the discard left.
    pub after: Option<KeptFile>,
}

/// Which side of a discard [`put_back`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscardSide {
    /// The files as they were: the discard taken back.
    Before,
    /// The files as the discard left them: it done again.
    After,
}

/// Discarding (or putting back) failed.
#[derive(Debug, thiserror::Error)]
pub enum DiscardError {
    /// A file isn't what it was read as (its diff, or what a discard left): nothing was
    /// written.
    #[error("{0} changed since")]
    Stale(BString),
    /// Lines of a symbolic link or a submodule: only a file's can be discarded.
    #[error("{0} isn't a file of lines")]
    NotLines(BString),
    /// Running git failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The work tree couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file, from the work tree's top.
        path: BString,
        /// What the system said.
        source: io::Error,
    },
}

/// Discards the changes not staged of `tracked`, and removes `untracked`.
///
/// Each tracked file goes back to what the index holds. Returns each file as it was and as it
/// is now, kept, or `None` when they were too many or one was too large (or a folder) to keep.
///
/// # Errors
///
/// When git fails (a path in conflict, say) or is cancelled, or a file can't be read to keep.
pub fn discard_files(
    git: &Git,
    repository: &Repository,
    tracked: &[&BStr],
    untracked: &[&BStr],
    cancel: &CancelToken,
) -> Result<Option<Vec<DiscardedFile>>, DiscardError> {
    let paths: Vec<&BStr> = tracked.iter().chain(untracked).copied().collect();
    let before = keep(
        git,
        repository,
        &paths,
        Keeping::Store { limit: true },
        cancel,
    )?;
    crate::stage::with_paths(git, repository, "restore", &["--worktree"], tracked, cancel)?;
    remove_untracked(git, repository, untracked, cancel)?;
    let Some(before) = before else {
        return Ok(None);
    };
    let Some(after) = keep(
        git,
        repository,
        &paths,
        Keeping::Store { limit: false },
        cancel,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(
        paths
            .iter()
            .zip(before)
            .zip(after)
            .map(|((path, before), after)| DiscardedFile {
                path: (*path).to_owned(),
                before,
                after,
            })
            .collect(),
    ))
}

/// Discards the lines of `diff` that `picked` chooses (by their index in `diff.lines`).
///
/// `diff` is the index against the work tree at `path`; the work tree's file then holds what
/// the index does there, and every change not picked. `expected` is what the index held at
/// `path` when the diff was read (`None`: the file isn't tracked). Returns the file as it was
/// and as it is now, kept.
///
/// # Errors
///
/// [`DiscardError::Stale`] when the index or the file changed since the diff was read (nothing
/// is written then), [`DiscardError::NotLines`] for a symbolic link or a submodule, and
/// otherwise when git fails or the file can't be written.
pub fn discard_lines(
    git: &Git,
    repository: &Repository,
    (path, diff): (&BStr, &FileDiff),
    picked: impl Fn(usize) -> bool,
    expected: Option<TreeEntry>,
    cancel: &CancelToken,
) -> Result<DiscardedFile, DiscardError> {
    let stale = || DiscardError::Stale(path.to_owned());
    let lines = |error| match error {
        LinesError::Stale => stale(),
        LinesError::Git(error) => DiscardError::Git(error),
    };
    if expected.is_some_and(|entry| matches!(entry.mode, LINK | SUBMODULE)) {
        return Err(DiscardError::NotLines(path.to_owned()));
    }
    if index_entry_at(git, repository, path, cancel).map_err(lines)? != expected {
        return Err(stale());
    }
    let work_tree = repository.work_tree();
    let full = work_tree.join(path.to_path_lossy());
    let found = look(&full).map_err(|source| io_error(path, source))?;
    let mode = match found {
        Found::File { executable, .. } => mode_of(executable),
        Found::Nothing => expected.map_or(REGULAR, |entry| entry.mode),
        Found::Link(_) | Found::Folder => return Err(DiscardError::NotLines(path.to_owned())),
    };
    // The file must still be what the diff showed (as git would store it), or be gone with it.
    let (_, shown) = diff.blobs.ok_or_else(stale)?;
    let now = match found {
        Found::Nothing => None,
        _ => Some(stored_blob(git, repository, path, cancel)?),
    };
    let gone = shown == shown.format().zero();
    if (gone && now.is_some()) || (!gone && now != Some(shown)) {
        return Err(stale());
    }
    let base = match expected {
        Some(entry) => blob(git, work_tree, entry.id, cancel)?,
        None => Vec::new(),
    };
    let content =
        apply_lines(&base, diff, Direction::Stage, |index| !picked(index)).map_err(lines)?;
    let before = keep(
        git,
        repository,
        &[path],
        Keeping::Store { limit: false },
        cancel,
    )?
    .and_then(|kept| kept.into_iter().next().flatten());
    let stored = hash_bytes(git, work_tree, content, true, cancel)?;
    let mut filters = OsString::from("--path=");
    filters.push(path.to_os_str_lossy());
    let written = git
        .read("cat-file")
        .arg("--filters")
        .arg(filters)
        .arg(stored.to_string())
        .current_dir(work_tree)
        .stdout_limit(BLOB_LIMIT)
        .cancel_token(cancel)
        .output()?
        .stdout;
    write_file(&full, &written, mode).map_err(|source| io_error(path, source))?;
    let after = KeptFile {
        id: hash_bytes(git, work_tree, written, true, cancel)?,
        mode,
    };
    Ok(DiscardedFile {
        path: path.to_owned(),
        before,
        after: Some(after),
    })
}

/// Writes `side` of `files` (each discarded together) back into the work tree, once every one
/// is as the other side left it.
///
/// # Errors
///
/// [`DiscardError::Stale`] when one isn't (nothing is written then), and otherwise when git
/// fails or a file can't be written.
pub fn put_back(
    git: &Git,
    repository: &Repository,
    files: &[DiscardedFile],
    side: DiscardSide,
    cancel: &CancelToken,
) -> Result<(), DiscardError> {
    // Each file as it must be now, and as it is to be.
    let sides = |file: &DiscardedFile| match side {
        DiscardSide::Before => (file.after, file.before),
        DiscardSide::After => (file.before, file.after),
    };
    let paths: Vec<&BStr> = files.iter().map(|file| file.path.as_bstr()).collect();
    let stale = |file: Option<&DiscardedFile>| {
        DiscardError::Stale(file.map(|file| file.path.clone()).unwrap_or_default())
    };
    let now = keep(git, repository, &paths, Keeping::Hash, cancel)?
        .ok_or_else(|| stale(files.first()))?;
    if let Some((file, _)) = files
        .iter()
        .zip(&now)
        .find(|(file, now)| !same(sides(file).0, **now))
    {
        return Err(stale(Some(file)));
    }
    let work_tree = repository.work_tree();
    for file in files.iter().filter(|file| sides(file).1.is_none()) {
        remove_file(&work_tree.join(file.path.to_path_lossy()))
            .map_err(|source| io_error(file.path.as_bstr(), source))?;
    }
    let mut writing = files.iter().filter_map(|file| Some((file, sides(file).1?)));
    let ids: Vec<ObjectId> = writing.clone().map(|(_, kept)| kept.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let mut failed = None;
    ObjectReader::start(git, work_tree)?.read(&ids, |_, object| {
        let Some((file, kept)) = writing.next() else {
            return;
        };
        let written = object.map_or_else(
            || {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "its kept copy is gone",
                ))
            },
            |object| {
                write_file(
                    &work_tree.join(file.path.to_path_lossy()),
                    object.data,
                    kept.mode,
                )
            },
        );
        if let Err(source) = written {
            failed.get_or_insert_with(|| io_error(file.path.as_bstr(), source));
        }
    })?;
    failed.map_or(Ok(()), Err)
}

/// Whether two keepings are of the same file: its bytes, and a link or not (an executable bit
/// changed since is put back, never a reason to refuse).
fn same(one: Option<KeptFile>, other: Option<KeptFile>) -> bool {
    match (one, other) {
        (None, None) => true,
        (Some(one), Some(other)) => {
            one.id == other.id && (one.mode == LINK) == (other.mode == LINK)
        }
        _ => false,
    }
}

/// How [`keep`] keeps.
#[derive(Clone, Copy, Debug)]
enum Keeping {
    /// Only the blobs' names, to compare.
    Hash,
    /// Blobs written; with `limit`, none when there are too many files, or one too large.
    Store { limit: bool },
}

/// What is at a path in the work tree.
enum Found {
    Nothing,
    /// A symbolic link, and its target.
    Link(Vec<u8>),
    /// A file: its size, and whether it is executable.
    File {
        size: u64,
        executable: bool,
    },
    /// A folder (an untracked repository's, say): never kept.
    Folder,
}

/// What is at `full`.
fn look(full: &Path) -> io::Result<Found> {
    let metadata = match std::fs::symlink_metadata(full) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Found::Nothing),
        Err(error) => return Err(error),
    };
    if metadata.is_symlink() {
        let target = std::fs::read_link(full)?;
        return Ok(Found::Link(Vec::from_path_lossy(&target).into_owned()));
    }
    if metadata.is_dir() {
        return Ok(Found::Folder);
    }
    Ok(Found::File {
        size: metadata.len(),
        executable: is_executable(&metadata),
    })
}

/// Each of `paths` (from the work tree's top) as it is now, kept as `keeping` says: `None`
/// where nothing is; all `None` when one is a folder, or (with a limit) there are too many or
/// one is too large.
fn keep(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    keeping: Keeping,
    cancel: &CancelToken,
) -> Result<Option<Vec<Option<KeptFile>>>, DiscardError> {
    let (store, limit) = match keeping {
        Keeping::Hash => (false, false),
        Keeping::Store { limit } => (true, limit),
    };
    if limit && paths.len() > KEEP_FILES {
        return Ok(None);
    }
    let work_tree = repository.work_tree();
    let mut kept = vec![None; paths.len()];
    // Read by one `hash-object` together, but those it can't take one per line.
    let mut listed: Vec<(usize, &BStr, u32)> = Vec::new();
    for ((index, path), slot) in paths.iter().enumerate().zip(kept.iter_mut()) {
        let full = work_tree.join(path.to_path_lossy());
        let found = look(&full).map_err(|source| io_error(path, source))?;
        match found {
            Found::Nothing => {}
            Found::Folder => return Ok(None),
            Found::File { size, .. } if limit && size > KEEP_BYTES => return Ok(None),
            Found::Link(target) => {
                let id = hash_bytes(git, work_tree, target, store, cancel)?;
                *slot = Some(KeptFile { id, mode: LINK });
            }
            Found::File { executable, .. } => {
                let mode = mode_of(executable);
                if path.contains(&b'\n') || path.starts_with(b"\"") {
                    let bytes = std::fs::read(&full).map_err(|source| io_error(path, source))?;
                    let id = hash_bytes(git, work_tree, bytes, store, cancel)?;
                    *slot = Some(KeptFile { id, mode });
                } else {
                    listed.push((index, *path, mode));
                }
            }
        }
    }
    let ids = hash_paths(
        git,
        work_tree,
        listed.iter().map(|(_, path, _)| *path),
        store,
        cancel,
    )?;
    if ids.len() != listed.len() {
        return Err(GitError::unexpected("hash-object", "a name for each file").into());
    }
    for ((index, _, mode), id) in listed.into_iter().zip(ids) {
        if let Some(slot) = kept.get_mut(index) {
            *slot = Some(KeptFile { id, mode });
        }
    }
    Ok(Some(kept))
}

/// The blobs of the files at `paths` as they are (`--stdin-paths`, one a line; written into
/// the object store when `store`).
fn hash_paths<'p>(
    git: &Git,
    work_tree: &Path,
    paths: impl Iterator<Item = &'p BStr>,
    store: bool,
    cancel: &CancelToken,
) -> Result<Vec<ObjectId>, GitError> {
    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path);
        stdin.push(b'\n');
    }
    if stdin.is_empty() {
        return Ok(Vec::new());
    }
    let command = if store {
        git.write("hash-object").arg("-w")
    } else {
        git.read("hash-object")
    };
    let hashed = command
        .args(["--no-filters", "--stdin-paths"])
        .current_dir(work_tree)
        .stdin(stdin)
        .cancel_token(cancel)
        .output()?;
    hashed
        .stdout
        .lines()
        .map(|line| {
            ObjectId::from_hex(line)
                .ok_or_else(|| GitError::unexpected("hash-object", "an object name"))
        })
        .collect()
}

/// The blob of `bytes` as they are (written into the object store when `store`).
fn hash_bytes(
    git: &Git,
    work_tree: &Path,
    bytes: Vec<u8>,
    store: bool,
    cancel: &CancelToken,
) -> Result<ObjectId, GitError> {
    let command = if store {
        git.write("hash-object").arg("-w")
    } else {
        git.read("hash-object")
    };
    let hashed = command
        .args(["--no-filters", "--stdin"])
        .current_dir(work_tree)
        .stdin(bytes)
        .cancel_token(cancel)
        .output()?;
    ObjectId::from_hex(hashed.stdout.trim_ascii())
        .ok_or_else(|| GitError::unexpected("hash-object", "an object name"))
}

/// The blob the work tree's file at `path` would be stored as: through its clean filters, as
/// `git add` makes it (and as a diff names its side).
fn stored_blob(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    cancel: &CancelToken,
) -> Result<ObjectId, GitError> {
    let hashed = git
        .read("hash-object")
        .arg("--")
        .arg(path.to_os_str_lossy())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    ObjectId::from_hex(hashed.stdout.trim_ascii())
        .ok_or_else(|| GitError::unexpected("hash-object", "an object name"))
}

/// A blob's bytes.
fn blob(
    git: &Git,
    work_tree: &Path,
    id: ObjectId,
    cancel: &CancelToken,
) -> Result<Vec<u8>, GitError> {
    Ok(git
        .read("cat-file")
        .arg("blob")
        .arg(id.to_string())
        .current_dir(work_tree)
        .stdout_limit(BLOB_LIMIT)
        .cancel_token(cancel)
        .output()?
        .stdout)
}

/// Removes the untracked files at `paths` (`git clean`: it removes nothing tracked, or
/// ignored), a few at a time.
fn remove_untracked(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let clean = |chunk: &[&BStr]| {
        git.write("clean")
            .args(["--force", "--quiet", "--"])
            .args(chunk.iter().map(|path| path.to_os_str_lossy()))
            // Paths are paths, never patterns.
            .env("GIT_LITERAL_PATHSPECS", "1")
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output()
            .map(drop)
    };
    let (mut start, mut bytes) = (0, 0_usize);
    for (index, path) in paths.iter().enumerate() {
        if index > start && bytes.saturating_add(path.len()) > CLEAN_BYTES {
            clean(paths.get(start..index).unwrap_or_default())?;
            (start, bytes) = (index, 0);
        }
        bytes = bytes.saturating_add(path.len()).saturating_add(1);
    }
    match paths.get(start..) {
        Some(rest) if !rest.is_empty() => clean(rest),
        _ => Ok(()),
    }
}

/// The mode git records for a file, executable or not.
const fn mode_of(executable: bool) -> u32 {
    if executable { EXECUTABLE } else { REGULAR }
}

/// Puts `bytes` at `full` with `mode`: a file written beside it and renamed over it, so it is
/// never seen half written (its permissions those of the file it replaces, its executable bit
/// as `mode` says); a symbolic link where links are made, a file holding its target elsewhere
/// (as git makes one there).
fn write_file(full: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let folder = full.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(folder)?;
    // Elsewhere, a file holding its target, as git checks a link out without `core.symlinks`.
    #[cfg(unix)]
    {
        if mode == LINK {
            return make_link(full, bytes);
        }
    }
    let mut file = tempfile::Builder::new()
        .prefix(".egit-")
        .suffix(".tmp")
        .tempfile_in(folder)?;
    file.write_all(bytes)?;
    let replaced = std::fs::symlink_metadata(full)
        .ok()
        .filter(std::fs::Metadata::is_file);
    std::fs::set_permissions(
        file.path(),
        permissions(replaced.as_ref(), &file.as_file().metadata()?, mode),
    )?;
    file.persist(full).map_err(|error| error.error)?;
    Ok(())
}

/// Removes the file (or link) at `full`, if there is one.
fn remove_file(full: &Path) -> io::Result<()> {
    match std::fs::remove_file(full) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Makes `full` a symbolic link to `target`, replacing what is there.
#[cfg(unix)]
fn make_link(full: &Path, target: &[u8]) -> io::Result<()> {
    remove_file(full)?;
    std::os::unix::fs::symlink(target.to_path_lossy(), full)
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

/// No executable bit here: git records files as it finds them in the index.
#[cfg(not(unix))]
const fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// A file's permissions: those of the file it replaces (else as git makes a file), executable
/// where it can be read when `mode` says so, not executable otherwise.
#[cfg(unix)]
fn permissions(
    replaced: Option<&std::fs::Metadata>,
    _new: &std::fs::Metadata,
    mode: u32,
) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt as _;
    let bits = replaced.map_or(0o644, |metadata| metadata.permissions().mode() & 0o7777);
    let bits = if mode == EXECUTABLE {
        bits | ((bits & 0o444) >> 2)
    } else {
        bits & !0o111
    };
    std::fs::Permissions::from_mode(bits)
}

/// A file's permissions: read-only only if the file it replaces was.
#[cfg(not(unix))]
fn permissions(
    replaced: Option<&std::fs::Metadata>,
    new: &std::fs::Metadata,
    _mode: u32,
) -> std::fs::Permissions {
    replaced.map_or_else(|| new.permissions(), std::fs::Metadata::permissions)
}

/// An error of the work tree's, with its path.
fn io_error(path: &BStr, source: io::Error) -> DiscardError {
    DiscardError::Io {
        path: path.to_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(digit: &str, mode: u32) -> Option<KeptFile> {
        Some(KeptFile {
            id: ObjectId::from_hex(digit.repeat(40).as_bytes())?,
            mode,
        })
    }

    #[test]
    fn the_same_file_is_its_bytes_and_whether_it_is_a_link() {
        assert!(same(None, None));
        assert!(
            same(kept("1", REGULAR), kept("1", EXECUTABLE)),
            "a bit, put back"
        );
        assert!(!same(kept("1", REGULAR), kept("2", REGULAR)));
        assert!(!same(kept("1", REGULAR), kept("1", LINK)));
        assert!(!same(kept("1", REGULAR), None));
    }

    #[cfg(unix)]
    #[test]
    fn permissions_follow_the_file_replaced_and_the_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("f");
        std::fs::write(&file, "x").unwrap();
        let bits = |mode: u32, replaced: u32| {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(replaced)).unwrap();
            let metadata = std::fs::metadata(&file).unwrap();
            permissions(Some(&metadata), &metadata, mode).mode() & 0o777
        };
        assert_eq!(bits(EXECUTABLE, 0o640), 0o750);
        assert_eq!(bits(REGULAR, 0o755), 0o644);
        assert_eq!(bits(REGULAR, 0o600), 0o600);
    }
}
