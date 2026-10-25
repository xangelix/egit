//! `egit-relay`: serves egit to a browser, and runs git for it on this computer.
//!
//! Started under the askpass helper's name (a link a session's askpass server makes to it), it
//! is the helper instead.

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use egit_relay::{HELPER_NAME, Options, Token, is_proxy_address, run_helper};

/// Where a relay listens unless told.
const LISTEN: &str = "127.0.0.1:7171";

/// What code-server sets in its terminals: the address its proxy gives a port of this computer
/// (`https://…/proxy/{{port}}/`).
const CODE_SERVER_PROXY: &str = "VSCODE_PROXY_URI";

const USAGE: &str = "\
Usage: egit-relay [--listen ADDRESS] [--web FOLDER] [--host NAME]... [--url ADDRESS]...

Serves egit to a browser, and runs git for it on this computer.

  --listen ADDRESS  where to listen (default 127.0.0.1:7171: reach it through `ssh -L`)
  --web FOLDER      egit's web build (default: `web` beside this program)
  --host NAME       a host name a browser may use besides this computer's own addresses
                    (a proxy's), with its port or without; may be given more than once
  --url ADDRESS     the address a proxy gives browsers for the relay, perhaps under a path
                    of its own (https://code.example/proxy/{{port}}/: {{port}} is the port
                    listened on); its host is let in. May be given more than once. In
                    code-server's terminals its proxy's (VSCODE_PROXY_URI) is taken unless
                    one is given.

The addresses to open are printed when it listens, one a line: its own first, then the
proxies'. Their token (after `#`) is what lets a browser in. EGIT_RELAY_TOKEN gives one of
your own, so the addresses stay the same between runs.
";

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    if Path::new(&program)
        .file_name()
        .is_some_and(|name| name == HELPER_NAME)
    {
        return run_helper(&args.next().unwrap_or_default());
    }
    let options = match parse(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            let _said = std::io::stdout().write_all(USAGE.as_bytes());
            return ExitCode::SUCCESS;
        }
        Err(problem) => {
            let _said = writeln!(std::io::stderr(), "egit-relay: {problem}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("EGIT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _logging = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("egit-relay")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _said = writeln!(std::io::stderr(), "egit-relay: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let bound = match egit_relay::bind(options).await {
            Ok(bound) => bound,
            Err(error) => {
                let _said = writeln!(std::io::stderr(), "egit-relay: {error}");
                return ExitCode::FAILURE;
            }
        };
        // Standard output: what to open, the relay's own address first.
        let mut stdout = std::io::stdout();
        let mut said = writeln!(stdout, "{}", bound.url());
        for url in bound.proxied_urls() {
            said = said.and_then(|()| writeln!(stdout, "{url}"));
        }
        let _said = said.and_then(|()| stdout.flush());
        bound.serve().await;
        ExitCode::SUCCESS
    })
}

/// The options `args` give; `None` for `--help`.
fn parse(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<Option<Options>, String> {
    let mut listen: SocketAddr = LISTEN.parse().map_err(|_| "a bad default".to_owned())?;
    let mut web = None;
    let mut hosts = Vec::new();
    let mut urls = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{} needs a value", arg.to_string_lossy()))
        };
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--listen") => {
                let given = value()?;
                listen = given
                    .to_str()
                    .and_then(|given| given.parse().ok())
                    .ok_or_else(|| {
                        format!("{} isn't an address and port", given.to_string_lossy())
                    })?;
            }
            Some("--web") => web = Some(PathBuf::from(value()?)),
            Some("--host") => hosts.push(value()?.to_string_lossy().into_owned()),
            Some("--url") => {
                let given = value()?.to_string_lossy().into_owned();
                if !is_proxy_address(&given) {
                    return Err(format!(
                        "{given} isn't an address a browser opens (http:// or https://, a \
                         host, perhaps a path)"
                    ));
                }
                urls.push(given);
            }
            _ => return Err(format!("{} isn't an option", arg.to_string_lossy())),
        }
    }
    if urls.is_empty()
        && let Some(proxy) = std::env::var(CODE_SERVER_PROXY)
            .ok()
            .filter(|proxy| is_proxy_address(proxy))
    {
        urls.push(proxy);
    }
    let web = web.or_else(|| {
        let beside = std::env::current_exe().ok()?.parent()?.join("web");
        beside.is_dir().then_some(beside)
    });
    let token = match std::env::var("EGIT_RELAY_TOKEN") {
        Ok(token) if !token.is_empty() => Token::given(&token),
        _ => Token::random().map_err(|error| error.to_string())?,
    };
    Ok(Some(Options {
        listen,
        web,
        token,
        hosts,
        urls,
    }))
}
