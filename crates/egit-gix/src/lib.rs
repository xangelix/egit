//! egit's in-process reads through gitoxide (`gix`): a repository's references, the commits a
//! walk of all of them starts from, and objects. Each gives what egit's git commands give
//! (checked against them in tests), and each was measured faster where egit spends its time
//! (`tests/measure.rs`); everything else stays git's, and every write.
//!
//! A repository gix wouldn't read as git does isn't opened here, so its callers fall back to
//! git: references kept in a reftable (gix reads files), and replaced objects (`refs/replace/`,
//! which `cat-file` reads in their place). A partial clone is read as far as it goes: an object
//! it hasn't may be its remote's, which git fetches when asked for it, so reading stops there
//! and leaves the rest to git.
//!
//! In a browser there are no files to read: a repository never opens, and git reads everything.

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod here;

#[cfg(target_arch = "wasm32")]
pub use browser::{GixError, GixReader, GixRepository};
#[cfg(not(target_arch = "wasm32"))]
pub use here::{GixError, GixReader, GixRepository};
