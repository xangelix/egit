//! egit's in-process reads through gitoxide (`gix`): a repository's references, the commits a
//! walk of all of them starts from, and objects. Each gives what egit's git commands give
//! (checked against them in tests), and each was measured faster where egit spends its time
//! (`docs/ROADMAP.md`, phase 12); everything else stays git's, and every write.
//!
//! A repository gix wouldn't read as git does isn't opened here, so its callers fall back to
//! git: references kept in a reftable (gix reads files), and replaced objects (`refs/replace/`,
//! which `cat-file` reads in their place). A partial clone is read as far as it goes: an object
//! it hasn't may be its remote's, which git fetches when asked for it, so reading stops there
//! and leaves the rest to git.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use egit_git::{Object, ObjectId, ObjectKind, RefKind, Reference};
use gix::bstr::{BString, ByteSlice as _};

/// Why gix couldn't read what it was asked.
#[derive(Debug, thiserror::Error)]
#[error("gitoxide couldn't read the repository: {0}")]
pub struct GixError(String);

impl GixError {
    fn of(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

/// A repository opened for gix's reads; clones share it, between threads too (each read takes
/// a thread's own handle).
#[derive(Clone)]
pub struct GixRepository {
    repository: Arc<gix::ThreadSafeRepository>,
    /// A partial clone: an object it hasn't may be its remote's.
    partial: bool,
}

impl std::fmt::Debug for GixRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GixRepository")
            .field("git_dir", &self.repository.git_dir())
            .field("partial", &self.partial)
            .finish()
    }
}

impl GixRepository {
    /// Opens the repository whose work tree (or git directory) is `path`, unless gix wouldn't
    /// read it as git does: references in a reftable, or replaced objects. `None` leaves it to
    /// git.
    #[must_use]
    pub fn open(path: &Path) -> Option<Self> {
        let repository = gix::ThreadSafeRepository::open(path).ok()?;
        let local = repository.to_thread_local();
        let (reftable, partial) = {
            let config = local.config_snapshot();
            let reftable = config
                .string("extensions.refStorage")
                .is_some_and(|storage| !storage.eq_ignore_ascii_case(b"files"));
            let partial = config.string("extensions.partialClone").is_some()
                || local.remote_names().iter().any(|remote| {
                    config
                        .boolean(format!("remote.{remote}.promisor").as_str())
                        .unwrap_or(false)
                });
            (reftable, partial)
        };
        if reftable {
            return None;
        }
        // `cat-file` reads a replaced object's replacement in its place.
        let replaced = local
            .references()
            .ok()?
            .prefixed("refs/replace/")
            .ok()?
            .next()
            .is_some();
        (!replaced).then(|| Self {
            repository: Arc::new(repository),
            partial,
        })
    }

    /// The repository's references, as egit's `list_references` lists them: every one but
    /// the symbolic, with what it points at through any annotated tags (`for-each-ref`'s
    /// `%(*objectname)`, which peels a tag of a tag too).
    ///
    /// # Errors
    ///
    /// When gix can't read the references or an object they point at.
    pub fn references(&self) -> Result<Vec<Reference>, GixError> {
        let repository = self.repository.to_thread_local();
        let platform = repository.references().map_err(GixError::of)?;
        let mut references = Vec::new();
        for reference in platform.all().map_err(GixError::of)? {
            let reference = reference.map_err(GixError::of)?;
            let gix::refs::TargetRef::Object(object) = reference.target() else {
                continue;
            };
            let (target, _) = peel(&repository, object.to_owned(), reference.inner.peeled)?;
            let name = BString::from(reference.name().as_bstr().as_bytes());
            references.push(Reference {
                kind: kind_of(&name),
                name,
                target: id_of(target)?,
            });
        }
        Ok(references)
    }

    /// The commits `git rev-list --no-walk=unsorted --exclude=refs/stash --all` lists, in its
    /// order: every reference but `refs/stash` (by name), then `HEAD`, then the other
    /// worktrees' `HEAD`s, each peeled to its commit (one that isn't a commit is left out),
    /// each commit once.
    ///
    /// # Errors
    ///
    /// When gix can't read the references or an object they point at.
    pub fn tips(&self) -> Result<Vec<ObjectId>, GixError> {
        let repository = self.repository.to_thread_local();
        let mut tips: Vec<gix::ObjectId> = Vec::new();
        // Each commit once, in the order first met: refs by the hundred thousand often share
        // commits, and a list searched for each would take time in the square of their number.
        let mut seen: HashSet<gix::ObjectId> = HashSet::new();
        let mut add = |id: gix::ObjectId, packed: Option<gix::ObjectId>, tips: &mut Vec<_>| {
            let (peeled, kind) = peel(&repository, id, packed)?;
            if kind == Some(gix::object::Kind::Commit) && seen.insert(peeled) {
                tips.push(peeled);
            }
            Ok::<_, GixError>(())
        };
        let platform = repository.references().map_err(GixError::of)?;
        for reference in platform.all().map_err(GixError::of)? {
            let mut reference = reference.map_err(GixError::of)?;
            if reference.name().as_bstr() == "refs/stash" {
                continue;
            }
            // A symbolic one is followed; one leading nowhere names nothing.
            let Ok(id) = reference.follow_to_object() else {
                continue;
            };
            let packed = reference.inner.peeled;
            add(id.detach(), packed, &mut tips)?;
        }
        if let Ok(head) = repository.head_id() {
            add(head.detach(), None, &mut tips)?;
        }
        for worktree in repository.worktrees().map_err(GixError::of)? {
            let Ok(other) = worktree.into_repo_with_possibly_inaccessible_worktree() else {
                continue;
            };
            if let Ok(head) = other.head_id() {
                add(head.detach(), None, &mut tips)?;
            }
        }
        tips.into_iter().map(id_of).collect()
    }

