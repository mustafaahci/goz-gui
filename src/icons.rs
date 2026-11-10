//! Shell icons for result rows.
//!
//! Most files take their icon from their extension, which the shell answers
//! from the registry without touching the file. Programs, shortcuts and
//! icon files carry their own icon, which means reading the file, so those
//! are looked up on a background thread and the row is redrawn when the
//! answer arrives.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::HIMAGELIST;
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::client::Row;
use crate::wide::{nul, wz};

/// Not looked at yet.
pub const UNSET: i32 = -2;
/// Asked of the background thread.
pub const PENDING: i32 = -1;
/// The file has no icon of its own: use its extension's.
pub const GENERIC: i32 = -3;

/// Extensions whose files carry their own icon.
const OWN_ICON: &[&str] = &[
    "exe",
    "ico",
    "lnk",
    "url",
    "cur",
    "ani",
    "scr",
    "cpl",
    "msc",
    "appref-ms",
];

/// Requests waiting at most; older ones have scrolled away.
const MAX_QUEUED: usize = 256;

pub struct IconReply {
    pub epoch: u64,
    pub index: usize,
    pub icon: i32,
}

struct Queue {
    epoch: u64,
    items: Vec<(usize, Vec<u16>)>,
}

struct Shared {
    queue: Mutex<Queue>,
    work: usize,
    hwnd: usize,
    msg: u32,
}

pub struct Icons {
    pub list: HIMAGELIST,
    folder: i32,
    by_ext: RefCell<HashMap<Vec<u16>, i32>>,
    shared: Arc<Shared>,
}

impl Icons {
    /// Starts the icon thread; its answers arrive at `hwnd` as `msg`, with a
    /// `Box<IconReply>` in `lParam`.
    pub fn new(hwnd: HWND, msg: u32) -> Icons {
        let mut info = info();
        // SAFETY: an attribute-only lookup; `info` is a plain out-struct.
        let list = unsafe {
            SHGetFileInfoW(
                wz("folder").as_ptr(),
                FILE_ATTRIBUTE_DIRECTORY,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            )
        } as HIMAGELIST;
        let folder = info.iIcon;
        // SAFETY: an unnamed manual-reset event.
        let work = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) } as usize;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                epoch: 0,
                items: Vec::new(),
            }),
            work,
            hwnd: hwnd as usize,
            msg,
        });
        let thread = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("goz-icons".into())
            .spawn(move || icon_thread(&thread))
            .expect("spawn icon thread");
        Icons {
            list,
            folder,
            by_ext: RefCell::new(HashMap::new()),
            shared,
        }
    }

    /// Drops queued lookups: rows of search `epoch` are on screen now.
    pub fn reset(&self, epoch: u64) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.epoch = epoch;
        q.items.clear();
    }

    /// The icon to draw for `row` (row `index` of search `epoch`) right now,
    /// and the row of that search whose lookup was dropped to make room for
    /// this one, if any: its icon must go back to [`UNSET`] so that it is
    /// asked for again when it is drawn again.
    pub fn for_row(&self, row: &Row, epoch: u64, index: usize) -> (i32, Option<usize>) {
        let own = row.icon.get();
        if own >= 0 {
            return (own, None);
        }
        if row.is_dir {
            return (self.folder, None);
        }
        let ext = extension(row.name());
        let generic = self.by_ext(&ext);
        let mut dropped = None;
        if own == UNSET && wants_own(&ext) {
            let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            // Rows of another search are not looked up (`reset` has not
            // caught up); they stay UNSET and ask again on their next draw.
            if q.epoch == epoch {
                if q.items.len() >= MAX_QUEUED {
                    dropped = Some(q.items.remove(0).0);
                }
                q.items.push((index, nul(&row.path)));
                row.icon.set(PENDING);
                // SAFETY: a live event handle.
                unsafe { SetEvent(self.shared.work as HANDLE) };
            }
        }
        (generic, dropped)
    }

    /// The icon of `row`'s type, never the file's own: what a thumbnail
    /// cell shows while its picture is made (or when it has none). A type's
    /// icon is one entry of the system image lists however many files show
    /// it; a program's own icon is an entry each, which drawn from the
    /// jumbo list stays there, about 50 KB a program (measured).
    pub fn type_icon(&self, row: &Row) -> i32 {
        if row.is_dir {
            self.folder
        } else {
            self.by_ext(&extension(row.name()))
        }
    }

    fn by_ext(&self, ext: &[u16]) -> i32 {
        if let Some(&i) = self.by_ext.borrow().get(ext) {
            return i;
        }
        let mut probe: Vec<u16> = "file.".encode_utf16().collect();
        probe.extend_from_slice(ext);
        probe.push(0);
        let mut info = info();
        // SAFETY: an attribute-only lookup by name; nothing is read from disk.
        unsafe {
            SHGetFileInfoW(
                probe.as_ptr(),
                FILE_ATTRIBUTE_NORMAL,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            )
        };
        self.by_ext.borrow_mut().insert(ext.to_vec(), info.iIcon);
        info.iIcon
    }
}

/// Whether files of extension `ext` (lower case, no dot) carry their own
/// icon.
pub fn wants_own(ext: &[u16]) -> bool {
    OWN_ICON
        .iter()
        .any(|e| e.len() == ext.len() && e.bytes().zip(ext).all(|(a, &b)| u16::from(a) == b))
}

/// The extension of file name `name`, lower case, without its dot.
fn extension(name: &[u16]) -> Vec<u16> {
    match name.iter().rposition(|&c| c == u16::from(b'.')) {
        Some(dot) => name[dot + 1..]
            .iter()
            .map(|&c| crate::highlight::fold(c))
            .collect(),
        None => Vec::new(),
    }
}

fn info() -> SHFILEINFOW {
    // SAFETY: SHFILEINFOW is plain data; all-zero is a valid value.
    unsafe { core::mem::zeroed() }
}

fn icon_thread(shared: &Shared) {
    // SAFETY: the shell wants COM on a thread that asks it for icons.
    unsafe {
        CoInitializeEx(
            core::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    loop {
        let next = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            match q.items.pop() {
                Some((index, path)) => Some((q.epoch, index, path)),
                None => {
                    // SAFETY: a live event handle.
                    unsafe { ResetEvent(shared.work as HANDLE) };
                    None
                }
            }
        };
        let Some((epoch, index, path)) = next else {
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(shared.work as HANDLE, INFINITE) };
            continue;
        };
        let mut info = info();
        // SAFETY: `path` is NUL-terminated; `info` is a plain out-struct.
        let ok = unsafe {
            SHGetFileInfoW(
                path.as_ptr(),
                0,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON,
            )
        } != 0;
        let reply = Box::into_raw(Box::new(IconReply {
            epoch,
            index,
            icon: if ok { info.iIcon } else { GENERIC },
        }));
        // SAFETY: the window owns the box once posted; reclaimed if not.
        unsafe {
            if PostMessageW(shared.hwnd as HWND, shared.msg, 0, reply as isize) == 0 {
                drop(Box::from_raw(reply));
            }
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the event is owned by `Shared` and closed once.
        unsafe { CloseHandle(self.work as HANDLE) };
    }
}
