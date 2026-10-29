//! The relay's launcher, installed as `egit-relay` (and again beside it as `egit-askpass`): it
//! starts the best of the relay's per-CPU cores (`egit_relay_core`) the computer runs, in this
//! process.

#[cfg(not(target_os = "macos"))]
seaworthy_launcher::launcher_main!(
    core = "egit_relay_core",
    cpus = ["x86-64-v4", "x86-64-v3", "x86-64-v2"],
);

#[cfg(target_os = "macos")]
seaworthy_launcher::launcher_main!(core = "egit_relay_core", cpus = ["apple-m1"]);