    /// Reads `ids`, handing each object to `each` in order (`None` for one that isn't there),
    /// as egit's `ObjectReader::read` does; the contents are borrowed until the next call. Says
    /// how many it handed over: all of them, but in a partial clone, which it stops at the
    /// first object it hasn't (its remote's, perhaps), the rest are git's to read.
    ///
    /// # Errors
    ///
    /// When gix can't read an object that is there (a damaged pack, say).
    pub fn read(
        &self,
        ids: &[ObjectId],
        mut each: impl FnMut(ObjectId, Option<Object<'_>>),
    ) -> Result<usize, GixError> {
        let mut reader = self.reader();
        for (handed, &id) in ids.iter().enumerate() {
            match reader.read(&id)? {
                Some(object) => each(id, Some(object)),
                None if self.partial => return Ok(handed),
                None => each(id, None),
            }
        }
        Ok(ids.len())
    }

    /// A reader for one thread: a handle of its own, and a buffer it reuses.
    #[must_use]
    pub fn reader(&self) -> GixReader {
        GixReader {
            repository: self.repository.to_thread_local(),
            buffer: Vec::new(),
        }
    }
}

/// Reads objects on one thread (each thread makes its own: [`GixRepository::reader`]).
pub struct GixReader {
    repository: gix::Repository,
    buffer: Vec<u8>,
}

impl std::fmt::Debug for GixReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GixReader")
            .field("git_dir", &self.repository.git_dir())
            .finish_non_exhaustive()
    }
}

impl GixReader {
    /// Object `id`, borrowed until the next read; `None` when it isn't there (in a partial
    /// clone, perhaps only not fetched).
    ///
    /// # Errors
    ///
    /// When gix can't read an object that is there (a damaged pack, say).
    pub fn read(&mut self, id: &ObjectId) -> Result<Option<Object<'_>>, GixError> {
        use gix::prelude::Find as _;
        let wanted = gix::ObjectId::try_from(id.as_bytes()).map_err(GixError::of)?;
        let found = self
            .repository
            .objects
            .try_find(&wanted, &mut self.buffer)
            .map_err(GixError::of)?;
        Ok(found.map(|data| Object {
            kind: match data.kind {
                gix::object::Kind::Commit => ObjectKind::Commit,
                gix::object::Kind::Tree => ObjectKind::Tree,
                gix::object::Kind::Blob => ObjectKind::Blob,
                gix::object::Kind::Tag => ObjectKind::Tag,
            },
            data: data.data,
        }))
    }
}

/// What `id` names through every annotated tag, and its kind (`None` when it isn't here, and the
/// peeling stops at it, as git's does). `packed` is a packed reference's peeled id, which git
/// wrote fully peeled. Kinds come from objects' headers: only tags are decoded.
fn peel(
    repository: &gix::Repository,
    id: gix::ObjectId,
    packed: Option<gix::ObjectId>,
) -> Result<(gix::ObjectId, Option<gix::object::Kind>), GixError> {
    let mut id = packed.unwrap_or(id);
    loop {
        let header = repository.try_find_header(id).map_err(GixError::of)?;
        match header.map(|header| header.kind()) {
            Some(gix::object::Kind::Tag) => {
                let tag = repository.find_object(id).map_err(GixError::of)?;
                id = tag.into_tag().target_id().map_err(GixError::of)?.detach();
            }
            kind => return Ok((id, kind)),
        }
    }
}

/// A reference's kind, by its namespace.
fn kind_of(name: &[u8]) -> RefKind {
    if name.starts_with(b"refs/heads/") {
        RefKind::Branch
    } else if name.starts_with(b"refs/remotes/") {
        RefKind::RemoteBranch
    } else if name.starts_with(b"refs/tags/") {
        RefKind::Tag
    } else {
        RefKind::Other
    }
}

/// egit's name for an object gix names.
fn id_of(id: gix::ObjectId) -> Result<ObjectId, GixError> {
    ObjectId::from_bytes(id.as_bytes())
        .ok_or_else(|| GixError(format!("{id} isn't a name egit reads")))
}
