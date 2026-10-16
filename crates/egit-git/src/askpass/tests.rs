//! The server and the helper's side together, over a real socket.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use super::client::ask;
use super::protocol::Token;
use super::*;

const WAIT: Duration = Duration::from_secs(10);

/// A private directory under the temporary directory, removed afterwards.
struct Base(PathBuf);

impl Base {
    fn new() -> Self {
        let nonce = Token::random().unwrap().to_hex();
        let path = std::env::temp_dir().join(format!("egit-askpass-test-{}", &nonce[..12]));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The socket and token a helper started by `server` would find in its environment.
fn helper_view(server: &AskpassServer) -> (PathBuf, Token) {
    let environment = server.environment();
    let find = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    let socket = PathBuf::from(find(SOCKET_VAR));
    let token = Token::from_hex(find(TOKEN_VAR).as_encoded_bytes()).unwrap();
    (socket, token)
}

fn start(
    base: &Base,
) -> (
    AskpassServer,
    std::sync::mpsc::Receiver<Prompt>,
    Arc<AtomicUsize>,
) {
    let notified = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&notified);
    let (server, prompts) =
        AskpassServer::start_in(&base.0, Path::new("/usr/bin/egit"), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    (server, prompts, notified)
}

#[test]
fn an_answer_reaches_the_helper() {
    let base = Base::new();
    let (server, prompts, notified) = start(&base);
    let (socket, token) = helper_view(&server);
    let helper = thread::spawn(move || {
        ask(
            &socket,
            &token,
            PromptKind::Secret,
            b"Password: ",
            b"Fetching origin",
        )
        .unwrap()
    });

    let prompt = prompts.recv_timeout(WAIT).unwrap();
    assert_eq!(prompt.kind(), PromptKind::Secret);
    assert_eq!(prompt.text(), "Password: ");
    assert_eq!(prompt.context(), Some("Fetching origin"));
    assert!(!prompt.is_abandoned());
    // The window is woken just after the prompt is handed over.
    let started = Instant::now();
    while notified.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < WAIT, "nobody was woken");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(notified.load(Ordering::SeqCst), 1);
    prompt.answer(b"hunter2").unwrap();

    let answer = helper.join().unwrap();
    assert_eq!(
        answer.as_deref().map(Vec::as_slice),
        Some(b"hunter2".as_slice())
    );
}

#[test]
fn refusing_or_dropping_a_prompt_refuses_it() {
    let base = Base::new();
    let (server, prompts, _) = start(&base);
    for refuse in [true, false] {
        let (socket, token) = helper_view(&server);
        let helper = thread::spawn(move || {
            ask(&socket, &token, PromptKind::Visible, b"Username: ", b"").unwrap()
        });
        let prompt = prompts.recv_timeout(WAIT).unwrap();
        assert_eq!(prompt.context(), None);
        if refuse {
            prompt.refuse().unwrap();
        } else {
            drop(prompt);
        }
        assert_eq!(helper.join().unwrap(), None);
    }
}

#[test]
fn a_helper_without_the_token_is_turned_away() {
    let base = Base::new();
    let (server, prompts, notified) = start(&base);
    let (socket, _) = helper_view(&server);
    let impostor = Token::random().unwrap();
    assert!(ask(&socket, &impostor, PromptKind::Secret, b"Password: ", b"").is_err());
    assert_eq!(
        prompts
            .recv_timeout(Duration::from_millis(100))
            .unwrap_err(),
        RecvTimeoutError::Timeout
    );
    assert_eq!(notified.load(Ordering::SeqCst), 0);
}

#[test]
fn notices_an_asker_that_went_away() {
    let base = Base::new();
    let (server, prompts, _) = start(&base);
    let (socket, token) = helper_view(&server);
    let (leave, left) = std::sync::mpsc::channel::<()>();
    let helper = thread::spawn(move || {
        let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        protocol::write_request(
            &mut stream,
            &token,
            PromptKind::Notice,
            b"Touch your key",
            b"",
        )
        .unwrap();
        left.recv().unwrap();
        // The connection closes here, as when ssh kills the helper.
    });
    let prompt = prompts.recv_timeout(WAIT).unwrap();
    assert_eq!(prompt.kind(), PromptKind::Notice);
    assert!(!prompt.is_abandoned());
    leave.send(()).unwrap();
    helper.join().unwrap();
    let deadline = Instant::now() + WAIT;
    while !prompt.is_abandoned() {
        assert!(
            Instant::now() < deadline,
            "the closed connection went unnoticed"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_environment_points_git_and_ssh_at_the_helper() {
    let base = Base::new();
    let (server, _, _) = start(&base);
    let environment: Vec<(OsString, OsString)> = server.environment();
    let value = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let helper = server.helper().as_os_str().to_owned();
    assert_eq!(value("GIT_ASKPASS"), Some(helper.clone()));
    assert_eq!(value("SSH_ASKPASS"), Some(helper));
    assert_eq!(value("SSH_ASKPASS_REQUIRE"), Some(OsString::from("force")));
    assert_eq!(server.helper().file_name().unwrap(), HELPER_NAME);
    assert_eq!(
        fs::read_link(server.helper()).unwrap(),
        Path::new("/usr/bin/egit")
    );
}

#[test]
fn stopping_removes_the_directory_and_ends_the_prompts() {
    let base = Base::new();
    let (server, prompts, _) = start(&base);
    let dir = server.helper().parent().unwrap().to_owned();
    assert!(dir.is_dir());
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(server);
    assert!(!dir.exists());
    assert_eq!(
        prompts.recv_timeout(WAIT).unwrap_err(),
        RecvTimeoutError::Disconnected
    );
}

#[test]
fn stopping_needs_nothing_on_disk() {
    let base = Base::new();
    let (server, _, _) = start(&base);
    // A temporary-files cleaner, or a sandbox torn down first.
    fs::remove_dir_all(server.helper().parent().unwrap()).unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    thread::spawn(move || {
        drop(server);
        done.send(()).unwrap();
    });
    finished.recv_timeout(WAIT).unwrap();
}

#[test]
fn refuses_a_directory_others_can_enter() {
    let base = Base::new();
    fs::set_permissions(&base.0, fs::Permissions::from_mode(0o755)).unwrap();
    let error = AskpassServer::start_in(&base.0, Path::new("/usr/bin/egit"), || {}).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn sweeps_directories_of_servers_that_are_gone() {
    let base = Base::new();
    // Beyond Linux's largest process id: no such process can exist.
    let stale = base.0.join("999999999-deadbeef");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("socket"), "").unwrap();
    let ours = base.0.join(format!("{}-cafef00d", std::process::id()));
    fs::create_dir(&ours).unwrap();
    let (_server, _, _) = start(&base);
    assert!(!stale.exists());
    assert!(ours.exists(), "a live process's directory was swept");
}
