//! A program a browser runs through the relay: started as egit starts git (leading a process
//! group of its own, with no terminal), fed its standard input as it comes, its output sent as
//! it is printed (no more of its standard output than the browser made room for), and how it
//! ended.
//!
//! Each runs on threads of its own: one starts it and sends its standard output, then waits
//! for it; one sends its standard error; one feeds it, while it reads anything.

use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::path::Path;
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;

use bstr::ByteSlice as _;
use egit_git::relay::protocol::{FromRelay, Missing, Program, WINDOW};
use egit_git::{Detached, Exit, Stopper};
use parking_lot::{Condvar, Mutex};

use crate::session::Out;

/// How much output goes in one message.
const CHUNK: usize = 64 * 1024;

/// A program running for a session.
pub(crate) struct Running {
    /// Where its standard input goes, while it is open.
    input: Option<Sender<Vec<u8>>>,
    /// How much of its standard output the browser has room for.
    room: Arc<Room>,
    /// What stops it, once it started (and asked to stop before then: stopped as it starts).
    stopper: Arc<Mutex<Stopping>>,
}

#[derive(Default)]
struct Stopping {
    stopper: Option<Stopper>,
    asked: bool,
}

/// Room in the browser for a program's standard output.
struct Room {
    left: Mutex<RoomLeft>,
    changed: Condvar,
}

struct RoomLeft {
    bytes: u64,
    /// Nobody takes it in any more: it is read and dropped, so the program isn't held up.
    unlimited: bool,
}

impl Room {
    /// Waits for room, and takes up to `wanted` of it: how much (unlimited, all).
    fn take(&self, wanted: usize) -> Option<usize> {
        let mut left = self.left.lock();
        while left.bytes == 0 && !left.unlimited {
            self.changed.wait(&mut left);
        }
        if left.unlimited {
            return None;
        }
        let taken = usize::try_from(left.bytes)
            .unwrap_or(usize::MAX)
            .min(wanted);
        left.bytes = left
            .bytes
            .saturating_sub(u64::try_from(taken).unwrap_or(u64::MAX));
        drop(left);
        Some(taken)
    }

    fn add(&self, bytes: u32) {
        let mut left = self.left.lock();
        left.bytes = left.bytes.saturating_add(u64::from(bytes));
        drop(left);
        self.changed.notify_all();
    }

    fn open(&self) {
        self.left.lock().unlimited = true;
        self.changed.notify_all();
    }
}

