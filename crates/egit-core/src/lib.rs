//! egit as a per-CPU core: the application (`egit_app::main`, the development executable's
//! entry too), built once per CPU level in `seaworthy.toml` with `-C target-cpu=<level>`, its
//! requirement record written into it by seaworthy-launcher's build script. egit's launcher
//! (`egit-launcher`) loads the best one the computer runs and calls it.

seaworthy_launcher::core_main!(egit_app::main);
