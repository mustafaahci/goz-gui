//! The trampolines at work, on real (message-only) windows and dialogs that
//! never show.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DS_SETFONT, HWND_MESSAGE, PM_REMOVE, WM_APP, WM_INITDIALOG, WM_NCDESTROY, WS_CHILD, WS_POPUP,
};

use super::class::{self, Class, Create};
use super::dlg::{self, Template};
use super::hook::{self, Event, Kind};
use super::{Msg, parcel, queue, subclass};

const CLASS: &str = "goz-gui test window";

/// Sets a flag when it drops: whether a handler (and what it holds) was
/// freed.
struct Dropped(Rc<Cell<bool>>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

/// A message-only window of the test class, its messages going to
/// `handler`.
fn window(handler: impl Fn(&Msg) -> isize + 'static) -> HWND {
    let _ = Class::new(CLASS).register();
    let hwnd = class::create(
        &Create {
            ex_style: 0,
            class: CLASS,
            title: "",
            style: 0,
            rect: (0, 0, 0, 0),
            parent: HWND_MESSAGE,
            menu: core::ptr::null_mut(),
        },
        handler,
    );
    assert!(!hwnd.is_null());
    hwnd
}

/// Dispatches what waits in `hwnd`'s queue.
fn pump(hwnd: HWND) {
    while let Some(q) = queue::peek(hwnd, (0, 0), PM_REMOVE) {
        queue::dispatch(&q);
    }
}

#[test]
fn a_window_handles_its_messages_and_frees_its_handler_when_destroyed() {
    let dropped = Rc::new(Cell::new(false));
    let held = Dropped(Rc::clone(&dropped));
    let hwnd = window(move |m| {
        let _ = &held;
        if m.id == WM_APP + 1 { 7 } else { m.def() }
    });
    assert_eq!(super::send_app(hwnd, WM_APP + 1, 0, 0), 7);
    assert!(!dropped.get());
    assert!(super::destroy(hwnd));
    assert!(dropped.get());
}

static FIRST: subclass::Key = subclass::Key::new("test first");
static SECOND: subclass::Key = subclass::Key::new("test second");

#[test]
fn subclasses_come_first_and_pass_messages_on() {
    let hwnd = window(|m| if m.id == WM_APP + 1 { 7 } else { m.def() });
    assert!(subclass::set(hwnd, &FIRST, |m| {
        if m.id == WM_APP + 1 {
            m.def() + 1
        } else {
            m.def()
        }
    }));
    assert_eq!(super::send_app(hwnd, WM_APP + 1, 0, 0), 8);
    // Two subclasses by two keys: both, the later first.
    assert!(subclass::set(hwnd, &SECOND, |m| {
        if m.id == WM_APP + 1 {
            m.def() * 10
        } else {
            m.def()
        }
    }));
    assert_eq!(super::send_app(hwnd, WM_APP + 1, 0, 0), 80);
    // Set again by the same key: replaced.
    assert!(subclass::set(hwnd, &SECOND, |m| {
        if m.id == WM_APP + 1 {
            m.def() * 100
        } else {
            m.def()
        }
    }));
    assert_eq!(super::send_app(hwnd, WM_APP + 1, 0, 0), 800);
    assert!(subclass::remove(hwnd, &SECOND));
    assert!(!subclass::has(hwnd, &SECOND));
    assert_eq!(super::send_app(hwnd, WM_APP + 1, 0, 0), 8);
    super::destroy(hwnd);
}

#[test]
fn a_subclass_may_remove_itself_and_its_window_may_go_while_it_runs() {
    let hwnd = window(|m| m.def());
    let dropped = Rc::new(Cell::new(false));
    let held = Dropped(Rc::clone(&dropped));
    let ran = Rc::new(Cell::new(0));
    let r = Rc::clone(&ran);
    assert!(subclass::set(hwnd, &FIRST, move |m| {
        if m.id == WM_APP + 2 {
            subclass::remove(m.hwnd, &FIRST);
            super::destroy(m.hwnd);
            // Still alive here: the running call holds the closure.
            r.set(r.get() + 1);
            let _ = &held;
            return 5;
        }
        m.def()
    }));
    assert_eq!(super::send_app(hwnd, WM_APP + 2, 0, 0), 5);
    assert_eq!(ran.get(), 1);
    assert!(dropped.get());
    assert!(!super::is_window(hwnd));
}

#[test]
fn a_subclass_goes_with_its_window() {
    let hwnd = window(|m| m.def());
    let dropped = Rc::new(Cell::new(false));
    let held = Dropped(Rc::clone(&dropped));
    let saw_end = Rc::new(Cell::new(false));
    let end = Rc::clone(&saw_end);
    subclass::set(hwnd, &FIRST, move |m| {
        let _ = &held;
        if m.id == WM_NCDESTROY {
            end.set(true);
        }
        m.def()
    });
    super::destroy(hwnd);
    assert!(saw_end.get());
    assert!(dropped.get());
}

#[test]
fn a_parcel_reaches_its_window_and_a_forged_ticket_finds_nothing() {
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = Rc::clone(&got);
    let hwnd = window(move |m| {
        if m.id == WM_APP + 3 {
            g.borrow_mut().push(parcel::take::<String>(m));
            return 0;
        }
        m.def()
    });
    assert!(parcel::post(hwnd, WM_APP + 3, 0, String::from("hi")));
    assert!(super::post_app(hwnd, WM_APP + 3, 0, 0x7fff_0000));
    pump(hwnd);
    assert_eq!(*got.borrow(), vec![Some(String::from("hi")), None]);
    super::destroy(hwnd);
}

fn template(style: u32) -> Template {
    let face: Vec<u16> = "Segoe UI".encode_utf16().collect();
    Template::with_styles("", style | DS_SETFONT as u32, 0, (10, 10), 9, &face)
}

#[test]
fn a_modal_dialog_runs_its_handler_until_it_ends() {
    let state = Cell::new(0);
    let ended = template(WS_POPUP).modal(core::ptr::null_mut(), &|m| {
        if m.id == WM_INITDIALOG {
            state.set(state.get() + 1);
            dlg::end(m.hwnd, 42);
            return 1;
        }
        0
    });
    assert_eq!(ended, 42);
    assert_eq!(state.get(), 1);
}

#[test]
fn a_child_dialog_frees_its_handler_when_destroyed() {
    let parent = window(|m| m.def());
    let dropped = Rc::new(Cell::new(false));
    let held = Dropped(Rc::clone(&dropped));
    let inits = Rc::new(Cell::new(0));
    let i = Rc::clone(&inits);
    let page = template(WS_CHILD).create_child(parent, move |m| {
        let _ = &held;
        if m.id == WM_INITDIALOG {
            i.set(i.get() + 1);
        }
        0
    });
    assert!(!page.is_null());
    assert_eq!(inits.get(), 1);
    assert!(!dropped.get());
    super::destroy(page);
    assert!(dropped.get());
    super::destroy(parent);
}

#[test]
fn a_hook_sees_the_windows_made_and_goes_when_dropped() {
    let made = Rc::new(RefCell::new(Vec::new()));
    let m = Rc::clone(&made);
    let hook = hook::set(Kind::Cbt, move |e| {
        if let Event::Create { hwnd, .. } = *e {
            m.borrow_mut().push(hwnd);
        }
        None
    })
    .expect("a thread hook");
    let hwnd = window(|m| m.def());
    assert!(made.borrow().contains(&hwnd));
    drop(hook);
    let other = window(|m| m.def());
    assert!(!made.borrow().contains(&other));
    super::destroy(hwnd);
    super::destroy(other);
}

/// A real drop of `paths` (a DROPFILES in moveable global memory, as the
/// shell makes one), for a test to post.
fn make_drop(paths: &[&str]) -> usize {
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows_sys::Win32::UI::Shell::DROPFILES;
    let mut units: Vec<u16> = Vec::new();
    for p in paths {
        units.extend(p.encode_utf16());
        units.push(0);
    }
    units.push(0);
    let head = size_of::<DROPFILES>();
    let bytes = head + units.len() * 2;
    // SAFETY: a block of `bytes`, locked to write the header and the paths
    // after it, then unlocked.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        assert!(!h.is_null());
        let p = GlobalLock(h).cast::<u8>();
        let df = DROPFILES {
            pFiles: head as u32,
            fWide: 1,
            ..Default::default()
        };
        core::ptr::write_unaligned(p.cast::<DROPFILES>(), df);
        core::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), p.add(head), units.len() * 2);
        GlobalUnlock(h);
        h as usize
    }
}

