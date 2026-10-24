//! Reading objects through one long-lived `git cat-file --batch-command --buffer`, instead of a
//! git process per object.

use std::io::{self, Write as _};
use std::path::Path;

use bstr::{BStr, ByteSlice as _};

use crate::batch::GitProcess;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;

/// What kind of object an object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    /// A commit.
    Commit,
    /// A tree (a directory).
    Tree,
    /// A blob (a file's contents).
    Blob,
    /// An annotated tag.
    Tag,
}

impl ObjectKind {
    const fn parse(name: &[u8]) -> Option<Self> {
        match name {
            b"commit" => Some(Self::Commit),
            b"tree" => Some(Self::Tree),
            b"blob" => Some(Self::Blob),
            b"tag" => Some(Self::Tag),
            _ => None,
        }
    }
}

/// An object's contents, borrowed from the reader until the next one.
#[derive(Clone, Copy, Debug)]
pub struct Object<'a> {
    /// Its kind.
    pub kind: ObjectKind,
    /// Its contents, as git stores them.
    pub data: &'a [u8],
}

/// Reads objects from a repository through one running `git cat-file`.
#[derive(Debug)]
pub struct ObjectReader {
    process: GitProcess,
    request: Vec<u8>,
    header: Vec<u8>,
    content: Vec<u8>,
}

impl ObjectReader {
    /// How many requests are sent before their answers are read: few enough that the requests
    /// always fit in the pipe, so neither side can block the other.
    const BATCH: usize = 128;

    /// The largest buffer kept between objects: a larger object's is dropped once it was
    /// handed over, so one big file read doesn't stay allocated for the reader's life.
    const KEPT_BUFFER: usize = 1 << 20;

    /// Starts reading objects from the repository whose work tree is `work_tree`.
    ///
    /// # Errors
    ///
    /// When git can't be started.
    pub async fn start(git: &Git, work_tree: &Path) -> Result<Self, GitError> {
        let process = git
            .read("cat-file")
            .args(["--batch-command", "--buffer"])
            .current_dir(work_tree)
            .spawn()
            .await?;
        Ok(Self {
            process,
            request: Vec::new(),
            header: Vec::new(),
            content: Vec::new(),
        })
    }

    /// Reads `ids`, handing each object to `each` in order (`None` for one that doesn't exist).
    /// The contents are borrowed until the next call of `each`.
    ///
    /// # Errors
    ///
    /// When git stops or answers something that isn't an object.
    pub async fn read(
        &mut self,
        ids: &[ObjectId],
        mut each: impl FnMut(ObjectId, Option<Object<'_>>),
    ) -> Result<(), GitError> {
        for batch in ids.chunks(Self::BATCH) {
            self.request.clear();
            for id in batch {
                // Writing into a `Vec` can't fail.
                let _ = writeln!(self.request, "contents {id}");
            }
            self.request.extend_from_slice(b"flush\n");
            self.process.send(&self.request).await.map_err(io_error)?;
            for &id in batch {
                let found = self.read_answer().await?;
                match found {
                    Some(kind) => each(
                        id,
                        Some(Object {
                            kind,
                            data: &self.content,
                        }),
                    ),
                    None => each(id, None),
                }
                if self.content.capacity() > Self::KEPT_BUFFER {
                    self.content = Vec::new();
                }
            }
        }
        Ok(())
    }

    /// Reads one answer into `self.content`: the object's kind, or `None` when it is missing.
    async fn read_answer(&mut self) -> Result<Option<ObjectKind>, GitError> {
        self.header.clear();
        let read = self
            .process
            .read_until(b'\n', &mut self.header)
            .await
            .map_err(io_error)?;
        if read == 0 {
            return Err(unexpected("it stopped answering"));
        }
        let header = self.header.strip_suffix(b"\n").unwrap_or(&self.header);
        let mut fields = header.split_str(b" ").skip(1);
        let (kind, size) = match (fields.next(), fields.next()) {
            (Some(b"missing" | b"ambiguous"), None) => return Ok(None),
            (Some(kind), Some(size)) => (kind, size),
            _ => {
                return Err(unexpected(format!(
                    "an answer {} isn't an object",
                    BStr::new(header)
                )));
            }
        };
        let kind = ObjectKind::parse(kind)
            .ok_or_else(|| unexpected(format!("an unknown kind {}", BStr::new(kind))))?;
        let size: usize = size
            .to_str()
            .ok()
            .and_then(|size| size.parse().ok())
            .ok_or_else(|| unexpected("an object's size isn't a number"))?;
        self.content.clear();
        self.content.resize(size, 0);
        self.process
            .read_exact(&mut self.content)
            .await
            .map_err(io_error)?;
        let mut newline = [0_u8; 1];
        self.process
            .read_exact(&mut newline)
            .await
            .map_err(io_error)?;
        if newline != *b"\n" {
            return Err(unexpected("an object didn't end where its size said"));
        }
        Ok(Some(kind))
    }
}

const fn io_error(source: io::Error) -> GitError {
    GitError::Io {
        subcommand: "cat-file",
        source,
    }
}

fn unexpected(detail: impl Into<String>) -> GitError {
    GitError::unexpected("cat-file", detail)
}