impl Running {
    /// Starts `program` as process `id`, in its own folder (else `home`), with the relay's
    /// environment and `environment` over it, then the program's own; tells `out` how it goes,
    /// and `finished` once it ended.
    pub(crate) fn start(
        id: u32,
        program: Program,
        (environment, home): (&Arc<[(OsString, OsString)]>, &Path),
        out: Out,
        finished: impl FnOnce(u32) + Send + 'static,
    ) -> Self {
        let room = Arc::new(Room {
            left: Mutex::new(RoomLeft {
                bytes: u64::from(WINDOW),
                unlimited: false,
            }),
            changed: Condvar::new(),
        });
        let stopper: Arc<Mutex<Stopping>> = Arc::default();
        let (input, fed) = if program.stdin {
            let (sender, receiver) = mpsc::channel::<Vec<u8>>();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let environment = Arc::clone(environment);
        let home = home.to_path_buf();
        let (shared_room, shared_stopper) = (Arc::clone(&room), Arc::clone(&stopper));
        let started = thread::Builder::new()
            .name(format!("relayed program {id}"))
            .spawn(move || {
                let ended = run(
                    id,
                    &program,
                    (&environment, &home),
                    (fed, &shared_room, &shared_stopper),
                    &out,
                );
                let _sent = out.send(ended.encode());
                finished(id);
            });
        if let Err(error) = started {
            tracing::warn!(%error, "a program's thread couldn't be started");
        }
        Self {
            input,
            room,
            stopper,
        }
    }

    /// Hands `data` to its standard input.
    pub(crate) fn input(&self, data: Vec<u8>) {
        if let Some(input) = &self.input {
            // A program no longer reading gets no more.
            let _fed = input.send(data);
        }
    }

    /// Ends its standard input.
    pub(crate) fn close_input(&mut self) {
        self.input = None;
    }

    /// Makes room for `bytes` more of its standard output.
    pub(crate) fn credit(&self, bytes: u32) {
        self.room.add(bytes);
    }

    /// Stops it, and everything it started (SIGTERM to its group).
    pub(crate) fn terminate(&self) {
        let mut stopping = self.stopper.lock();
        stopping.asked = true;
        if let Some(stopper) = &stopping.stopper {
            stopper.terminate();
        }
    }

    /// Stops it for a session gone: its output read and dropped, its input closed.
    pub(crate) fn abandon(mut self) {
        self.terminate();
        self.room.open();
        self.input = None;
    }
}

/// Runs `program`: what to say once it ended.
fn run(
    id: u32,
    program: &Program,
    (environment, home): (&[(OsString, OsString)], &Path),
    (fed, room, stopping): (Option<mpsc::Receiver<Vec<u8>>>, &Room, &Mutex<Stopping>),
    out: &Out,
) -> FromRelay {
    let dir = program.dir.as_ref().map_or_else(
        || home.to_path_buf(),
        |dir| dir.to_path_lossy().into_owned(),
    );
    if !dir.is_dir() {
        return FromRelay::Failed {
            id,
            missing: Missing::Folder,
            message: format!("{} isn't a directory", dir.display()),
        };
    }
    let mut command = Command::new(program.program.to_os_str_lossy());
    command
        .args(program.args.iter().map(|arg| arg.to_os_str_lossy()))
        .current_dir(&dir)
        .envs(environment.iter().map(|(key, value)| (key, value)));
    // As `env -u … K=V …` does over ssh: what it doesn't get goes first, then what it sets.
    for key in &program.unset {
        command.env_remove(key.to_os_str_lossy());
    }
    command
        .envs(
            program
                .set
                .iter()
                .map(|(key, value)| (key.to_os_str_lossy(), value.to_os_str_lossy())),
        )
        .stdin(if fed.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = match Detached::spawn(&mut command) {
        Ok(process) => process,
        Err(error) => {
            return FromRelay::Failed {
                id,
                missing: if error.kind() == io::ErrorKind::NotFound {
                    Missing::Program
                } else {
                    Missing::Nothing
                },
                message: error.to_string(),
            };
        }
    };
    {
        let mut stopping = stopping.lock();
        stopping.stopper = Some(process.stopper());
        if stopping.asked {
            process.stopper().terminate();
        }
    }
    let child = process.child_mut();
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    if let (Some(mut stdin), Some(fed)) = (stdin, fed) {
        let feeding = thread::Builder::new()
            .name(format!("relayed program {id}'s input"))
            .spawn(move || {
                for chunk in fed {
                    // It stopped reading: what is left goes nowhere.
                    if stdin.write_all(&chunk).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = feeding {
            tracing::warn!(%error, "a program's input couldn't be fed");
        }
    }
    let said = stderr.map(|stderr| {
        let out = out.clone();
        thread::Builder::new()
            .name(format!("relayed program {id}'s errors"))
            .spawn(move || send_errors(id, stderr, &out))
    });
    if let Some(stdout) = stdout {
        send_output(id, stdout, room, out);
    }
    if let Some(Ok(said)) = said {
        let _joined = said.join();
    }
    match process.wait() {
        Ok(status) => FromRelay::Exited {
            id,
            exit: Exit::from(status),
        },
        Err(error) => FromRelay::Failed {
            id,
            missing: Missing::Nothing,
            message: format!("waiting for it failed: {error}"),
        },
    }
}

/// Sends its standard output as it comes, no more than the browser has room for.
fn send_output(id: u32, mut stdout: ChildStdout, room: &Room, out: &Out) {
    let mut buffer = vec![0; CHUNK];
    loop {
        let wanted = room.take(CHUNK);
        let into = buffer
            .get_mut(..wanted.unwrap_or(CHUNK))
            .unwrap_or_default();
        let read = match stdout.read(into) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        // Room taken but not used is given back.
        if let Some(wanted) = wanted {
            room.add(u32::try_from(wanted.saturating_sub(read)).unwrap_or(u32::MAX));
        }
        if wanted.is_some() {
            let data = buffer.get(..read).unwrap_or_default().to_vec();
            if out.send(FromRelay::Stdout { id, data }.encode()).is_err() {
                // The session is gone: read on, so the program isn't held up.
                room.open();
            }
        }
    }
}

/// Sends its standard error as it comes.
fn send_errors(id: u32, mut stderr: ChildStderr, out: &Out) {
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        let data = buffer.get(..read).unwrap_or_default().to_vec();
        // A session gone reads on, so the program isn't held up.
        let _sent = out.send(FromRelay::Stderr { id, data }.encode());
    }
}
