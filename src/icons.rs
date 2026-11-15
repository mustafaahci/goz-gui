//! Shell icons for result rows.
//!
//! Most files take their icon from their extension, which the shell answers
//! from the registry without touching the file; each type's is looked up
//! once, on a thread (`typeinfo`), and a file of a type not known yet shows
//! the document icon of no known type meanwhile. Programs, shortcuts and
//! icon files carry their own icon, which means reading the file, so those
//! are looked up on a background thread of their own and the row is
//! redrawn when the answer arrives.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::UI::Controls::HIMAGELIST;
use windows_sys::Win32::UI::Shell::{SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES, SIID_DOCNOASSOC};

use crate::client::Row;
use crate::filetype;
use crate::typeinfo::TypeInfo;
use crate::win;
use crate::win::com::Apartment;
use crate::win::icon;
use crate::win::parcel;
use crate::win::sync::Event;

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
    /// Set while lookups wait (`None` if the system could not make one).
    work: Option<Event>,
    hwnd: usize,
    msg: u32,
}

pub struct Icons {
    pub list: HIMAGELIST,
    folder: i32,
    /// A file's icon while its type's is looked up: the document of no
    /// known type (the shell's stock icon), asked for as the window starts.
    generic: i32,
    /// Each type's icon and name (`typeinfo`).
    pub types: TypeInfo,
    shared: Arc<Shared>,
}

impl Icons {
    /// Starts the icon thread; its answers arrive at `hwnd` as `msg`, each
    /// an [`IconReply`] parcel (`win::parcel::take`). The types' thread
    /// tells `hwnd` of its answers as `types_msg`, and `ask_msg` is posted
    /// to send it what a paint asked for (`TypeInfo::new`).
    pub fn new(hwnd: HWND, msg: u32, types_msg: u32, ask_msg: u32) -> Icons {
        // An attribute-only lookup.
        let (list, folder) = icon::sys_icon(
            &win::wz("folder"),
            FILE_ATTRIBUTE_DIRECTORY,
            SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        // The stock icon's index only (0.3 ms; the same lookup by the name
        // "file" took 2.5 to 3.4 ms).
        let generic = icon::stock_index(SIID_DOCNOASSOC).map_or(0, |i| i.max(0));
        // A manual-reset event.
        let work = Event::new(true, false);
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
            generic,
            types: TypeInfo::new(hwnd, types_msg, ask_msg, list, vec![generic, folder]),
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
        let generic = self.of_type(&ext);
        let mut dropped = None;
        if own == UNSET && wants_own(&ext) {
            let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            // Rows of another search are not looked up (`reset` has not
            // caught up); they stay UNSET and ask again on their next draw.
            if q.epoch == epoch {
                if q.items.len() >= MAX_QUEUED {
                    dropped = Some(q.items.remove(0).0);
                }
                q.items.push((index, win::wz_units(&row.path)));
                row.icon.set(PENDING);
                if let Some(work) = &self.shared.work {
                    work.set();
                }
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
            self.of_type(&extension(row.name()))
        }
    }

    /// The icon of files of extension `ext` (case-folded): the type's once
    /// the types' thread has said, the generic one meanwhile (or for good,
    /// if the shell has none).
    fn of_type(&self, ext: &[u16]) -> i32 {
        self.types.icon(ext).unwrap_or(self.generic)
    }

    /// The Type column's text for `row`, if its type is known; else it is
    /// looked up on the types' thread, and the cell is empty meanwhile.
    pub fn type_name(&self, row: &Row) -> Option<Rc<[u16]>> {
        self.types.name(&filetype::key(row))
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

fn icon_thread(shared: &Shared) {
    // The shell wants COM on a thread that asks it for icons, for the
    // thread's life.
    let _com = Apartment::sta();
    loop {
        let next = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            match q.items.pop() {
                Some((index, path)) => Some((q.epoch, index, path)),
                None => {
                    if let Some(work) = &shared.work {
                        work.reset();
                    }
                    None
                }
            }
        };
        let Some((epoch, index, path)) = next else {
            if let Some(work) = &shared.work {
                work.wait(INFINITE);
            }
            continue;
        };
        let (list, icon) = icon::sys_icon(&path, 0, SHGFI_SMALLICON);
        let ok = list != 0;
        let reply = IconReply {
            epoch,
            index,
            icon: if ok { icon } else { GENERIC },
        };
        // The window takes the reply once posted; it is dropped if not.
        parcel::post(shared.hwnd as HWND, shared.msg, 0, reply);
    }
}
