//! Batch 8 of the move onto `win`: the gozd client's pipe, the clipboard,
//! launching and the shell's item lists, file operations, named mutexes,
//! the tray, files' attributes, locale formats, services and small helpers.

use core::marker::PhantomData;
use core::ptr::NonNull;
use std::borrow::Cow;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FileOperation as FileOperationClass, IFileOperation,
    IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
};
use windows::core::PCWSTR as WPCWSTR;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_MORE_DATA, FILETIME,
    GlobalFree, HANDLE, HWND, LPARAM, LocalFree, RECT, SYSTEMTIME, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Globalization::{
    COMPARESTRING_RESULT, CPINFO, CompareStringOrdinal, GetCPInfo, GetDateFormatEx,
    GetLocaleInfoEx, GetTimeFormatEx, IsDBCSLeadByteEx, MultiByteToWideChar,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, EnumDisplayMonitors, HBITMAP, HDC, HMONITOR,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileAttributesExW, GetFileAttributesW, GetFileExInfoStandard, INVALID_FILE_ATTRIBUTES,
    MOVE_FILE_FLAGS, MoveFileExW, ReadFile, SetFileAttributesW, WIN32_FILE_ATTRIBUTE_DATA,
    WriteFile,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows_sys::Win32::System::DataExchange::{
    COPYDATASTRUCT, CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, HeapOptimizeResources,
    HeapSetInformation,
};
use windows_sys::Win32::System::Ole::{CF_HDROP, CF_TEXT, CF_UNICODETEXT};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
    SC_HANDLE,
};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemDirectoryW, GetSystemTimePreciseAsFileTime,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentThread, GetExitCodeProcess, GetThreadId, ReleaseMutex, Sleep,
    WaitForSingleObject,
};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{
    CommandLineToArgvW, DragQueryFileW, HDROP, ILClone, ILCreateFromPathW, ILFindChild,
    ILFindLastID, ILFree, ILIsEqual, ILIsParent, ILRemoveLastID, NIF_MESSAGE, NOTIFY_ICON_MESSAGE,
    NOTIFYICONDATAW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHARD_PATHW, SHAddToRecentDocs,
    SHCreateDataObject, SHELLEXECUTEINFOW, SHFILEINFOW, SHGFI_DISPLAYNAME, SHGFI_FLAGS,
    SHGFI_LARGEICON, SHGFI_OPENICON, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_TYPENAME,
    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetKnownFolderPath, SHMultiFileProperties,
    SHOP_FILEPATH, SHObjectProperties, SHOpenFolderAndSelectItems, Shell_NotifyIconW,
    ShellExecuteExW, StrFormatByteSizeW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, AllowSetForegroundWindow, CreateAcceleratorTableW, DestroyAcceleratorTable, HACCEL,
    HICON, LoadIconW, RegisterWindowMessageW, SHOW_WINDOW_CMD, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WM_APP, WM_COPYDATA,
};
use windows_sys::core::{BOOL, GUID, PCWSTR};

use crate::win::msg::APP_END;
use crate::win::sync::{self, Event};
use crate::win::sys;

/// `units` as a C string: as they are when they end with a NUL, else a
/// copy with one.
fn nul_ended(units: &[u16]) -> Cow<'_, [u16]> {
    if units.last() == Some(&0) {
        Cow::Borrowed(units)
    } else {
        Cow::Owned(crate::win::wz_units(units))
    }
}

/// A slice's length as the `i32` count a call takes, cut to fit (the call
/// then reads no further than the slice).
fn count_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

fn count_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

// ---- Threads and time ----

/// Suspends this thread for `ms` (Sleep).
pub fn sleep(ms: u32) {
    // SAFETY: no pointers.
    unsafe { Sleep(ms) };
}

