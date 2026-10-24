//! A sandboxed git for tests (the `testing` feature).
//!
//! A sandbox is a temporary directory with a home of its own and a known global configuration.
//! The system's configuration is ignored, and so is the user's, and so are identities and dates
//! the environment would give commits (`GIT_AUTHOR_NAME`, say), so tests behave the same on
//! every machine. Repository discovery stops at the sandbox (`GIT_CEILING_DIRECTORIES`).
//!
//! Set `EGIT_TEST_GIT` to a git executable to run the tests against it (an older release, say)
//! instead of the one on `PATH`. Set `EGIT_TEST_SSH` to run every sandbox's git as on another
//! computer: this one, through an sshd of the tests' own on the loopback address (Linux, where
//! sshd runs as any user), each command a session of one shared connection, as egit runs git
//! over SSH. Set `EGIT_TEST_RELAY` to an `egit-relay` executable to run it through a relay
//! instead, as egit in a browser does: one relay a process, on the loopback address, every
//! sandbox's commands over one WebSocket.

use crate::exec::Wait as _;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::{Completed, Git, GitOptions, Host, SshHost};

/// The global configuration every sandbox starts with.
const GLOBAL_CONFIG: &str = "\
[user]
\tname = egit tests
\temail = tests@egit.invalid
[init]
\tdefaultBranch = main
[commit]
\tgpgSign = false
[tag]
\tgpgSign = false
[core]
\tautocrlf = false
";

/// Variables that would give commits another identity or date than the sandbox's.
const IDENTITY_VARIABLES: [&str; 7] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "EMAIL",
];

/// Variables naming the ssh command git runs: a test names its own, or none.
const SSH_VARIABLES: [&str; 3] = ["GIT_SSH_COMMAND", "GIT_SSH", "GIT_SSH_VARIANT"];

/// What can go wrong setting up a sandbox.
pub type SandboxError = Box<dyn Error + Send + Sync>;

/// A temporary home with its own git configuration.
#[derive(Debug)]
pub struct Sandbox {
    root: TempDir,
    home: PathBuf,
    env: Vec<(OsString, OsString)>,
    git: Git,
}

/// Sets `path`'s permission bits to `mode` where files have them (Unix); elsewhere does
/// nothing: Git for Windows runs a script by its `#!` line, and keeps no permission bits.
///
/// # Errors
///
/// When the permissions can't be set.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Sets `path`'s permission bits to `mode` where files have them (Unix); elsewhere does
/// nothing: Git for Windows runs a script by its `#!` line, and keeps no permission bits.
///
/// # Errors
///
/// None here.
#[cfg(not(unix))]
pub const fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// The tests' own sshd, on the loopback address (started the first time; Linux, where sshd
/// runs as any user): how to reach this computer as if it were another.
///
/// # Errors
///
/// When sshd isn't installed, or can't be started.
pub fn ssh_host() -> Result<SshHost, SandboxError> {
    ssh::host()
}

/// Whether sandboxes run git as on another computer (`EGIT_TEST_SSH`, or through a relay:
/// `EGIT_TEST_RELAY`): a test of what only this computer does (its askpass, git's own ssh)
/// skips then.
#[must_use]
pub fn on_another_computer() -> bool {
    std::env::var_os("EGIT_TEST_SSH").is_some() || through_a_relay()
}

/// Whether sandboxes run git through a relay (`EGIT_TEST_RELAY`).
#[must_use]
pub fn through_a_relay() -> bool {
    std::env::var_os("EGIT_TEST_RELAY").is_some_and(|relay| !relay.is_empty())
}

/// The tests' own relay (started the first time, and connected to): how egit in a browser
/// reaches this computer.
///
/// # Errors
///
/// When `EGIT_TEST_RELAY` names none, or it can't be started or reached.
pub fn relay() -> Result<crate::Relay, SandboxError> {
    relay::connection()
}

/// Where the tests' own relay listens (`127.0.0.1:PORT`), as a browser reaches it: started the
/// first time, as [`relay`] does.
///
/// # Errors
///
/// As [`relay`]'s.
pub fn relay_address() -> Result<String, SandboxError> {
    relay::address()
}

