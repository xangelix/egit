//! Windows: git in a process group of its own and without a console window, stopped the way
//! Git for Windows stops a process gently.
//!
//! Windows has no SIGTERM. Git for Windows' own `kill` (its `exit_process`) injects a thread
//! running `ExitProcess` into the process instead: unlike `TerminateProcess`, that runs git's
//! exit handlers, which remove its lock files. A group is stopped so here: the processes git
//! started (hooks, ssh, helpers: found by their parents in a snapshot of the system's
//! processes, each born after its parent, so an id reused since can't pass for one), the
//! deepest first, then git. Where a thread can't be injected (a process of another
//! architecture), it is terminated, as Git for Windows then does too.
//!
//! A process's id stays reserved while a handle to it is open, and egit keeps git's `Child`
//! (and so its handle) until it has reaped it: git's id never names another process while egit
//! may stop or wait on it.
//!
//! Its `unsafe` code is the Win32 calls the standard library doesn't make.

#![expect(
    unsafe_code,
    reason = "the Win32 calls the standard library doesn't make"
)]

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::os::windows::process::CommandExt as _;
use std::process::{Child, Command};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_UNKNOWN,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateRemoteThread, GetCurrentProcess,
    GetProcessTimes, INFINITE, IsWow64Process2, LPTHREAD_START_ROUTINE, OpenProcess,
    PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, PROCESS_VM_OPERATION, PROCESS_VM_READ,
    PROCESS_VM_WRITE, TerminateProcess, WaitForSingleObject,
};

/// A process group: the id of the git that leads it.
pub(crate) type Group = u32;

/// The status a stopped process exits with: 128 + SIGTERM, as git reports one stopped so.
const STOPPED: u32 = 143;

/// Spawns `command` as the leader of a new process group, without a console window.
pub(crate) fn spawn_in_new_session(command: &mut Command) -> io::Result<Child> {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command.spawn()
}

/// The group a spawned git leads (its id is the child's).
pub(crate) fn group_of(child: &Child) -> Group {
    child.id()
}

/// Asks every process in the group to stop, the processes git started first, letting each run
/// its exit handlers (git's remove its lock files).
pub(crate) fn terminate_group(group: Group) {
    let mut stopping = descendants(group);
    stopping.reverse();
    stopping.push(group);
    for id in stopping {
        stop(id);
    }
}

/// Blocks until `id` has exited. Its id stays reserved: the caller holds its `Child`.
pub(crate) fn wait_until_exited(id: Group) -> io::Result<()> {
    let process = open(PROCESS_SYNCHRONIZE, id).ok_or_else(io::Error::last_os_error)?;
    // SAFETY: `process` is an open handle with the right to wait on it.
    match unsafe { WaitForSingleObject(raw(&process), INFINITE) } {
        WAIT_FAILED => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

/// Stops one process: `ExitProcess` run in it, or, where that can't be, terminated.
fn stop(id: u32) {
    let rights = PROCESS_CREATE_THREAD
        | PROCESS_QUERY_INFORMATION
        | PROCESS_VM_OPERATION
        | PROCESS_VM_READ
        | PROCESS_VM_WRITE
        | PROCESS_TERMINATE;
    // Gone already, or not one this user may stop.
    let Some(process) = open(rights, id) else {
        return;
    };
    if let Some(exit) = exit_process().filter(|_| same_architecture(&process)) {
        let argument =
            std::ptr::without_provenance::<c_void>(usize::try_from(STOPPED).unwrap_or(usize::MAX));
        // SAFETY: `process` is open with the rights `CreateRemoteThread` needs; `exit` is
        // `ExitProcess` in kernel32, which every process maps at the same address (checked to be
        // of this process's architecture), and takes the exit status as its one argument, which
        // the thread's parameter carries.
        let thread = unsafe {
            CreateRemoteThread(
                raw(&process),
                std::ptr::null(),
                0,
                Some(exit),
                argument,
                0,
                std::ptr::null_mut(),
            )
        };
        if !thread.is_null() {
            // SAFETY: a thread handle `CreateRemoteThread` just returned, closed once.
            drop(unsafe { OwnedHandle::from_raw_handle(thread) });
            return;
        }
        tracing::debug!(error = %io::Error::last_os_error(), id, "couldn't run ExitProcess in git");
    }
    // SAFETY: `process` is open with the right to terminate it.
    if unsafe { TerminateProcess(raw(&process), STOPPED) } == 0 {
        tracing::warn!(error = %io::Error::last_os_error(), id, "could not stop git");
    }
}

/// `ExitProcess`, as a thread's start: kernel32's, at the address every process has it at.
fn exit_process() -> Option<unsafe extern "system" fn(*mut c_void) -> u32> {
    static EXIT: OnceLock<LPTHREAD_START_ROUTINE> = OnceLock::new();
    *EXIT.get_or_init(|| {
        let name: Vec<u16> = "kernel32.dll".encode_utf16().chain([0]).collect();
        // SAFETY: a NUL-terminated name; kernel32 is loaded in every process, and the handle
        // `GetModuleHandleW` returns isn't to be closed.
        let kernel32 = unsafe { GetModuleHandleW(name.as_ptr()) };
        if kernel32.is_null() {
            return None;
        }
        // SAFETY: a module handle and a NUL-terminated name.
        let found = unsafe { GetProcAddress(kernel32, c"ExitProcess".as_ptr().cast()) }?;
        // SAFETY: `ExitProcess` takes one 32-bit argument and doesn't return: called as a
        // thread's start, it gets the thread's parameter, in the register the first argument
        // goes in either way.
        Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(found)
        })
    })
}