/// The system's precise time now (GetSystemTimePreciseAsFileTime): 100 ns
/// since 1601, as a FILETIME counts.
pub fn precise_time() -> u64 {
    let mut ft = FILETIME::default();
    // SAFETY: an out-struct.
    unsafe { GetSystemTimePreciseAsFileTime(&mut ft) };
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
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

/// Initializes COM on this thread as a single-threaded apartment with OLE
/// 1.0's DDE left on (COINIT_APARTMENTTHREADED alone), for the rest of the
/// thread's life: it is never uninitialized, so nothing made in it can
/// outlive it. False if the thread has an apartment of another kind.
pub fn sta_with_ole1dde() -> bool {
    // SAFETY: no pointers (the reserved one is null); never balanced, which
    // only keeps COM loaded on the thread.
    unsafe { CoInitializeEx(core::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0 }
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
    // SAFETY: the documented structure, version 1, and its size; each heap
    // is locked meanwhile.
    unsafe {
        HeapSetInformation(
            core::ptr::null_mut(),
            HeapOptimizeResources,
            (&raw const info).cast(),
            size_of::<Optimize>(),
        )
    };
}

/// `units` as the bytes they are in memory (native order), for a byte
/// search over UTF-16 text.
pub fn units_as_bytes(units: &[u16]) -> &[u8] {
    // SAFETY: u16 has no padding and u8's alignment is 1; the bytes are
    // the slice's own, borrowed for as long as it is.
    unsafe { core::slice::from_raw_parts(units.as_ptr().cast::<u8>(), size_of_val(units)) }
}

// ---- Kernel objects ----

/// A named mutex (CreateMutexW), its handle closed when this drops.
pub struct NamedMutex(OwnedHandle);

impl NamedMutex {
    /// Makes the mutex `name` (not owned), or opens the one there is: with
    /// whether it was there already. `None` if it could be neither (one made
    /// by a process this one may not open, say).
    pub fn create(name: &str) -> Option<(NamedMutex, bool)> {
        let name = crate::win::wz(name);
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

/// A process's handle (one the shell gave), closed when this drops.
pub struct Process(OwnedHandle);

impl Process {
    /// Waits up to `ms` (INFINITE: no limit) for the process to end;
    /// whether it did.
    pub fn wait(&self, ms: u32) -> bool {
        // SAFETY: the handle this owns.
        unsafe { WaitForSingleObject(self.0.as_raw_handle() as HANDLE, ms) == WAIT_OBJECT_0 }
    }

    /// The process's exit code (STILL_ACTIVE while it runs); `None` if the
    /// system would not say.
    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: the handle this owns, and an out-value.
        let ok = unsafe { GetExitCodeProcess(self.0.as_raw_handle() as HANDLE, &mut code) } != 0;
        ok.then_some(code)
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
                data.as_ptr(),
                count_u32(data.len()),
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
        let len = count_u32(self.buf.len());
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
    let name = crate::win::wz(name);
    // SAFETY: a NUL-terminated name.
    unsafe { WaitNamedPipeW(name.as_ptr(), ms) != 0 }
}

// ---- Files ----

/// The attributes of the file at `path` (GetFileAttributesW);
/// INVALID_FILE_ATTRIBUTES when there is none or it cannot be read.
pub fn file_attributes(path: &[u16]) -> u32 {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(path.as_ptr()) }
}

/// Sets the attributes of the file at `path`; false if it could not.
pub fn set_file_attributes(path: &[u16], attributes: u32) -> bool {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path.
    unsafe { SetFileAttributesW(path.as_ptr(), attributes) != 0 }
}

/// The attributes and dates of the file at `path`
/// (GetFileAttributesExW: no handle is opened); `None` when they cannot be
/// read.
pub fn file_attribute_data(path: &[u16]) -> Option<WIN32_FILE_ATTRIBUTE_DATA> {
    let path = nul_ended(path);
    let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: a NUL-terminated path and an out-struct of the level's type.
    let ok = unsafe {
        GetFileAttributesExW(path.as_ptr(), GetFileExInfoStandard, (&raw mut data).cast())
    } != 0;
    ok.then_some(data)
}

/// Moves (renames) `from` to `to` as MoveFileExW does with `flags`.
pub fn move_file(from: &[u16], to: &[u16], flags: MOVE_FILE_FLAGS) -> io::Result<()> {
    let (from, to) = (nul_ended(from), nul_ended(to));
    // SAFETY: two NUL-terminated paths.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// ---- Locale ----

/// GetLocaleInfoEx of the user's locale for `kind` into `buf`: the units
/// written, its NUL included (0 when it failed, or `buf` is too small).
pub fn user_locale_info(kind: u32, buf: &mut [u16]) -> i32 {
    // SAFETY: a null locale name is LOCALE_NAME_USER_DEFAULT; the buffer
    // and its length.
    unsafe {
        GetLocaleInfoEx(
            core::ptr::null(),
            kind,
            buf.as_mut_ptr(),
            count_i32(buf.len()),
        )
    }
}

/// A FILETIME as a SYSTEMTIME (both UTC); `None` if it is out of range.
pub fn file_time_to_system_time(ft: &FILETIME) -> Option<SYSTEMTIME> {
    let mut st = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct.
    (unsafe { FileTimeToSystemTime(ft, &mut st) } != 0).then_some(st)
}

/// A UTC time in the current time zone; `None` if it cannot be.
pub fn to_local_time(utc: &SYSTEMTIME) -> Option<SYSTEMTIME> {
    let mut local = SYSTEMTIME::default();
    // SAFETY: a null zone is the current one; an in-struct and an
    // out-struct.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(core::ptr::null(), utc, &mut local) } != 0;
    ok.then_some(local)
}

/// `st`'s date as the user's locale writes it with `flags` (DATE_SHORTDATE…)
/// into `buf`: the units written, its NUL included (0 when it failed).
pub fn date_format(flags: u32, st: &SYSTEMTIME, buf: &mut [u16]) -> i32 {
    // SAFETY: the user's locale, its own format and calendar (nulls); an
    // in-struct; the buffer and its length.
    unsafe {
        GetDateFormatEx(
            core::ptr::null(),
            flags,
            st,
            core::ptr::null(),
            buf.as_mut_ptr(),
            count_i32(buf.len()),
            core::ptr::null(),
        )
    }
}

/// `st`'s time as the user's locale writes it with `flags` (TIME_NOSECONDS…)
/// into `buf`: the units written, its NUL included (0 when it failed).
pub fn time_format(flags: u32, st: &SYSTEMTIME, buf: &mut [u16]) -> i32 {
    // SAFETY: the user's locale and its own format (nulls); an in-struct;
    // the buffer and its length.
    unsafe {
        GetTimeFormatEx(
            core::ptr::null(),
            flags,
            st,
            core::ptr::null(),
            buf.as_mut_ptr(),
            count_i32(buf.len()),
        )
    }
}

/// `n` bytes as Explorer writes a size (StrFormatByteSizeW).
pub fn format_byte_size(n: i64) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length.
    unsafe { StrFormatByteSizeW(n, buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(crate::win::until_nul(&buf))
}

/// What code page `cp` is (GetCPInfo); `None` if it is not one.
pub fn code_page_info(cp: u32) -> Option<CPINFO> {
    let mut info = CPINFO::default();
    // SAFETY: an out-struct of the right type.
    (unsafe { GetCPInfo(cp, &mut info) } != 0).then_some(info)
}

/// Whether `byte` starts a two-byte character in code page `cp`.
pub fn is_dbcs_lead_byte(cp: u32, byte: u8) -> bool {
    // SAFETY: no pointers.
    unsafe { IsDBCSLeadByteEx(cp, byte) != 0 }
}

/// `src` decoded from code page `cp` (MultiByteToWideChar with `flags`)
/// into `out`: how many units were written (0 when it failed, or `out` is
/// too small for them all).
pub fn multi_byte_to_wide(cp: u32, flags: u32, src: &[u8], out: &mut [u16]) -> usize {
    // SAFETY: the source and its length, the room and its length.
    let n = unsafe {
        MultiByteToWideChar(
            cp,
            flags,
            src.as_ptr(),
            count_i32(src.len()),
            out.as_mut_ptr(),
            count_i32(out.len()),
        )
    };
    usize::try_from(n).unwrap_or(0)
}

/// `a` against `b` by their units (CompareStringOrdinal), case aside when
/// `ignore_case`: CSTR_LESS_THAN, CSTR_EQUAL or CSTR_GREATER_THAN (0 when
/// it failed).
pub fn compare_ordinal(a: &[u16], b: &[u16], ignore_case: bool) -> COMPARESTRING_RESULT {
    // SAFETY: two buffers, each read for its own length.
    unsafe {
        CompareStringOrdinal(
            a.as_ptr(),
            count_i32(a.len()),
            b.as_ptr(),
            count_i32(b.len()),
            ignore_case.into(),
        )
    }
}

// ---- The clipboard ----

/// The clipboard, opened by this thread (OpenClipboard): closed when this
/// drops, on this thread (it is not `Send`).
pub struct Clipboard {
    _thread: PhantomData<*mut ()>,
}

/// Whether the clipboard's data in `format` is a global memory block,
/// which is what [`Clipboard::data`] reads and [`Clipboard::set`] gives:
/// text, file drops, and the registered formats (others, such as bitmaps,
/// are handles of other kinds).
fn is_global_format(format: u32) -> bool {
    [CF_TEXT, CF_UNICODETEXT, CF_HDROP]
        .iter()
        .any(|&f| u32::from(f) == format)
        || (0xc000..=0xffff).contains(&format)
}

impl Clipboard {
    /// Opens the clipboard for `hwnd` (null: this thread); `None` when
    /// another window has it open.
    pub fn open(hwnd: HWND) -> Option<Clipboard> {
        // SAFETY: no pointers; it is closed when the guard drops.
        (unsafe { OpenClipboard(hwnd) } != 0).then_some(Clipboard {
            _thread: PhantomData,
        })
    }

    /// Empties it, making the window that opened it its owner.
    pub fn empty(&self) -> bool {
        // SAFETY: no pointers; the clipboard is open.
        unsafe { EmptyClipboard() != 0 }
    }

    /// A copy of its data in `format` (text, CF_HDROP or a registered
    /// format; see [`is_global_format`]); `None` when it holds none.
    pub fn data(&self, format: u32) -> Option<Vec<u8>> {
        if !is_global_format(format) {
            return None;
        }
        // SAFETY: the clipboard is open; the handle stays its own while it
        // is.
        let mem = unsafe { GetClipboardData(format) };
        if mem.is_null() {
            return None;
        }
        // SAFETY: a global memory block (the format's kind), locked while
        // it is copied out, no further than its size, and unlocked.
        unsafe {
            let p = GlobalLock(mem) as *const u8;
            if p.is_null() {
                return None;
            }
            let bytes = core::slice::from_raw_parts(p, GlobalSize(mem)).to_vec();
            GlobalUnlock(mem);
            Some(bytes)
        }
    }

    /// The paths of the files on it (CF_HDROP); empty when it holds none.
    pub fn files(&self) -> Vec<Vec<u16>> {
        // SAFETY: the clipboard is open; the handle stays its own while it
        // is.
        let drop = unsafe { GetClipboardData(u32::from(CF_HDROP)) } as HDROP;
        if drop.is_null() {
            return Vec::new();
        }
        // SAFETY: the clipboard's CF_HDROP, valid while it is open (for
        // the call).
        unsafe { hdrop_paths(drop) }
    }

    /// Puts `data` on it as `format` (text, CF_HDROP or a registered
    /// format); false if it could not.
    pub fn set(&self, format: u32, data: &[u8]) -> bool {
        if !is_global_format(format) {
            return false;
        }
        // SAFETY: the block is sized for `data` and filled while locked; the
        // clipboard owns it once SetClipboardData succeeds, and it is freed
        // here otherwise.
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
            if mem.is_null() {
                return false;
            }
            let dst = GlobalLock(mem) as *mut u8;
            let mut ok = false;
            if !dst.is_null() {
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
                GlobalUnlock(mem);
                ok = !SetClipboardData(format, mem).is_null();
            }
            if !ok {
                GlobalFree(mem);
            }
            ok
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: closes the clipboard this guard opened, on its thread.
        unsafe { CloseClipboard() };
    }
}

/// The clipboard's sequence number: it changes with anything put on it.
pub fn clipboard_sequence() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetClipboardSequenceNumber() }
}

/// Whether the clipboard holds `format` (it need not be open).
pub fn clipboard_has(format: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { IsClipboardFormatAvailable(format) != 0 }
}

/// The clipboard format registered as `name` (made if new); 0 if it could
/// not be.
pub fn register_clipboard_format(name: &str) -> u32 {
    let name = crate::win::wz(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegisterClipboardFormatW(name.as_ptr()) }
}

/// The paths of a CF_HDROP block (a drop's, or the clipboard's).
///
/// # Safety
/// `drop` is a valid HDROP for as long as the call runs.
pub(crate) unsafe fn hdrop_paths(drop: HDROP) -> Vec<Vec<u16>> {
    // SAFETY: as the caller promises; each buffer is sized by the length
    // DragQueryFileW reports, its NUL included.
    unsafe {
        let n = DragQueryFileW(drop, u32::MAX, core::ptr::null_mut(), 0);
        (0..n)
            .filter_map(|i| {
                let len = DragQueryFileW(drop, i, core::ptr::null_mut(), 0) as usize;
                if len == 0 {
                    return None;
                }
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                buf.truncate(got);
                (got > 0).then_some(buf)
            })
            .collect()
    }
}

// ---- The shell ----

/// The shell's name and icon for a file named `name` with attributes
/// `attrs`, by its name alone (SHGFI_USEFILEATTRIBUTES: nothing is read
/// from the disk). `flags` may ask for SHGFI_TYPENAME, SHGFI_DISPLAYNAME,
/// SHGFI_SYSICONINDEX with SHGFI_SMALLICON, SHGFI_LARGEICON or
/// SHGFI_OPENICON; the others are left out. `None` when it fails.
pub fn file_info_by_name(name: &[u16], attrs: u32, flags: SHGFI_FLAGS) -> Option<SHFILEINFOW> {
    let allowed = SHGFI_TYPENAME
        | SHGFI_DISPLAYNAME
        | SHGFI_SYSICONINDEX
        | SHGFI_SMALLICON
        | SHGFI_LARGEICON
        | SHGFI_OPENICON;
    let flags = (flags & allowed) | SHGFI_USEFILEATTRIBUTES;
    let name = nul_ended(name);
    let mut info = SHFILEINFOW::default();
    // SAFETY: a NUL-terminated name (no flag left makes it a PIDL), and an
    // out-struct with its size; no icon is made for the caller to free.
    let ok = unsafe {
        SHGetFileInfoW(
            name.as_ptr(),
            attrs,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    } != 0;
    ok.then_some(info)
}

/// The path of the known folder `id` (FOLDERID_*); `None` if the shell
/// does not know it.
pub fn known_folder_path(id: &GUID) -> Option<OsString> {
    let mut p: *mut u16 = core::ptr::null_mut();
    // SAFETY: the shell allocates a NUL-terminated path into `p`, read up to
    // its NUL and then freed with the allocator the shell used (null is
    // freed harmlessly).
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, core::ptr::null_mut(), &mut p);
        let path = (hr >= 0 && !p.is_null()).then(|| {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            OsString::from_wide(core::slice::from_raw_parts(p, len))
        });
        CoTaskMemFree(p.cast());
        path
    }
}

/// How CommandLineToArgvW splits `line` (as a new process's arguments).
pub fn command_line_to_argv(line: &str) -> Vec<String> {
    let w = crate::win::wz(line);
    let mut n = 0;
    // SAFETY: a NUL-terminated line; the array is read for its `n`
    // NUL-terminated strings, then freed with LocalFree as documented.
    unsafe {
        let list = CommandLineToArgvW(w.as_ptr(), &mut n);
        if list.is_null() {
            return Vec::new();
        }
        let out = (0..usize::try_from(n).unwrap_or(0))
            .map(|i| {
                let p = *list.add(i);
                let len = (0..).take_while(|&j| *p.add(j) != 0).count();
                String::from_utf16_lossy(core::slice::from_raw_parts(p, len))
            })
            .collect();
        LocalFree(list.cast());
        out
    }
}

/// Puts the file at `path` in Windows' recent items (SHAddToRecentDocs);
/// Windows leaves it out when the user turned that off.
pub fn add_to_recent_docs(path: &[u16]) {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path, as SHARD_PATHW says.
    unsafe { SHAddToRecentDocs(SHARD_PATHW as u32, path.as_ptr().cast()) };
}

/// Shows the shell's Properties for the file at `path`, owned by `hwnd`
/// (SHObjectProperties); false if it could not.
pub fn object_properties(hwnd: HWND, path: &[u16]) -> bool {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path; no page named.
    unsafe { SHObjectProperties(hwnd, SHOP_FILEPATH as u32, path.as_ptr(), core::ptr::null()) != 0 }
}

/// An item ID list the shell made (an absolute PIDL), freed when this
/// drops.
pub struct Pidl(NonNull<ITEMIDLIST>);

impl Pidl {
    /// The item at `path` (ILCreateFromPathW, which may read the disk);
    /// `None` if there is none.
    pub fn from_path(path: &[u16]) -> Option<Pidl> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated path; the list made is this one's.
        NonNull::new(unsafe { ILCreateFromPathW(path.as_ptr()) }).map(Pidl)
    }

    fn ptr(&self) -> *const ITEMIDLIST {
        self.0.as_ptr()
    }

    /// A copy (ILClone); `None` if there was no memory for it.
    pub fn try_clone(&self) -> Option<Pidl> {
        // SAFETY: a valid list; the copy is the new one's.
        NonNull::new(unsafe { ILClone(self.ptr()) }).map(Pidl)
    }

    /// Takes its last ID off, making it its parent's; false at the
    /// desktop's (empty) list, which has none.
    pub fn remove_last_id(&mut self) -> bool {
        // SAFETY: a valid list of this one's, shortened in place.
        unsafe { ILRemoveLastID(self.0.as_ptr()) != 0 }
    }

    /// Whether it holds `child` (`immediate`: directly).
    pub fn is_parent_of(&self, child: &Pidl, immediate: bool) -> bool {
        // SAFETY: two valid lists, read during the call.
        unsafe { ILIsParent(self.ptr(), child.ptr(), immediate.into()) != 0 }
    }

    /// Whether it is the same item as `other`.
    pub fn is_equal(&self, other: &Pidl) -> bool {
        // SAFETY: two valid lists, read during the call.
        unsafe { ILIsEqual(self.ptr(), other.ptr()) != 0 }
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: the list this owns, freed once.
        unsafe { ILFree(self.ptr()) };
    }
}

/// Opens `folder` in Explorer with `items` (items in it) selected, by
/// their last IDs; with none, opens `folder`'s own folder with `folder`
/// selected (SHOpenFolderAndSelectItems).
pub fn open_folder_and_select(folder: &Pidl, items: &[&Pidl]) -> bool {
    // SAFETY: each item's last ID points into that item's list, which
    // outlives the call.
    let children: Vec<*const ITEMIDLIST> = items
        .iter()
        .map(|p| unsafe { ILFindLastID(p.ptr()) }.cast_const())
        .collect();
    let array = if children.is_empty() {
        core::ptr::null()
    } else {
        children.as_ptr()
    };
    // SAFETY: a valid list and `children.len()` relative IDs, all alive for
    // the call.
    unsafe { SHOpenFolderAndSelectItems(folder.ptr(), count_u32(children.len()), array, 0) >= 0 }
}

/// IDataObject's interface id.
const IID_IDATA_OBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);