pub use relay::TestRelay;

impl Sandbox {
    /// A new sandbox around the git named by `EGIT_TEST_GIT`, or the one on `PATH`.
    ///
    /// # Errors
    ///
    /// When the directory can't be created or git can't be started.
    pub fn new() -> Result<Self, SandboxError> {
        let root = tempfile::Builder::new().prefix("egit-sandbox-").tempdir()?;
        let home = root.path().join("home");
        fs::create_dir(&home)?;
        let global_config = home.join(".gitconfig");
        fs::write(&global_config, GLOBAL_CONFIG)?;

        let env: Vec<(OsString, OsString)> = [
            ("HOME", home.clone().into_os_string()),
            ("XDG_CONFIG_HOME", home.join(".config").into_os_string()),
            ("GIT_CONFIG_GLOBAL", global_config.into_os_string()),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            (
                "GIT_CEILING_DIRECTORIES",
                root.path().as_os_str().to_owned(),
            ),
            // An editor's terminal may export its own askpass helper; nothing in a test may
            // prompt anyone.
            ("GIT_ASKPASS", OsString::new()),
            ("SSH_ASKPASS", OsString::new()),
        ]
        .into_iter()
        .map(|(key, value)| (OsString::from(key), value))
        .collect();
        let git = Self::git_from(env.clone())?;
        Ok(Self {
            root,
            home,
            env,
            git,
        })
    }

    fn git_from(env: Vec<(OsString, OsString)>) -> Result<Git, SandboxError> {
        let host = if through_a_relay() {
            Host::Relay(relay::connection()?)
        } else if on_another_computer() {
            Host::Ssh(ssh::host()?)
        } else {
            Host::Local
        };
        Ok(Git::new(GitOptions {
            host,
            ..Self::options_from(env)
        })
        .wait()?)
    }

    /// How the sandbox's git is run, here: its program, variables, and those it doesn't get.
    #[must_use]
    pub fn options(&self) -> GitOptions {
        Self::options_from(self.env.clone())
    }

    fn options_from(env: Vec<(OsString, OsString)>) -> GitOptions {
        GitOptions {
            host: Host::Local,
            program: std::env::var_os("EGIT_TEST_GIT").map(PathBuf::from),
            // The environment's identity and ssh command, unless a test names its own (a commit's
            // date, say).
            unset: IDENTITY_VARIABLES
                .iter()
                .chain(SSH_VARIABLES.iter())
                .filter(|name| !env.iter().any(|(key, _)| key == **name))
                .map(OsString::from)
                .collect(),
            env,
        }
    }

