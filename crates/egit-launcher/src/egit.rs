//! egit's launcher, installed as `egit` (and again beside it as `egit-askpass`, the helper git
//! and ssh start): it starts the best of egit's per-CPU cores (`egit_core`, one per level in
//! `seaworthy.toml`) the computer runs, in this process. Built for its architecture's baseline,
//! so it runs anywhere and can say when a CPU is too old.

// No console window behind the application on Windows: the launcher shows its own failures in
// a message box there instead (a debug build keeps one, as egit's own executable does).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(target_os = "macos"))]
seaworthy_launcher::launcher_main!(
    core = "egit_core",
    cpus = ["x86-64-v4", "x86-64-v3", "x86-64-v2"],
);

#[cfg(target_os = "macos")]
seaworthy_launcher::launcher_main!(core = "egit_core", cpus = ["apple-m1"]);
