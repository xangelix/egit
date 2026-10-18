//! egit, the application: its window, its views, and the work it hands to git.
//!
//! - `app` — the application's state, the actions views ask for, and the jobs behind them.
//! - `tabs` — the tabs across the window (`egui_tiles`): Home, the profile, one per repository.
//! - `views` — what each tab shows, and the status bar.
//! - `repo` — an open repository and the jobs that keep it current.
//! - `history` — a repository's history: rows streamed in by a walk, refs, commits' summaries.
//! - `diff` — a file's diff on the text grid: lines on their kind's background, numbered.
//! - `fonts` — system fonts for characters egui's fonts lack (CJK, most scripts).
//! - `grid` — the text grid: a measured text drawn as a terminal draws it, rows on screen only.
//! - `jobs` — the runtime and the write locks behind every git command.
//! - `tables` — what the tables show, as `egui-table-kit` providers.
//! - `theme`, `widgets`, `format` — colours and fonts, shared pieces, text for people.
//! - `settings` — what is remembered between runs.
//! - `syntax` — syntax highlighting: grammars parsed in the background, lines coloured on screen.
//! - `watch` — noticing what changes repositories and configuration behind egit's back.
//! - `worktree` — a repository's working tree: its status, kept current.

mod app;
mod blame;
mod commands;
mod desktop;
mod diff;
mod file_tree;
mod fonts;
mod forge;
mod format;
mod grid;
mod history;
mod jobs;
mod lfs;
mod merge;
mod pictures;
mod preferences;
mod problem;
mod repo;
mod search;
mod settings;
mod staging;
mod syntax;
mod tables;
mod tabs;
mod tasks;
mod theme;
mod tree_table;
mod undo;
mod views;
mod watch;
mod widgets;
mod worktree;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

/// What `egit --help` prints.
const USAGE: &str = "\
usage: egit [PATH…]

Opens egit, with the repositories around each PATH in tabs.

  -h, --help       print this
  -V, --version    print egit's version

EGIT_LOG sets what egit logs to stderr (\"warn\" by default; \"debug\" for every git command).
";

/// Runs egit with the command line's arguments (after the program's name) and returns how the
/// process should exit.
#[must_use]
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut paths = Vec::new();
    for arg in args {
        match arg.to_str() {
            Some("-h" | "--help") => return say(USAGE, ExitCode::SUCCESS),
            Some("-V" | "--version") => {
                return say(
                    &format!("egit {}\n", env!("CARGO_PKG_VERSION")),
                    ExitCode::SUCCESS,
                );
            }
            Some(option) if option.starts_with('-') => {
                complain(&format!("unknown option {option}\n\n{USAGE}"));
                return ExitCode::from(2);
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    init_logging();
    if let Err(error) = jobs::install_runtime() {
        complain(&format!("couldn't start egit's worker threads: {error}\n"));
        return ExitCode::FAILURE;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("egit")
            .with_app_id("egit")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0]),
        ..eframe::NativeOptions::default()
    };
    let started = eframe::run_native(
        "egit",
        options,
        Box::new(move |creation| Ok(Box::new(app::EgitApp::new(creation, paths)))),
    );
    match started {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            complain(&format!("the window couldn't open: {error}\n"));
            ExitCode::FAILURE
        }
    }
}

/// Logs to stderr, as `EGIT_LOG` asks (`warn` by default).
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("EGIT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    // Only fails when a logger is already installed, which is then the one used.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Prints `text` to stdout.
fn say(text: &str, code: ExitCode) -> ExitCode {
    match std::io::stdout().write_all(text.as_bytes()) {
        Ok(()) => code,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Prints `text` to stderr, prefixed with the program's name.
fn complain(text: &str) {
    // Nothing more can be done when stderr is gone.
    let _ = write!(std::io::stderr(), "egit: {text}");
}