    /// The sandbox's git through `relay`, with more environment variables.
    ///
    /// # Errors
    ///
    /// When the relay refuses, or git there can't be started.
    pub fn git_through<K, V>(
        &self,
        relay: &crate::Relay,
        extra: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Git, SandboxError>
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        // The relay's own askpass asks, and only where the test takes its prompts.
        let mut env: Vec<(OsString, OsString)> = self
            .env
            .iter()
            .filter(|(key, _)| key != "GIT_ASKPASS" && key != "SSH_ASKPASS")
            .cloned()
            .collect();
        env.extend(
            extra
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        Ok(Git::new(GitOptions {
            host: Host::Relay(relay.clone()),
            ..Self::options_from(env)
        })
        .wait()?)
    }

    /// The sandbox's git with more environment variables, such as git's own test switches
    /// (`GIT_TEST_ASSUME_DIFFERENT_OWNER`) or an askpass server's.
    ///
    /// # Errors
    ///
    /// When git can't be started.
    pub fn git_with<K, V>(
        &self,
        extra: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Git, SandboxError>
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let mut env = self.env.clone();
        env.extend(
            extra
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        Self::git_from(env)
    }

    /// Git, configured for the sandbox.
    #[must_use]
    pub const fn git(&self) -> &Git {
        &self.git
    }

    /// The sandbox's home directory (it holds the global `.gitconfig`).
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// A path inside the sandbox.
    #[must_use]
    pub fn path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.root.path().join(name)
    }

    /// A new, empty repository called `name`, on branch `main`.
    ///
    /// # Errors
    ///
    /// When the directory can't be created or `git init` fails.
    pub fn init(&self, name: &str) -> Result<PathBuf, SandboxError> {
        let repo = self.path(name);
        fs::create_dir_all(&repo)?;
        self.git
            .write("init")
            .arg("--quiet")
            .current_dir(&repo)
            .output()
            .wait()?;
        Ok(repo)
    }

    /// Runs `git <subcommand> <args>` in `dir`, which must succeed.
    ///
    /// # Errors
    ///
    /// When git fails.
    pub fn run(
        &self,
        dir: &Path,
        subcommand: &'static str,
        args: &[&str],
    ) -> Result<Completed, SandboxError> {
        Ok(self
            .git
            .write(subcommand)
            .args(args)
            .current_dir(dir)
            .output()
            .wait()?)
    }

    /// Writes `contents` to `file` in `repo` and commits it with `message`.
    ///
    /// # Errors
    ///
    /// When the file can't be written or git fails.
    pub fn commit_file(
        &self,
        repo: &Path,
        file: &str,
        contents: &str,
        message: &str,
    ) -> Result<(), SandboxError> {
        let path = repo.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        self.run(repo, "add", &["--", file])?;
        self.run(repo, "commit", &["--quiet", "--message", message])?;
        Ok(())
    }
}

/// The tests' own relay, for `EGIT_TEST_RELAY`: the executable it names, started once a
/// process through a shell that stops it when the process ends (its input closes), and one
/// connection to it, a WebSocket on a thread of its own.
mod relay {
    use std::io::{BufRead as _, BufReader};
    use std::path::Path;
    use std::process::{ChildStdin, Command, Stdio};
    use std::sync::OnceLock;

    use futures_util::{SinkExt as _, StreamExt as _};
    use parking_lot::Mutex;
    use tokio::sync::mpsc::{self, UnboundedSender};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    use super::SandboxError;
    use crate::{Relay, Transport};

    static CONNECTED: OnceLock<Result<(Relay, TestRelay), String>> = OnceLock::new();

    /// The connection every sandbox's git runs through, the relay started the first time.
    pub(super) fn connection() -> Result<Relay, SandboxError> {
        CONNECTED
            .get_or_init(|| {
                let started = || -> Result<(Relay, TestRelay), SandboxError> {
                    let program = std::env::var_os("EGIT_TEST_RELAY")
                        .ok_or("EGIT_TEST_RELAY names no relay")?;
                    let relay = TestRelay::start(Path::new(&program))?;
                    Ok((relay.connect(&relay.token)?, relay))
                };
                started().map_err(|error| error.to_string())
            })
            .as_ref()
            .map(|(relay, _)| relay.clone())
            .map_err(|error| SandboxError::from(error.clone()))
    }

    /// Where the relay [`connection`] reaches listens.
    pub(super) fn address() -> Result<String, SandboxError> {
        connection()?;
        CONNECTED
            .get()
            .and_then(|connected| connected.as_ref().ok())
            .map(|(_, relay)| relay.address.clone())
            .ok_or_else(|| SandboxError::from("the tests' relay isn't there".to_owned()))
    }

    /// What a test's connection sends: into its WebSocket's writer (`None` closes it).
    struct Socket(UnboundedSender<Option<Vec<u8>>>);

    impl Transport for Socket {
        fn send(&self, message: Vec<u8>) {
            let _sent = self.0.send(Some(message));
        }

        fn close(&self) {
            let _sent = self.0.send(None);
        }
    }

    /// A relay of a test's own: `egit-relay` on the loopback address, stopped when dropped.
    #[derive(Debug)]
    pub struct TestRelay {
        /// Where it listens (`127.0.0.1:PORT`).
        pub address: String,
        /// Its token.
        pub token: String,
        /// The input of the shell keeping it: closed, the relay stops.
        keeper: Mutex<Option<ChildStdin>>,
    }