/// Whether `process` runs as this one does (not, say, under emulation of another processor),
/// so this process's address of `ExitProcess` is its own.
fn same_architecture(process: &OwnedHandle) -> bool {
    let machine = |handle: HANDLE| {
        let mut running: IMAGE_FILE_MACHINE = IMAGE_FILE_MACHINE_UNKNOWN;
        let mut native: IMAGE_FILE_MACHINE = IMAGE_FILE_MACHINE_UNKNOWN;
        // SAFETY: an open process handle (or the current process's pseudo handle) and two
        // places to write to.
        let asked = unsafe { IsWow64Process2(handle, &raw mut running, &raw mut native) };
        (asked != 0).then_some(running)
    };
    // SAFETY: the current process's pseudo handle, which needs no closing.
    let ours = machine(unsafe { GetCurrentProcess() });
    ours.is_some() && ours == machine(raw(process))
}

/// The processes `root` started, and theirs, as a snapshot of the system's processes shows them
/// (breadth first).
fn descendants(root: u32) -> Vec<u32> {
    let processes = snapshot();
    let mut found = vec![root];
    let mut at = 0_usize;
    while let Some(&parent) = found.get(at) {
        at = at.saturating_add(1);
        let Some(born) = creation_time(parent) else {
            continue;
        };
        for &(id, of) in &processes {
            // A process whose parent's id was reused since is older than the process now
            // holding it: not one of its children.
            if of == parent
                && id != parent
                && !found.contains(&id)
                && creation_time(id).is_some_and(|child| child >= born)
            {
                found.push(id);
            }
        }
    }
    found.remove(0);
    found
}

/// Every process's id and its parent's.
fn snapshot() -> Vec<(u32, u32)> {
    // SAFETY: flags asking for processes; the handle it returns is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        tracing::debug!(error = %io::Error::last_os_error(), "couldn't list processes");
        return Vec::new();
    }
    // SAFETY: a handle `CreateToolhelp32Snapshot` just returned, closed once.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..PROCESSENTRY32W::default()
    };
    let mut processes = Vec::new();
    // SAFETY: a snapshot handle, and an entry whose size is set.
    let mut more = unsafe { Process32FirstW(raw(&snapshot), &raw mut entry) } != 0;
    while more {
        processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
        // SAFETY: as above.
        more = unsafe { Process32NextW(raw(&snapshot), &raw mut entry) } != 0;
    }
    processes
}

/// When process `id` was created (as `FILETIME`'s parts compare), if it can be asked.
fn creation_time(id: u32) -> Option<(u32, u32)> {
    let process = open(PROCESS_QUERY_LIMITED_INFORMATION, id)?;
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: an open process handle with the right to query it, and four places to write to.
    let asked = unsafe {
        GetProcessTimes(
            raw(&process),
            &raw mut *created,
            &raw mut *exited,
            &raw mut *kernel,
            &raw mut *user,
        )
    };
    (asked != 0).then_some((created.dwHighDateTime, created.dwLowDateTime))
}

/// Process `id` opened with `rights`, if it exists and this user may.
fn open(rights: u32, id: u32) -> Option<OwnedHandle> {
    // SAFETY: plain values; the handle returned, when there is one, is owned here.
    let handle = unsafe { OpenProcess(rights, 0, id) };
    // SAFETY: a handle `OpenProcess` just returned, closed once by its owner.
    (!handle.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// A handle as the Win32 calls take it.
fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}