/// A data object the shell made of items (IDataObject), released when this
/// drops. It belongs to this thread's apartment (it is not `Send`).
pub struct DataObject(NonNull<core::ffi::c_void>);

impl DataObject {
    /// A data object of `items`, each held by `folder`, as children of it
    /// (SHCreateDataObject, which copies what it keeps); `None` if one of
    /// them is not under `folder`, or the shell made none.
    pub fn of_children(folder: &Pidl, items: &[&Pidl]) -> Option<DataObject> {
        let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(items.len());
        for item in items {
            // SAFETY: two valid lists; the child points into `item`'s,
            // which outlives its use below.
            let child = unsafe { ILFindChild(folder.ptr(), item.ptr()) };
            if child.is_null() {
                return None;
            }
            children.push(child.cast_const());
        }
        let mut data = core::ptr::null_mut();
        // SAFETY: a valid folder and `children.len()` children of it, alive
        // for the call; no inner object; the interface id and an out-pointer
        // for an IDataObject, whose reference is this one's.
        let hr = unsafe {
            SHCreateDataObject(
                folder.ptr(),
                count_u32(children.len()),
                children.as_ptr(),
                core::ptr::null_mut(),
                &IID_IDATA_OBJECT,
                &mut data,
            )
        };
        if hr < 0 {
            return None;
        }
        NonNull::new(data).map(DataObject)
    }