    impl TestRelay {
        /// Starts `program` (an `egit-relay`) on a port of its own.
        ///
        /// # Errors
        ///
        /// When it can't be started, or doesn't say where it listens.
        pub fn start(program: &Path) -> Result<Self, SandboxError> {
            Self::start_with(program, &[])
        }

        /// Starts `program` (an `egit-relay`) on a port of its own, with more of its options.
        ///
        /// # Errors
        ///
        /// When it can't be started, or doesn't say where it listens.
        pub fn start_with(program: &Path, options: &[&str]) -> Result<Self, SandboxError> {
            let mut keeper = Command::new("sh")
                .args([
                    "-c",
                    "prog=$1; shift; \"$prog\" --listen 127.0.0.1:0 \"$@\" & pid=$!; \
                     cat >/dev/null; kill \"$pid\"; wait \"$pid\"",
                    "sh",
                ])
                .arg(program)
                .args(options)
                // Its errors alone: a test refusing a browser on purpose warns.
                .env(
                    "EGIT_LOG",
                    std::env::var_os("EGIT_LOG").unwrap_or_else(|| "error".into()),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            let input = keeper
                .stdin
                .take()
                .ok_or("the relay's keeper has no input")?;
            let output = keeper
                .stdout
                .take()
                .ok_or("the relay's keeper has no output")?;
            let mut line = String::new();
            BufReader::new(output).read_line(&mut line)?;
            // `http://127.0.0.1:PORT/#TOKEN`
            let (address, token) = line
                .trim()
                .strip_prefix("http://")
                .and_then(|rest| rest.split_once("/#"))
                .ok_or_else(|| format!("the relay said {line:?}, not where it listens"))?;
            Ok(Self {
                address: address.to_owned(),
                token: token.to_owned(),
                keeper: Mutex::new(Some(input)),
            })
        }

        /// A connection to it, saying `token`: a WebSocket on a thread of its own.
        ///
        /// # Errors
        ///
        /// When the thread can't be started (one that can't connect ends at once, saying why).
        pub fn connect(&self, token: &str) -> Result<Relay, SandboxError> {
            let address = self.address.clone();
            let (sender, mut sending) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
            let relay = Relay::new(Socket(sender), token.as_bytes());
            let reading = relay.clone();
            let mut request = format!("ws://{address}/relay").into_client_request()?;
            request
                .headers_mut()
                .insert("Origin", format!("http://{address}").parse()?);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            std::thread::Builder::new()
                .name("a test's relay".to_owned())
                .spawn(move || {
                    runtime.block_on(async move {
                        let connected = async {
                            let stream = tokio::net::TcpStream::connect(&address).await?;
                            stream.set_nodelay(true)?;
                            let (socket, _) =
                                tokio_tungstenite::client_async(request, stream).await?;
                            Ok::<_, SandboxError>(socket)
                        };
                        let socket = match connected.await {
                            Ok(socket) => socket,
                            Err(error) => {
                                reading.closed(&format!("the relay can't be reached: {error}"));
                                return;
                            }
                        };
                        let (mut sink, mut stream) = socket.split();
                        let writing = async {
                            while let Some(Some(message)) = sending.recv().await {
                                if sink.send(Message::Binary(message.into())).await.is_err() {
                                    break;
                                }
                            }
                            let _closed = sink.close().await;
                        };
                        let read = async {
                            while let Some(Ok(message)) = stream.next().await {
                                if let Message::Binary(bytes) = message {
                                    reading.receive(&bytes);
                                }
                            }
                            reading.closed("the relay went away");
                        };
                        tokio::select! {
                            () = writing => {}
                            () = read => {}
                        }
                        // Closed this side, or that: whichever said first is why.
                        reading.closed("the connection to the relay closed");
                    });
                })?;
            Ok(relay)
        }
    }

    impl Drop for TestRelay {
        fn drop(&mut self) {
            // Its keeper's input closes: the relay is stopped.
            self.keeper.lock().take();
        }
    }
}

/// The tests' own sshd, for `EGIT_TEST_SSH`.
mod ssh {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{ChildStdin, Command, Stdio};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    use super::SandboxError;
    use crate::SshHost;

