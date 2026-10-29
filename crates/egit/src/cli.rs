//! The command line: what egit was started as, and what its arguments ask.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use egit_git::askpass::{HELPER_NAME, run_helper};

/// egit's entry, shared by its executable and its per-CPU cores (`egit-core`).
///
/// Started under the helper's name (the installed `egit-askpass` beside the application, or the
/// link egit makes to itself elsewhere), it is git's and ssh's askpass helper; else the
/// application, with the command line's arguments.
#[must_use]
pub fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    if is_helper(&program) {
        return run_helper(&args.next().unwrap_or_default());
    }
    crate::run(args)
}

/// Whether a process started as `program` (its first argument) is the askpass helper: its file
/// name is the helper's (on Windows in any case, `.exe` or not, as Windows matches names).
fn is_helper(program: &OsStr) -> bool {
    let name = Path::new(program)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if cfg!(windows) {
        name.eq_ignore_ascii_case(HELPER_NAME)
            || HELPER_NAME
                .strip_suffix(".exe")
                .is_some_and(|stem| name.eq_ignore_ascii_case(stem))
    } else {
        name == HELPER_NAME
    }
}

/// What the arguments after the program's name ask.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    /// `-h`, `--help`.
    Help,
    /// `-V`, `--version`.
    Version,
    /// An option egit doesn't know.
    Unknown(String),
    /// The window, with the repositories around these paths in tabs.
    Open(Vec<PathBuf>),
}

/// Reads the arguments: options until `--`, after which every argument is a path, even one
/// starting with `-` (a file manager opens a folder as `egit -- <folder>`, so no folder's name
/// is ever read as an option).
pub(crate) fn asked(args: impl IntoIterator<Item = OsString>) -> Asked {
    let mut paths = Vec::new();
    let mut options = true;
    for arg in args {
        if !options {
            paths.push(PathBuf::from(arg));
            continue;
        }
        match arg.to_str() {
            Some("--") => options = false,
            Some("-h" | "--help") => return Asked::Help,
            Some("-V" | "--version") => return Asked::Version,
            Some(option) if option.starts_with('-') => return Asked::Unknown(option.to_owned()),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    Asked::Open(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    fn paths(words: &[&str]) -> Asked {
        Asked::Open(words.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn paths_open_and_options_ask_what_they_say() {
        assert_eq!(asked(args(&[])), paths(&[]));
        assert_eq!(asked(args(&["a", "b/c"])), paths(&["a", "b/c"]));
        assert_eq!(asked(args(&["a", "--help"])), Asked::Help);
        assert_eq!(asked(args(&["-V"])), Asked::Version);
        assert_eq!(
            asked(args(&["--open", "a"])),
            Asked::Unknown("--open".to_owned())
        );
    }

    #[test]
    fn after_two_dashes_every_argument_is_a_path() {
        assert_eq!(asked(args(&["--", "-dir"])), paths(&["-dir"]));
        assert_eq!(
            asked(args(&["a", "--", "--help", "--", "-V"])),
            paths(&["a", "--help", "--", "-V"])
        );
        assert_eq!(asked(args(&["--"])), paths(&[]));
    }

    #[test]
    fn the_helper_is_known_by_its_name_alone() {
        assert!(is_helper(OsStr::new(HELPER_NAME)));
        assert!(is_helper(
            Path::new("/opt/egit/app").join(HELPER_NAME).as_os_str()
        ));
        assert!(!is_helper(OsStr::new("egit")));
        assert!(!is_helper(OsStr::new("egit-askpass-not")));
        assert!(!is_helper(OsStr::new("")));
        if cfg!(windows) {
            assert!(is_helper(OsStr::new("EGIT-ASKPASS")));
            assert!(is_helper(OsStr::new("Egit-Askpass.EXE")));
        } else {
            assert!(!is_helper(OsStr::new("EGIT-ASKPASS")));
        }
    }
}
