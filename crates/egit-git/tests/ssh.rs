//! A key's passphrase held and ssh connections shared while egit runs, against an sshd of the
//! test's own (on the loopback address, as this user, a key with a passphrase): cloning,
//! fetching, and pushing ask for the passphrase once; once the shared connection is gone (a
//! server dropping it idle, or refusing a second session on it) the key egit's own agent holds
//! still serves, a background fetch too, as it does a repository's own ssh command; without
//! either, every command asks; dropped, the shared connection closes and the agent ends.
//!
//! Linux only (sshd runs as any user there); skipped where sshd isn't installed, unless
//! `EGIT_REQUIRE_SSHD` is set (CI sets it).

#![cfg(target_os = "linux")]
// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers;
// a skipped test says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::print_stderr
)]

use std::ffi::OsString;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use egit_git::testing::{Sandbox, set_mode};
use egit_git::{
    CancelToken, CloneOptions, FetchOptions, Git, Holding, Progress, PushFlag, PushOptions,
    Repository, SshSharing, clone, fetch, push,
};

/// An sshd of the test's own, stopped when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
    }
}

/// An installed sshd (it must be started by an absolute path).
fn sshd() -> Option<PathBuf> {
    // The ssh git runs from this computer is what is tested.
    if egit_git::testing::on_another_computer() {
        eprintln!("skipped: git runs on another computer");
        return None;
    }
    let found = ["/usr/sbin/sshd", "/usr/bin/sshd", "/usr/local/sbin/sshd"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file());
    if found.is_none() {
        assert!(
            std::env::var_os("EGIT_REQUIRE_SSHD").is_none(),
            "EGIT_REQUIRE_SSHD is set, but sshd isn't installed"
        );
        eprintln!("skipped: sshd isn't installed");
    }
    found
}

/// The first `name` on `PATH`.
fn on_path(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{name} isn't on PATH"))
}

fn run(program: &Path, args: &[&str]) -> String {
    let out = Command::new(program).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{} {args:?}: {out:?}",
        program.display()
    );
    String::from_utf8(out.stdout).unwrap()
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    set_mode(path, 0o755).unwrap();
}

/// Keys, an sshd letting this user in by one with a passphrase, and an ssh reaching it as any
/// user's would, but for its configuration (that key, that host's key, no agent): first on
/// `PATH`, as the ssh the wrapper runs. Returns the server, the variables git needs (`PATH`,
/// an askpass counting what it is asked), and the file it counts in.
fn serve(sandbox: &Sandbox, sshd: &Path) -> (Server, Vec<(OsString, OsString)>, PathBuf) {
    let dir = sandbox.path("ssh");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let keygen = on_path("ssh-keygen");
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    run(
        &keygen,
        &["-q", "-t", "ed25519", "-N", "", "-f", &path("host")],
    );
    run(
        &keygen,
        &["-q", "-t", "ed25519", "-N", "secret", "-f", &path("id")],
    );
    std::fs::copy(dir.join("id.pub"), dir.join("authorized_keys")).unwrap();

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        dir.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nPidFile {pid}\n\
             AuthorizedKeysFile {keys}\nStrictModes no\nPasswordAuthentication no\n\
             KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nLogLevel ERROR\n",
            host = path("host"),
            pid = path("sshd.pid"),
            keys = path("authorized_keys"),
        ),
    )
    .unwrap();
    let child = Command::new(sshd)
        .args(["-D", "-e", "-f", &path("sshd_config")])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let server = Server { child, port };
    let until = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < until, "sshd didn't start listening");
        std::thread::sleep(Duration::from_millis(20));
    }

    let host_key = std::fs::read_to_string(dir.join("host.pub")).unwrap();
    std::fs::write(
        dir.join("known_hosts"),
        format!("[127.0.0.1]:{port} {host_key}"),
    )
    .unwrap();
    std::fs::write(
        dir.join("ssh_config"),
        format!(
            "Host *\n  IdentityFile {id}\n  IdentitiesOnly yes\n  \
             UserKnownHostsFile {known}\n  GlobalKnownHostsFile /dev/null\n  \
             StrictHostKeyChecking yes\n  PreferredAuthentications publickey\n  LogLevel ERROR\n",
            id = path("id"),
            known = path("known_hosts"),
        ),
    )
    .unwrap();
    write_script(
        &dir.join("bin/ssh"),
        &format!(
            "exec '{}' -F '{}' \"$@\"",
            on_path("ssh").display(),
            path("ssh_config")
        ),
    );
    let asked = dir.join("asked");
    write_script(
        &dir.join("askpass"),
        &format!("echo \"$1\" >> '{}'\necho secret", asked.display()),
    );
    let mut search = OsString::from(dir.join("bin"));
    search.push(":");
    search.push(std::env::var_os("PATH").unwrap());
    let env = vec![
        ("PATH".into(), search),
        ("SSH_ASKPASS".into(), dir.join("askpass").into()),
        ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        // Never the agent of whoever runs the tests: none, or egit's own.
        ("SSH_AUTH_SOCK".into(), OsString::new()),
    ];
    (server, env, asked)
}