    /// The sshd every sandbox of the process reaches, and the input of the shell keeping it: it
    /// stops the sshd, and removes its folder, once that input closes (the process ended).
    static SERVED: OnceLock<Result<(SshHost, Mutex<ChildStdin>), String>> = OnceLock::new();

    /// The host every sandbox's git runs on, the sshd started the first time.
    pub(super) fn host() -> Result<SshHost, SandboxError> {
        SERVED
            .get_or_init(|| serve().map_err(|error| error.to_string()))
            .as_ref()
            .map(|(host, _)| host.clone())
            .map_err(|error| SandboxError::from(error.clone()))
    }

    /// Starts an sshd on the loopback address letting this user in by a key of its own, and says
    /// how to reach it: that key, its host's key, no configuration of the user's, connections
    /// shared through a folder of its own, kept a few seconds once idle.
    fn serve() -> Result<(SshHost, Mutex<ChildStdin>), SandboxError> {
        let dir = PathBuf::from(format!("/tmp/egit-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        super::set_mode(&dir, 0o700)?;
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        for key in ["host", "id"] {
            let made = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f", &path(key)])
                .status()?;
            if !made.success() {
                return Err("ssh-keygen failed".into());
            }
        }
        std::fs::copy(dir.join("id.pub"), dir.join("authorized_keys"))?;
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let mut config = String::new();
        writeln!(config, "Port {port}\nListenAddress 127.0.0.1")?;
        writeln!(
            config,
            "HostKey {}\nPidFile {}",
            path("host"),
            path("sshd.pid")
        )?;
        writeln!(
            config,
            "AuthorizedKeysFile {}\nStrictModes no",
            path("authorized_keys")
        )?;
        writeln!(
            config,
            "PasswordAuthentication no\nKbdInteractiveAuthentication no"
        )?;
        writeln!(
            config,
            "PubkeyAuthentication yes\nLogLevel ERROR\nMaxSessions 1000"
        )?;
        writeln!(config, "MaxStartups 1000")?;
        std::fs::write(dir.join("sshd_config"), config)?;
        let host_key = std::fs::read_to_string(dir.join("host.pub"))?;
        std::fs::write(
            dir.join("known_hosts"),
            format!("[127.0.0.1]:{port} {host_key}"),
        )?;

        // The sshd stops, and its folder goes, when this process's end closes the shell's input.
        let sshd = sshd().ok_or("sshd isn't installed")?;
        let mut keeper = Command::new("sh")
            .args([
                "-c",
                "\"$1\" -D -e -f \"$2\" & pid=$!; cat >/dev/null; kill \"$pid\"; wait \"$pid\"; rm -rf -- \"$3\"",
                "sh",
            ])
            .arg(&sshd)
            .arg(dir.join("sshd_config"))
            .arg(&dir)
            .stdin(Stdio::piped())
            .spawn()?;
        let input = keeper
            .stdin
            .take()
            .ok_or("the sshd's keeper has no input")?;
        let until = Instant::now().checked_add(Duration::from_secs(10));
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if until.is_none_or(|until| Instant::now() > until) {
                return Err("the tests' sshd didn't start listening".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let options = [
            "-F",
            "/dev/null",
            "-p",
            &port.to_string(),
            "-i",
            &path("id"),
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("UserKnownHostsFile={}", path("known_hosts")),
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "ControlPersist=5",
        ]
        .map(str::to_owned)
        .to_vec();
        let host = SshHost {
            destination: "127.0.0.1".to_owned(),
            options,
            control: Some(dir),
        };
        let mut input = input;
        input.flush()?;
        Ok((host, Mutex::new(input)))
    }

    /// An installed sshd (it must be started by an absolute path).
    fn sshd() -> Option<PathBuf> {
        ["/usr/sbin/sshd", "/usr/bin/sshd", "/usr/local/sbin/sshd"]
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(Path::to_owned)
    }
}
