//! The `egit` executable: the application, or git's and ssh's askpass helper when started under
//! the helper's name (a link egit makes to itself, `egit-askpass`).

// No console window behind the application on Windows (a debug build keeps one, for its logs).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::process::ExitCode;

#[cfg(not(target_arch = "wasm32"))]
use egit_git::askpass::{HELPER_NAME, run_helper};

/// In a browser, egit starts from its page (`egit-web`), not here.
#[cfg(target_arch = "wasm32")]
const fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    let name = Path::new(&program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // Windows matches names in any case, and may leave `.exe` off.
    let helper = if cfg!(windows) {
        name.eq_ignore_ascii_case(HELPER_NAME)
            || HELPER_NAME
                .strip_suffix(".exe")
                .is_some_and(|stem| name.eq_ignore_ascii_case(stem))
    } else {
        name == HELPER_NAME
    };
    if helper {
        return run_helper(&args.next().unwrap_or_default());
    }
    egit_app::run(args)
}
