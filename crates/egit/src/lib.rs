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

mod agents;
mod app;
mod attachments;
mod avatars;
mod blame;
mod ci;
#[cfg(not(target_arch = "wasm32"))]
mod cli;
mod commands;
mod desktop;
mod diff;
mod discarding;
mod downloads;
mod exporting;
mod file_tree;
mod fonts;
mod foresight;
mod forge;
mod format;
mod grid;
mod help;
mod history;
mod ignoring;
mod job_log;
mod jobs;
mod lfs;
mod log_folds;
mod markdown;
mod merge;
mod output;
mod pictures;
mod preferences;
mod problem;
mod repaint;
mod repo;
mod search;
mod settings;
mod sign_ins;
mod stack;
mod staging;
mod stats;
mod submodules;
mod suggested;
mod syntax;
mod tables;
mod tabs;
mod tasks;
mod theme;
mod tree_table;
mod undo;
mod views;
mod watch;
#[cfg(target_arch = "wasm32")]
mod web;
mod widgets;
mod worktree;

#[cfg(test)]
mod tests;

#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsString;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write as _;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::process::ExitCode;

#[cfg(not(target_arch = "wasm32"))]
pub use cli::main;
#[cfg(target_arch = "wasm32")]
pub use web::start;

/// egit's id where a desktop names applications.
///
/// Its window's on Linux (Wayland's app id, X11's class), its menu entry's and icons'
/// (`seaworthy.toml`'s `desktop.app-id`), and its macOS bundle's: one id, so the desktop shows the
/// window under egit's entry and icon. Never changes: macOS keys an app's permissions and
/// notarization to its bundle id.
pub const APP_ID: &str = "io.github.xangelix.egit";

/// The window's icon, the 256-pixel rendering of `packaging/icons/egit.svg`.
#[cfg(not(target_arch = "wasm32"))]
const ICON_PNG: &[u8] = include_bytes!("../../../packaging/icons/egit-256.png");

/// What `egit --help` prints.
#[cfg(not(target_arch = "wasm32"))]
const USAGE: &str = "\
usage: egit [PATH…] [-- PATH…]

Opens egit, with the repositories around each PATH in tabs.

  -h, --help       print this
  -V, --version    print egit's version
  --               every argument after it is a PATH, even one starting with -

EGIT_LOG sets what egit logs to stderr (\"warn\" by default; \"debug\" for every git command;
\"egit_forge=debug,egit_app=debug\" for each request to GitHub or GitLab and what each remote
was taken for).
";

/// Runs egit with the command line's arguments (after the program's name) and returns how the
/// process should exit.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let paths: Vec<PathBuf> = match cli::asked(args) {
        cli::Asked::Help => return say(USAGE, ExitCode::SUCCESS),
        cli::Asked::Version => {
            return say(
                &format!("egit {}\n", env!("CARGO_PKG_VERSION")),
                ExitCode::SUCCESS,
            );
        }
        cli::Asked::Unknown(option) => {
            complain(&format!("unknown option {option}\n\n{USAGE}"));
            return ExitCode::from(2);
        }
        cli::Asked::Open(paths) => paths,
    };

    init_logging();
    if let Err(error) = jobs::install_runtime() {
        complain(&format!("couldn't start egit's worker threads: {error}\n"));
        return ExitCode::FAILURE;
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("egit")
        .with_app_id(APP_ID)
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([720.0, 480.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
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

/// The window's icon, decoded from [`ICON_PNG`]; none (the platform's default, said in the log)
/// should it not decode.
#[cfg(not(target_arch = "wasm32"))]
fn window_icon() -> Option<egui::IconData> {
    match image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png) {
        Ok(decoded) => {
            let rgba = decoded.into_rgba8();
            let (width, height) = rgba.dimensions();
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(error) => {
            tracing::warn!(%error, "the window's icon didn't decode");
            None
        }
    }
}

/// Logs to stderr, as `EGIT_LOG` asks (`warn` by default).
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn say(text: &str, code: ExitCode) -> ExitCode {
    match std::io::stdout().write_all(text.as_bytes()) {
        Ok(()) => code,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Prints `text` to stderr, prefixed with the program's name.
#[cfg(not(target_arch = "wasm32"))]
fn complain(text: &str) {
    // Nothing more can be done when stderr is gone.
    let _ = write!(std::io::stderr(), "egit: {text}");
}