    /// Shows the shell's Properties for its items (SHMultiFileProperties):
    /// one sheet for them all, which takes its own reference.
    pub fn show_properties(&self) -> bool {
        // SAFETY: a live IDataObject this holds a reference to.
        unsafe { SHMultiFileProperties(self.0.as_ptr(), 0) >= 0 }
    }
}

impl Drop for DataObject {
    fn drop(&mut self) {
        // SAFETY: the reference this holds, given up once (IUnknown's
        // Release, by the interface's drop).
        drop(unsafe {
            <windows_core::IUnknown as windows_core::Interface>::from_raw(self.0.as_ptr())
        });
    }
}

/// What [`shell_execute`] runs, and how.
pub struct Execute<'a> {
    /// The window that owns what the shell shows (null: none).
    pub owner: HWND,
    /// The verb (`None`: the default one).
    pub verb: Option<&'a str>,
    pub file: &'a [u16],
    pub params: Option<&'a str>,
    pub dir: Option<&'a [u16]>,
    /// SW_SHOWNORMAL, SW_HIDE…
    pub show: SHOW_WINDOW_CMD,
    /// Keep the process started (SEE_MASK_NOCLOSEPROCESS), to wait for it.
    pub process: bool,
}

/// ShellExecuteEx, finishing what it does before it returns
/// (SEE_MASK_NOASYNC: the thread may end right after). The process started,
/// when `process` asked for it and there is one; the system's error code
/// when it failed.
pub fn shell_execute(e: &Execute) -> Result<Option<Process>, u32> {
    let verb = e.verb.map(crate::win::wz);
    let file = nul_ended(e.file);
    let params = e.params.map(crate::win::wz);
    let dir = e.dir.map(nul_ended);
    let ptr = |s: Option<&[u16]>| s.map_or(core::ptr::null(), <[u16]>::as_ptr);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC
            | if e.process {
                SEE_MASK_NOCLOSEPROCESS
            } else {
                0
            },
        hwnd: e.owner,
        lpVerb: ptr(verb.as_deref()),
        lpFile: file.as_ptr(),
        lpParameters: ptr(params.as_deref()),
        lpDirectory: ptr(dir.as_deref()),
        nShow: e.show,
        ..Default::default()
    };
    // SAFETY: every string is NUL-terminated (or null) and outlives the
    // call; the mask asks for nothing that reads another field.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(sys::last_error());
    }
    if info.hProcess.is_null() {
        return Ok(None);
    }
    // SAFETY: the process handle SEE_MASK_NOCLOSEPROCESS gave the caller,
    // owned by nothing else.
    Ok(Some(Process(unsafe {
        OwnedHandle::from_raw_handle(info.hProcess)
    })))
}

