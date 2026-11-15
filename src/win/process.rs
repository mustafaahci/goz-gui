//! Processes and threads: processes started, opened and waited for, named
//! mutexes, a named pipe read and written overlapped, services, and this
//! process's and thread's own state. Every kernel handle here is owned, and
//! closed when its owner drops.

use core::marker::PhantomData;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_MORE_DATA, HANDLE,
    WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, THREAD_ERROR_MODE};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Memory::{HeapOptimizeResources, HeapSetInformation};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
    SC_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, GetCurrentProcess, GetCurrentThread, GetExitCodeProcess,
    GetStartupInfoW, GetThreadId, OpenProcess, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_SYNCHRONIZE, ReleaseMutex, STARTF_USESHOWWINDOW, STARTUPINFOW, Sleep,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows_sys::core::BOOL;

use super::sync::{self, Event};
use super::{len_u32, sys, wz};

// ---- This process and thread ----

/// Suspends this thread for `ms` (Sleep).
pub fn sleep(ms: u32) {
    // SAFETY: no pointers.
    unsafe { Sleep(ms) };
}

// kernel32's (windows-sys has it under a feature goz-gui does not build).
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> BOOL;
}

/// The CPU cycles this thread has used so far (QueryThreadCycleTime); 0 if
/// the system would not say.
pub fn thread_cycles() -> u64 {
    let mut cycles = 0u64;
    // SAFETY: the thread's own pseudo-handle and an out-value.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
    cycles
}

/// The system's id of the thread `thread` runs on (GetThreadId), for a
/// message posted to its queue.
pub fn thread_id_of<T>(thread: &std::thread::JoinHandle<T>) -> u32 {
    // SAFETY: the thread's handle, which the JoinHandle owns and keeps open
    // for the call.
    unsafe { GetThreadId(thread.as_raw_handle() as HANDLE) }
}

/// Sets this thread's error mode (SEM_FAILCRITICALERRORS: no dialog for a
/// drive without a disk); false if it could not.
pub fn set_thread_error_mode(mode: THREAD_ERROR_MODE) -> bool {
    // SAFETY: no old mode asked for (null is allowed).
    unsafe { SetThreadErrorMode(mode, core::ptr::null_mut()) != 0 }
}

/// Gives what the process's heaps hold free back to Windows
/// (HeapSetInformation's HeapOptimizeResources, which any thread may ask).
pub fn optimize_heaps() {
    /// `HEAP_OPTIMIZE_RESOURCES_INFORMATION`.
    #[repr(C)]
    struct Optimize {
        version: u32,
        flags: u32,
    }
    let info = Optimize {
        version: 1,
        flags: 0,
    };
    // SAFETY: no heap handle (all of the process's); the documented
    // structure, version 1, and its size, read during the call.
    unsafe {
        HeapSetInformation(
            core::ptr::null_mut(),
            HeapOptimizeResources,
            (&raw const info).cast(),
            size_of::<Optimize>(),
        )
    };
}

/// This process's memory counters (K32GetProcessMemoryInfo).
pub fn memory() -> Option<PROCESS_MEMORY_COUNTERS_EX> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct whose size is passed, for this process (the
    // pseudo-handle GetCurrentProcess gives, which needs no closing).
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
            c.cb,
        )
    };
    (ok != 0).then_some(c)
}

/// How the program that started this process asked for its first window
/// to be shown (STARTUPINFO's wShowWindow), if it asked.
pub fn startup_show_window() -> Option<u16> {
    let mut si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size; only its flags and show command
    // are read (its strings are the process's, not kept).
    unsafe { GetStartupInfoW(&mut si) };
    (si.dwFlags & STARTF_USESHOWWINDOW != 0).then_some(si.wShowWindow)
}

/// Lets process `pid` bring its window forward (AllowSetForegroundWindow).
pub fn allow_set_foreground(pid: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { AllowSetForegroundWindow(pid) != 0 }
}

