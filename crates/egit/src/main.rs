//! The `egit` executable: the application, or git's and ssh's askpass helper when started under
//! the helper's name (a link egit makes to itself, `egit-askpass`).

use std::path::Path;
use std::process::ExitCode;

use egit_git::askpass::{HELPER_NAME, run_helper};

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    if Path::new(&program).file_name() == Some(HELPER_NAME.as_ref()) {
        return run_helper(&args.next().unwrap_or_default());
    }
    egit_app::run(args)
}
