//! The `egit` executable: the application, or git's and ssh's askpass helper when started under
//! the helper's name (a link egit makes to itself, `egit-askpass`). The installed application is
//! a launcher and its per-CPU cores (`egit-launcher`, `egit_core`) instead, sharing this entry
//! (`egit_app::main`).

// No console window behind the application on Windows (a debug build keeps one, for its logs).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

/// In a browser, egit starts from its page (`egit-web`), not here.
#[cfg(target_arch = "wasm32")]
const fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(egit_app::main())
}