// ---- Processes ----

/// Starts the program at `exe` (by its exact path) with the command line
/// `line`, its first window shown as `show` (a SW_*), with CREATE_* `flags`
/// and no handles inherited, in this process's environment and folder.
/// With `front` it may take the foreground. The process's handles are
/// closed here; the error is the system's.
pub fn create(
    exe: &Path,
    line: &str,
    show: u16,
    flags: PROCESS_CREATION_FLAGS,
    front: bool,
) -> io::Result<()> {
    let app: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // CreateProcessW may write to the command line: a buffer of its own.
    let mut line = wz(line);
    let si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: show,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: NUL-terminated strings (the command line in a buffer of its
    // own the call may write to), no security attributes, no handles
    // inherited, this process's environment and folder; an in-struct with
    // its size and an out-struct.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            flags,
            core::ptr::null(),
            core::ptr::null(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if front {
        allow_set_foreground(pi.dwProcessId);
    }
    // SAFETY: the handles CreateProcessW gave back, owned by nothing else,
    // closed once (as they drop here).
    unsafe {
        drop(OwnedHandle::from_raw_handle(pi.hThread));
        drop(OwnedHandle::from_raw_handle(pi.hProcess));
    }
    Ok(())
}

/// A process's handle, closed when this drops: one the shell gave
/// ([`super::shell::execute`]), or one opened to wait for
/// ([`Process::open`]).
pub struct Process(OwnedHandle);

impl Process {
    /// Takes `handle`, a process's handle owned by nothing else.
    pub(crate) fn own(handle: OwnedHandle) -> Process {
        Process(handle)
    }

    /// Process `pid`, opened for waiting only (PROCESS_SYNCHRONIZE); `None`
    /// for one gone already (or one this process may not wait for).
    pub fn open(pid: u32) -> Option<Process> {
        // SAFETY: no pointers.
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if h.is_null() {
            return None;
        }
        // SAFETY: the handle just opened, owned by nothing else.
        Some(Process(unsafe { OwnedHandle::from_raw_handle(h) }))
    }

    fn handle(&self) -> HANDLE {
        self.0.as_raw_handle() as HANDLE
    }

    /// Waits up to `ms` (INFINITE: no limit) for the process to end;
    /// whether it did.
    pub fn wait(&self, ms: u32) -> bool {
        // SAFETY: the handle this owns.
        unsafe { WaitForSingleObject(self.handle(), ms) == WAIT_OBJECT_0 }
    }

    /// The process's exit code (STILL_ACTIVE while it runs); `None` if the
    /// system would not say (as for a handle opened only to wait).
    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: the handle this owns, and an out-value.
        let ok = unsafe { GetExitCodeProcess(self.handle(), &mut code) } != 0;
        ok.then_some(code)
    }
}

// ---- Named mutexes ----

/// A named mutex (CreateMutexW), its handle closed when this drops.
pub struct NamedMutex(OwnedHandle);

impl NamedMutex {
    /// Makes the mutex `name` (not owned), or opens the one there is: with
    /// whether it was there already. `None` if it could be neither (one made
    /// by a process this one may not open, say).
    pub fn create(name: &str) -> Option<(NamedMutex, bool)> {
        let name = wz(name);
        // SAFETY: no security attributes; a NUL-terminated name.
        let h = unsafe { CreateMutexW(core::ptr::null(), 0, name.as_ptr()) };
        let existed = sys::last_error() == ERROR_ALREADY_EXISTS;
        if h.is_null() {
            return None;
        }
        // SAFETY: the handle just made, owned by nothing else.
        Some((
            NamedMutex(unsafe { OwnedHandle::from_raw_handle(h) }),
            existed,
        ))
    }

    /// Waits up to `ms` (INFINITE: no limit) to own the mutex, as this
    /// thread; one abandoned by a thread that ended owning it is owned too.
    /// The mutex back when it was not had in time.
    pub fn lock(self, ms: u32) -> Result<OwnedMutex, NamedMutex> {
        // SAFETY: the handle this owns.
        match unsafe { WaitForSingleObject(self.0.as_raw_handle() as HANDLE, ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(OwnedMutex {
                mutex: self,
                _thread: PhantomData,
            }),
            _ => Err(self),
        }
    }
}

/// A named mutex this thread owns: given back (ReleaseMutex), then closed,
/// when this drops, on the thread that owns it (it is not `Send`).
pub struct OwnedMutex {
    mutex: NamedMutex,
    _thread: PhantomData<*mut ()>,
}

impl Drop for OwnedMutex {
    fn drop(&mut self) {
        // SAFETY: the handle the mutex owns, on the thread that owns the
        // mutex; the handle closes after, with `mutex`.
        unsafe { ReleaseMutex(self.mutex.0.as_raw_handle() as HANDLE) };
    }
}

// ---- A pipe, read and written overlapped ----

/// A file (a named pipe) read and written with overlapped I/O, one
/// transfer at a time. Each transfer is finished (or cancelled and waited
/// for) before the call that started it returns, unwinding included: the
/// OVERLAPPED, the event it signals and the buffer read into are this
/// type's own, and the system is done with them by then.
pub struct OverlappedFile {
    file: File,
    /// Signalled as each transfer finishes; nothing else can set it.
    event: Event,
    buf: Vec<u8>,
}

/// A transfer in flight, lent to the code that waits for it.
pub struct InFlight<'a> {
    event: &'a Event,
}

impl InFlight<'_> {
    /// Waits up to `ms` (INFINITE: no limit) for the transfer to finish or
    /// for one of `others` to be set: 0 for the transfer, `i + 1` for
    /// `others[i]`; `None` on a timeout or failure.
    pub fn wait(&self, others: &[&Event], ms: u32) -> Option<usize> {
        let mut all = Vec::with_capacity(others.len() + 1);
        all.push(self.event);
        all.extend_from_slice(others);
        sync::wait_any(&all, false, ms)
    }
}

