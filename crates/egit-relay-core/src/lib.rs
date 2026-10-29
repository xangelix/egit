//! The relay as a per-CPU core: `egit_relay::main` (the development executable's entry too),
//! built once per CPU level in `seaworthy.toml`, which its launcher (`egit-launcher`'s
//! `egit-relay-launcher`) loads and calls.

seaworthy_launcher::core_main!(egit_relay::main);
