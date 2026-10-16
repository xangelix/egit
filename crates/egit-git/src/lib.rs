//! egit's interface to the git binary.
//!
//! egit never reads or writes a repository itself: every operation runs the `git` installed on
//! the system, so hooks, signing, credential helpers, filters, and every repository format git
//! understands work exactly as they do on the command line. This crate runs that binary safely
//! and turns what it prints into types.