/// A transfer the system may still be using `ov` for: cancelled and waited
/// for when this drops while `armed`.
struct Pending {
    handle: HANDLE,
    ov: *mut OVERLAPPED,
    armed: bool,
}

impl Pending {
    /// Cancels the transfer and waits for the system to be done with it.
    fn abort(&mut self) {
        if !self.armed {
            return;
        }
        let mut n = 0u32;
        // SAFETY: the transfer started on `handle` with `ov`, which is alive
        // (its owner's frame is below this); after the wait the system no
        // longer uses it or its buffer.
        unsafe {
            CancelIoEx(self.handle, self.ov);
            while GetOverlappedResult(self.handle, self.ov, &mut n, 1) == 0
                && sys::last_error() == ERROR_IO_INCOMPLETE
            {}
        }
        self.armed = false;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.abort();
    }
}

impl OverlappedFile {
    /// `file` (opened with FILE_FLAG_OVERLAPPED, as a pipe of gozd's is),
    /// read `buf_len` bytes at most at a time. `None` if no event could be
    /// made for it.
    pub fn new(file: File, buf_len: usize) -> Option<OverlappedFile> {
        Some(OverlappedFile {
            file,
            event: Event::new(true, false)?,
            buf: vec![0; buf_len],
        })
    }

    fn handle(&self) -> HANDLE {
        self.file.as_raw_handle() as HANDLE
    }

