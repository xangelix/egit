//! In a browser: no files of its own, so no repository is ever opened here, and git (through
//! its relay) reads everything.

#![allow(
    clippy::unused_self,
    clippy::unnecessary_wraps,
    clippy::missing_const_for_fn,
    reason = "the shape of every platform's own module, which these stand in for"
)]

use std::path::Path;

use egit_git::{Object, ObjectId, Reference};

/// Why gix couldn't read what it was asked: here, never anything.
#[derive(Debug, thiserror::Error)]
#[error("gitoxide reads nothing in a browser")]
pub struct GixError;

/// A repository opened for gix's reads: never one here ([`Self::open`] gives none).
#[derive(Clone, Debug)]
pub struct GixRepository;

impl GixRepository {
    /// Never opens: git reads it.
    #[must_use]
    pub const fn open(_path: &Path) -> Option<Self> {
        None
    }

    /// Nothing read.
    ///
    /// # Errors
    ///
    /// Always.
    pub const fn references(&self) -> Result<Vec<Reference>, GixError> {
        Err(GixError)
    }

    /// Nothing read.
    ///
    /// # Errors
    ///
    /// Always.
    pub const fn tips(&self) -> Result<Vec<ObjectId>, GixError> {
        Err(GixError)
    }

    /// Nothing read.
    ///
    /// # Errors
    ///
    /// Always.
    pub fn read(
        &self,
        _ids: &[ObjectId],
        _each: impl FnMut(ObjectId, Option<Object<'_>>),
    ) -> Result<usize, GixError> {
        Err(GixError)
    }

    /// A reader that reads nothing.
    #[must_use]
    pub const fn reader(&self) -> GixReader {
        GixReader
    }
}

/// Reads objects on one thread: here, nothing.
#[derive(Debug)]
pub struct GixReader;

impl GixReader {
    /// Nothing read.
    ///
    /// # Errors
    ///
    /// Always.
    pub const fn read(&mut self, _id: &ObjectId) -> Result<Option<Object<'_>>, GixError> {
        Err(GixError)
    }
}
