//! `egit-relay`: serves egit to a browser, and runs git for it on this computer.
//!
//! Started under the askpass helper's name (a link a session's askpass server makes to it), it
//! is the helper instead. The installed relay is a launcher and its per-CPU cores
//! (`egit-launcher`'s `egit-relay-launcher`, `egit_relay_core`) instead, sharing this entry
//! (`egit_relay::main`).

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(egit_relay::main())
}