    /// What the last [`OverlappedFile::read`] put in the buffer: its first
    /// `n` bytes (the read's count).
    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    /// Writes some of `data` (WriteFile) and waits for it: how much was
    /// written, or why not.
    pub fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let handle = self.handle();
        let mut slot = OVERLAPPED {
            hEvent: self.event.handle(),
            ..Default::default()
        };
        let ov: *mut OVERLAPPED = &raw mut slot;
        // SAFETY: `data` and `ov` outlive the write: it is waited for below,
        // before either goes.
        let ok = unsafe {
            WriteFile(
                handle,
                super::bytes_ptr(data),
                len_u32(data.len()),
                core::ptr::null_mut(),
                ov,
            )
        } != 0;
        if !ok && sys::last_error() != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        let mut n = 0u32;
        loop {
            // SAFETY: waits for the write on `ov` to finish.
            if unsafe { GetOverlappedResult(handle, ov, &mut n, 1) } != 0 {
                return Ok(n as usize);
            }
            // Only the system sets the event, so a wait comes back
            // unfinished never; were it to, it is waited for again.
            if sys::last_error() != ERROR_IO_INCOMPLETE {
                return Err(io::Error::last_os_error());
            }
        }
    }

    /// Reads into the buffer (ReadFile; [`OverlappedFile::buffer`] has what
    /// came). While the read is in flight `wait` waits for it: `Ok` once it
    /// finished, or an error to give it up, and then it is cancelled and
    /// waited for, and the error is this call's. Otherwise how many bytes
    /// came (a message larger than the buffer gives what fits), or why none.
    pub fn read<E>(
        &mut self,
        wait: impl FnOnce(&InFlight) -> Result<(), E>,
    ) -> Result<io::Result<usize>, E> {
        let handle = self.handle();
        let mut slot = OVERLAPPED {
            hEvent: self.event.handle(),
            ..Default::default()
        };
        let ov: *mut OVERLAPPED = &raw mut slot;
        let len = len_u32(self.buf.len());
        // SAFETY: the buffer (this call's through `&mut self`) and `ov`
        // outlive the read: every way out below finishes it, or `pending`
        // cancels and waits for it, unwinding too.
        let ok = unsafe {
            ReadFile(
                handle,
                self.buf.as_mut_ptr(),
                len,
                core::ptr::null_mut(),
                ov,
            )
        } != 0;
        let mut pending = Pending {
            handle,
            ov,
            armed: false,
        };
        if !ok {
            match sys::last_error() {
                ERROR_IO_PENDING => {
                    pending.armed = true;
                    if let Err(e) = wait(&InFlight { event: &self.event }) {
                        pending.abort();
                        return Err(e);
                    }
                }
                ERROR_MORE_DATA => {}
                _ => return Ok(Err(io::Error::last_os_error())),
            }
        }
        let mut n = 0u32;
        // SAFETY: the read on `ov`, asked how it ended.
        if unsafe { GetOverlappedResult(handle, ov, &mut n, 0) } == 0 {
            let err = sys::last_error();
            if err == ERROR_IO_INCOMPLETE {
                // `wait` came back before the read finished.
                pending.abort();
                return Ok(Err(io::Error::from_raw_os_error(err as i32)));
            }
            pending.armed = false;
            if err != ERROR_MORE_DATA {
                return Ok(Err(io::Error::from_raw_os_error(err as i32)));
            }
        }
        pending.armed = false;
        Ok(Ok(n as usize))
    }
}

/// Waits up to `ms` for an instance of the pipe `name` to be free
/// (WaitNamedPipeW); whether one was.
pub fn wait_named_pipe(name: &str, ms: u32) -> bool {
    let name = wz(name);
    // SAFETY: a NUL-terminated name.
    unsafe { WaitNamedPipeW(name.as_ptr(), ms) != 0 }
}

// ---- Services ----

/// The service manager or a service (SC_HANDLE), closed when this drops.
pub struct ServiceHandle(SC_HANDLE);

impl ServiceHandle {
    /// This computer's service manager, with `access` (SC_MANAGER_*).
    pub fn manager(access: u32) -> Option<ServiceHandle> {
        // SAFETY: this computer's active database (nulls).
        let h = unsafe { OpenSCManagerW(core::ptr::null(), core::ptr::null(), access) };
        (!h.is_null()).then_some(ServiceHandle(h))
    }