/// How many times the passphrase was asked for.
fn asked(file: &Path) -> usize {
    std::fs::read_to_string(file).map_or(0, |asked| {
        assert!(
            asked.lines().all(|line| line.contains("passphrase")),
            "{asked}"
        );
        asked.lines().count()
    })
}

fn fetched(git: &Git, repository: &Repository) {
    fetch(
        git,
        repository,
        FetchOptions::default(),
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
}

/// A bare repository the server serves, and its address.
fn remote(sandbox: &Sandbox, server: &Server) -> String {
    let seed = sandbox.init("seed").unwrap();
    sandbox.commit_file(&seed, "f", "1\n", "Start").unwrap();
    let bare = sandbox.path("remote.git");
    sandbox
        .run(
            &seed,
            "clone",
            &["--quiet", "--bare", ".", bare.to_str().unwrap()],
        )
        .unwrap();
    let user = run(Path::new("id"), &["-un"]);
    format!(
        "ssh://{}@127.0.0.1:{}{}",
        user.trim(),
        server.port,
        bare.display()
    )
}

/// Pushes `main`, a fast-forward.
fn pushed(git: &Git, repository: &Repository) {
    let pushed = push(
        git,
        repository,
        &PushOptions {
            remote: "origin".into(),
            branch: "main".into(),
            remote_branch: "main".into(),
            set_upstream: false,
            force: false,
        },
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(
        pushed
            .refs
            .iter()
            .map(|pushed| pushed.flag)
            .collect::<Vec<_>>(),
        [PushFlag::FastForward]
    );
}

/// Closes the shared master, as a server dropping an idle connection does.
fn close_master(sharing: &SshSharing) {
    let pid = master(sharing);
    let socket = master_socket(sharing);
    run(
        &on_path("ssh"),
        &[
            "-F",
            "/dev/null",
            "-S",
            socket.to_str().unwrap(),
            "-O",
            "exit",
            "egit",
        ],
    );
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < until, "the master is still running");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The shared master's socket.
fn master_socket(sharing: &SshSharing) -> PathBuf {
    std::fs::read_dir(sharing.dir())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name != "ssh" && name != "agent")
        })
        .expect("a master's socket")
}

/// The shared master's process, as it says.
fn master(sharing: &SshSharing) -> u32 {
    let socket = master_socket(sharing);
    let out = Command::new("ssh")
        .args(["-F", "/dev/null", "-S"])
        .arg(&socket)
        .args(["-O", "check", "egit"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    said.split("pid=")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("no master: {said}"))
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_passphrase_is_asked_once_while_connections_are_shared() {
    let Some(sshd) = sshd() else {
        return;
    };
    let sandbox = Sandbox::new().unwrap();
    let (server, env, asked_file) = serve(&sandbox, &sshd);
    let url = remote(&sandbox, &server);

    let sharing = SshSharing::start_in(&sandbox.path("run"), Holding::Own).unwrap();
    assert_eq!(sharing.holding(), Holding::Own);
    let shared = sandbox
        .git_with(env.iter().cloned().chain(sharing.environment()))
        .unwrap();
    let work = sandbox.path("work");
    clone(
        &shared,
        &url,
        &work,
        CloneOptions::default(),
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(asked(&asked_file), 1);
    let repository = Repository::discover(&shared, &work).unwrap();
    fetched(&shared, &repository);
    fetched(&shared, &repository);
    sandbox.commit_file(&work, "f", "2\n", "More").unwrap();
    pushed(&shared, &repository);
    // Cloned, fetched twice, pushed: asked once.
    assert_eq!(asked(&asked_file), 1);

    // The shared connection gone (a server dropping it idle, or refusing a second session on
    // it): the key egit's agent holds serves the next connection, a background fetch's too,
    // which can't ask.
    close_master(&sharing);
    fetched(&shared, &repository);
    close_master(&sharing);
    fetch(
        &shared,
        &repository,
        FetchOptions {
            background: true,
            ..FetchOptions::default()
        },
        &Progress::new(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(asked(&asked_file), 1);

    // Without sharing or an agent, each command asks.
    let unshared = sandbox.git_with(env.iter().cloned()).unwrap();
    fetched(&unshared, &repository);
    fetched(&unshared, &repository);
    assert_eq!(asked(&asked_file), 3);
    // Where the repository names its own ssh command (it outranks the wrapper), the key held
    // serves it too.
    sandbox
        .run(
            &work,
            "config",
            &[
                "core.sshCommand",
                sandbox.path("ssh/bin/ssh").to_str().unwrap(),
            ],
        )
        .unwrap();
    fetched(&shared, &repository);
    assert_eq!(asked(&asked_file), 3);
    sandbox
        .run(&work, "config", &["--unset", "core.sshCommand"])
        .unwrap();
    fetched(&shared, &repository);

    // Dropped, the shared connection closes, and egit's agent forgets the key and ends.
    let pid = master(&sharing);
    assert!(alive(pid));
    let dir = sharing.dir().to_owned();
    let agent = dir.join("agent");
    let held = Command::new(on_path("ssh-add"))
        .arg("-l")
        .env("SSH_AUTH_SOCK", &agent)
        .output()
        .unwrap();
    assert!(held.status.success(), "{held:?}");
    drop(sharing);
    assert!(!dir.exists());
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < until, "the master is still running");
        std::thread::sleep(Duration::from_millis(20));
    }
}