/// The system folder, such as `C:\Windows\system32`; `None` if it does not
/// fit 260 units.
pub fn system_dir() -> Option<String> {
    let mut buf = [0u16; 260];
    // SAFETY: the buffer and its length.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n > buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// The shell item at `path`; `None` when there is none.
pub fn shell_item(path: &[u16]) -> Option<IShellItem> {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path that outlives the call.
    unsafe { SHCreateItemFromParsingName(WPCWSTR(path.as_ptr()), None).ok() }
}

/// The shell's file operation (IFileOperation): items queued, then done
/// together with its own questions, progress and errors.
pub struct FileOperation(IFileOperation);

impl FileOperation {
    /// A new operation, owned by `owner` (null: none), with FOF_/FOFX_
    /// `flags`, telling `sink` what it does. Fails without COM on the
    /// thread.
    pub fn new(
        owner: HWND,
        flags: u32,
        sink: &IFileOperationProgressSink,
    ) -> windows::core::Result<FileOperation> {
        // SAFETY: plain COM calls on the operation made here; without COM
        // on the thread they fail.
        unsafe {
            let op: IFileOperation = CoCreateInstance(&FileOperationClass, None, CLSCTX_ALL)?;
            if !owner.is_null() {
                op.SetOwnerWindow(WHWND(owner))?;
            }
            op.SetOperationFlags(FILEOPERATION_FLAGS(flags))?;
            // The operation is released with its sink when it drops.
            op.Advise(sink)?;
            Ok(FileOperation(op))
        }
    }

    pub fn delete_item(&self, item: &IShellItem) -> windows::core::Result<()> {
        // SAFETY: an item, held for the call; no sink of its own.
        unsafe { self.0.DeleteItem(item, None::<&IFileOperationProgressSink>) }
    }

    /// Queues a move of `item` into `folder`, under its own name.
    pub fn move_item(&self, item: &IShellItem, folder: &IShellItem) -> windows::core::Result<()> {
        // SAFETY: two items, held for the call; no new name, no sink.
        unsafe {
            self.0.MoveItem(
                item,
                folder,
                WPCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
    }

    /// Queues a copy of `item` into `folder`, under its own name.
    pub fn copy_item(&self, item: &IShellItem, folder: &IShellItem) -> windows::core::Result<()> {
        // SAFETY: two items, held for the call; no new name, no sink.
        unsafe {
            self.0.CopyItem(
                item,
                folder,
                WPCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
    }

    /// Queues a rename of `item` to `name` (a name, not a path).
    pub fn rename_item(&self, item: &IShellItem, name: &[u16]) -> windows::core::Result<()> {
        let name = nul_ended(name);
        // SAFETY: an item and a NUL-terminated name (the operation copies
        // it), held for the call; no sink.
        unsafe {
            self.0.RenameItem(
                item,
                WPCWSTR(name.as_ptr()),
                None::<&IFileOperationProgressSink>,
            )
        }
    }

    /// Does what was queued; its windows run their own message loops.
    pub fn perform(&self) -> windows::core::Result<()> {
        // SAFETY: a COM call on the operation this holds.
        unsafe { self.0.PerformOperations() }
    }

    /// Whether any of it was stopped or skipped (by the user).
    pub fn any_aborted(&self) -> windows::core::Result<bool> {
        // SAFETY: a COM call on the operation this holds.
        unsafe { self.0.GetAnyOperationsAborted() }.map(|b| b.as_bool())
    }
}

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
        let name = crate::win::wz(name);
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
        let ok = unsafe { QueryServiceConfigW(self.0, cfg, count_u32(buf.len() * 8), &mut needed) };
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

// ---- Windows, the tray, monitors ----

/// The message registered as `name` (RegisterWindowMessageW), the same in
/// every process; 0 if it could not be.
pub fn register_message(name: &str) -> u32 {
    let name = crate::win::wz(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegisterWindowMessageW(name.as_ptr()) }
}

/// One of the system's icons (IDI_APPLICATION…), shared, never destroyed;
/// null for anything else.
pub fn system_icon(id: PCWSTR) -> HICON {
    // A system icon's id travels as the pointer's address
    // (MAKEINTRESOURCE), below 0x10000; anything else would be read as a
    // name.
    if id as usize >= 0x10000 {
        return core::ptr::null_mut();
    }
    // SAFETY: an id, not a string (above), of the system's icons.
    unsafe { LoadIconW(core::ptr::null_mut(), id) }
}

/// Shell_NotifyIconW: adds, changes or deletes a notification-area icon as
/// `op` says (NIM_*), described by `d` (its size is set here).
///
/// # Panics
/// If `d` asks for its clicks (NIF_MESSAGE) as a message that is not one of
/// goz-gui's own (`WM_APP` up to the registered ones): the shell posts it
/// with integers, which a system message could take for pointers.
pub fn notify_icon(op: NOTIFY_ICON_MESSAGE, d: &NOTIFYICONDATAW) -> bool {
    let mut d = *d;
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    if d.uFlags & NIF_MESSAGE == 0 {
        d.uCallbackMessage = 0;
    } else {
        assert!(
            (WM_APP..APP_END).contains(&d.uCallbackMessage),
            "not one of goz-gui's private messages"
        );
    }
    // SAFETY: a complete structure (no pointers in it) with its size.
    unsafe { Shell_NotifyIconW(op, &d) != 0 }
}

/// Lets process `pid` bring its window forward (AllowSetForegroundWindow).
pub fn allow_set_foreground(pid: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { AllowSetForegroundWindow(pid) != 0 }
}

/// Sends `to` a WM_COPYDATA from `from` with `data` (its dwData) and a copy
/// of `bytes`, giving up after `timeout_ms` or at once if `to` hangs
/// (SMTO_ABORTIFHUNG): its answer, `None` if it gave none.
pub fn send_copy_data(
    to: HWND,
    from: HWND,
    data: usize,
    bytes: &[u8],
    timeout_ms: u32,
) -> Option<usize> {
    let cds = COPYDATASTRUCT {
        dwData: data,
        cbData: count_u32(bytes.len()),
        lpData: bytes.as_ptr() as *mut core::ffi::c_void,
    };
    let mut answer = 0usize;
    // SAFETY: WM_COPYDATA's lParam is a COPYDATASTRUCT, here of `bytes`
    // (no more than its length); both outlive the call, and the system
    // copies them for a receiver on another thread before it returns.
    let ok = unsafe {
        SendMessageTimeoutW(
            to,
            WM_COPYDATA,
            from as usize,
            (&raw const cds) as LPARAM,
            SMTO_ABORTIFHUNG,
            timeout_ms,
            &mut answer,
        )
    } != 0;
    ok.then_some(answer)
}

/// An accelerator table (CreateAcceleratorTableW), destroyed when this
/// drops.
pub struct AccelTable(HACCEL);

impl AccelTable {
    /// A table of `entries` (Windows copies them); `None` for an empty one,
    /// which cannot be made, or when Windows would not make it.
    pub fn new(entries: &[ACCEL]) -> Option<AccelTable> {
        // SAFETY: the entries and their count.
        let h = unsafe { CreateAcceleratorTableW(entries.as_ptr(), count_i32(entries.len())) };
        (!h.is_null()).then_some(AccelTable(h))
    }

    pub fn handle(&self) -> HACCEL {
        self.0
    }

    /// The table, no longer destroyed by this: whoever takes it destroys
    /// it.
    pub fn into_raw(self) -> HACCEL {
        let h = self.0;
        core::mem::forget(self);
        h
    }
}

impl Drop for AccelTable {
    fn drop(&mut self) {
        // SAFETY: the table this owns, destroyed once.
        unsafe { DestroyAcceleratorTable(self.0) };
    }
}

/// The display monitors as Windows has them now (EnumDisplayMonitors).
pub fn display_monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn each(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec of `display_monitors`, alive across the
        // enumeration and not otherwise borrowed meanwhile.
        let out = unsafe { &mut *(data as *mut Vec<HMONITOR>) };
        out.push(mon);
        1
    }
    let mut out: Vec<HMONITOR> = Vec::new();
    // SAFETY: every monitor (no DC, no clip); the callback above, which only
    // pushes to `out`.
    unsafe {
        EnumDisplayMonitors(
            core::ptr::null_mut(),
            core::ptr::null(),
            Some(each),
            (&raw mut out) as LPARAM,
        )
    };
    out
}

/// A monitor's effective DPI (horizontal); `None` if the system would not
/// say.
pub fn monitor_dpi(mon: HMONITOR) -> Option<u32> {
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: two out-values.
    let hr = unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    (hr >= 0).then_some(dx)
}

/// A bitmap of `cx` by `cy` pixels, `planes` and `bits` per pixel, its
/// pixels not set (CreateBitmap with no bits); null if it could not be
/// made. The caller deletes it.
pub fn create_bitmap(cx: i32, cy: i32, planes: u32, bits: u32) -> HBITMAP {
    // SAFETY: no initial bits.
    unsafe { CreateBitmap(cx, cy, planes, bits, core::ptr::null()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_are_their_bytes() {
        assert_eq!(units_as_bytes(&[0x0102, 0x0304]), [2, 1, 4, 3]);
        assert!(units_as_bytes(&[]).is_empty());
    }

    #[test]
    fn a_named_mutex_is_owned_once_per_holder() {
        let name = format!("Local\\goz-gui-test-b8-{}", std::process::id());
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
    fn command_lines_split_as_windows_splits_them() {
        assert_eq!(
            command_line_to_argv(r#"a.exe "two words" x"#),
            ["a.exe", "two words", "x"]
        );
    }

    #[test]
    fn only_global_memory_formats_are_read_or_put() {
        assert!(is_global_format(u32::from(CF_UNICODETEXT)));
        assert!(is_global_format(0xc0ff));
        // CF_BITMAP's data is a bitmap, not a memory block.
        assert!(!is_global_format(2));
    }

    #[test]
    fn a_pipe_is_written_and_read_overlapped() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        };
        let name = format!(r"\\.\pipe\goz-gui-test-b8-{}", std::process::id());
        let wide = crate::win::wz(&name);
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
}