    /// The service `name` of this manager, with `access` (SERVICE_*).
    pub fn service(&self, name: &str, access: u32) -> Option<ServiceHandle> {
        let name = wz(name);
        // SAFETY: the handle this owns, a NUL-terminated name.
        let h = unsafe { OpenServiceW(self.0, name.as_ptr(), access) };
        (!h.is_null()).then_some(ServiceHandle(h))
    }

    /// The service's command line (its configuration's binary path); `None`
    /// if it cannot be read (or this is no service opened with
    /// SERVICE_QUERY_CONFIG).
    pub fn binary_path(&self) -> Option<String> {
        let mut needed = 0u32;
        // SAFETY: asks for the size only.
        unsafe { QueryServiceConfigW(self.0, core::ptr::null_mut(), 0, &mut needed) };
        // u64s: aligned for QUERY_SERVICE_CONFIGW, whose strings point into
        // the buffer after it.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let cfg = buf.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        // SAFETY: the buffer and its size in bytes.
        let ok = unsafe { QueryServiceConfigW(self.0, cfg, len_u32(buf.len() * 8), &mut needed) };
        if ok == 0 {
            return None;
        }
        // SAFETY: the structure the call filled, its path NUL-terminated in
        // the buffer (or null), read while the buffer lives.
        unsafe {
            let p = (*cfg).lpBinaryPathName;
            if p.is_null() {
                return None;
            }
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            Some(String::from_utf16_lossy(core::slice::from_raw_parts(
                p, len,
            )))
        }
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: the handle this owns, closed once.
        unsafe { CloseServiceHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_mutex_is_owned_once_per_holder() {
        let name = format!("Local\\goz-gui-test-process-{}", std::process::id());
        let (first, existed) = NamedMutex::create(&name).expect("made");
        assert!(!existed);
        let (second, existed) = NamedMutex::create(&name).expect("opened");
        assert!(existed);
        let owned = first.lock(0).ok().expect("free");
        // Another thread waits in vain while this one owns it.
        let busy = std::thread::spawn(move || second.lock(0).is_err())
            .join()
            .unwrap();
        assert!(busy);
        drop(owned);
    }

    #[test]
    fn a_pipe_is_written_and_read_overlapped() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        };
        let name = format!(r"\\.\pipe\goz-gui-test-process-{}", std::process::id());
        let wide = wz(&name);
        // SAFETY: a NUL-terminated name; the handle is owned below.
        let server = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                core::ptr::null(),
            )
        };
        assert_ne!(server, windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
        // SAFETY: the pipe just made, owned by nothing else.
        let server = File::from(unsafe { OwnedHandle::from_raw_handle(server) });
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&name)
            .expect("client");
        let mut pipe = OverlappedFile::new(client, 16).expect("event");
        assert_eq!(pipe.write(b"hello").expect("written"), 5);
        let mut got = [0u8; 5];
        std::io::Read::read_exact(&mut &server, &mut got).expect("read");
        assert_eq!(&got, b"hello");
        // Nothing to read: the wait gives up, and the read is cancelled.
        let gave_up = pipe.read(|io| match io.wait(&[], 20) {
            Some(0) => Ok(()),
            _ => Err("silent"),
        });
        assert!(matches!(gave_up, Err("silent")));
        std::io::Write::write_all(&mut &server, b"back").expect("written");
        let n = pipe
            .read(|io| io.wait(&[], 5000).map(drop).ok_or("silent"))
            .expect("waited")
            .expect("read");
        assert_eq!(&pipe.buffer()[..n], b"back");
    }

    #[test]
    fn this_process_is_there() {
        assert!(memory().is_some_and(|m| m.WorkingSetSize > 0));
        let me = Process::open(sys::process_id()).expect("opened");
        assert!(!me.wait(0), "still running");
        assert!(Process::open(0).is_none());
    }
}