static DROP_KEY: subclass::Key = subclass::Key::new("test drop");

#[test]
fn a_drop_is_read_while_it_is_delivered_and_finished_once_after() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_DROPFILES};
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s = Rc::clone(&seen);
    let hwnd = window(move |m| {
        if let Some(paths) = m.dropped_files() {
            s.borrow_mut().push(("window", paths));
            return 0;
        }
        m.def()
    });
    // A subclass reads it too, then passes it on: both see it, and the
    // drop is finished once, after the subclass returns.
    let s = Rc::clone(&seen);
    subclass::set(hwnd, &DROP_KEY, move |m| {
        if let Some(paths) = m.dropped_files() {
            s.borrow_mut().push(("subclass", paths));
            let r = m.def();
            // Still in flight: the window's call has returned, this one has
            // not.
            assert!(m.dropped_files().is_some());
            // Not passed on to other windows.
            assert_eq!(m.forward(m.hwnd), 0);
            return r;
        }
        m.def()
    });
    let drop = make_drop(&[r"C:\a.txt", r"C:\b c"]);
    // SAFETY: posts the drop as the shell does, to this thread's window.
    assert!(unsafe { PostMessageW(hwnd, WM_DROPFILES, drop, 0) } != 0);
    pump(hwnd);
    let want: Vec<Vec<u16>> = [r"C:\a.txt", r"C:\b c"]
        .iter()
        .map(|p| p.encode_utf16().collect())
        .collect();
    assert_eq!(
        *seen.borrow(),
        vec![("subclass", want.clone()), ("window", want)]
    );
    // Delivered and finished: nothing reads it now.
    assert!(super::dropfiles::paths(drop).is_none());
    // A drop made up in Rust is never read.
    let made = Msg::new(hwnd, WM_DROPFILES, drop, 0);
    assert!(made.dropped_files().is_none());
    super::destroy(hwnd);
}
