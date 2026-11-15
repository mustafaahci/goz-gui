//! The trampolines at work, on real (message-only) windows and dialogs that
//! never show.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DS_SETFONT, HWND_MESSAGE, WM_APP, WM_INITDIALOG, WM_NCDESTROY, WS_CHILD, WS_POPUP,
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
    while let Some(q) = queue::take(hwnd, (0, 0)) {
        queue::dispatch(q);
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

/// A hidden control of `class` (WS_CHILD and `style`), child of a hidden
/// top-level window of the test class: (the control, its parent).
pub(super) fn hidden_control(class: &str, style: u32) -> (HWND, HWND) {
    use windows_sys::Win32::UI::Controls::{ICC_LISTVIEW_CLASSES, ICC_TREEVIEW_CLASSES};
    super::sys::init_common_controls(ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES);
    let _ = Class::new(CLASS).register();
    let parent = class::create(
        &Create {
            ex_style: 0,
            class: CLASS,
            title: "",
            style: WS_POPUP,
            rect: (0, 0, 200, 200),
            parent: core::ptr::null_mut(),
            menu: core::ptr::null_mut(),
        },
        |m| m.def(),
    );
    assert!(!parent.is_null());
    let control = class::create_plain(&Create {
        ex_style: 0,
        class,
        title: "",
        style: WS_CHILD | style,
        rect: (0, 0, 200, 200),
        parent,
        menu: class::id_menu(1),
    });
    assert!(!control.is_null(), "a {class}");
    (control, parent)
}

#[test]
fn a_dialog_s_answer_goes_only_to_a_dialog() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DWLP_MSGRESULT, GetWindowLongPtrW};
    // An integer of the window's extra bytes, at DWLP_MSGRESULT's offset.
    let result = |hwnd: HWND| {
        // SAFETY: reads an integer, no pointers.
        unsafe { GetWindowLongPtrW(hwnd, DWLP_MSGRESULT as i32) }
    };
    // A list view keeps its object's address at offset 0, DWLP_MSGRESULT's.
    let (list, parent) = hidden_control(super::lv::CLASS, 0);
    let before = result(list);
    dlg::set_result(list, 0x1234);
    assert_eq!(result(list), before);
    assert_eq!(super::lv::item_count(list), 0, "the list still answers");
    // A dialog's goes in.
    let page = template(WS_CHILD).create_child(parent, |_| 0);
    assert!(!page.is_null());
    dlg::set_result(page, 0x1234);
    assert_eq!(result(page), 0x1234);
    super::destroy(parent);
}

#[test]
fn only_a_class_registered_here_gets_a_handler() {
    let made = |class: &str| {
        class::create(
            &Create {
                ex_style: 0,
                class,
                title: "",
                style: 0,
                rect: (0, 0, 0, 0),
                parent: HWND_MESSAGE,
                menu: core::ptr::null_mut(),
            },
            |m| m.def(),
        )
    };
    // A system class, whose procedure would take the handler's address
    // for creation data of its own: not made.
    assert!(made("Static").is_null());
    assert!(made("MDIClient").is_null());
    // The test class, registered here (whatever the case of its name).
    let _ = Class::new(CLASS).register();
    let hwnd = made(&CLASS.to_uppercase());
    assert!(!hwnd.is_null());
    super::destroy(hwnd);
    // No class goes by its atom; no MDI client is made plain.
    assert!(!Class::new("#32770").register());
    let plain = |class: &str| {
        class::create_plain(&Create {
            ex_style: 0,
            class,
            title: "",
            style: 0,
            rect: (0, 0, 0, 0),
            parent: HWND_MESSAGE,
            menu: core::ptr::null_mut(),
        })
    };
    assert!(plain("mdiclient").is_null());
    assert!(plain("#32770").is_null());
}

#[test]
fn a_queued_message_is_taken_and_dispatched_once() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F5;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ACCEL, FVIRTKEY, PM_REMOVE, WM_COMMAND, WM_KEYDOWN,
    };
    // Not Clone: the call would be ambiguous if it were.
    trait Ambiguous<A> {
        fn check() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: Clone> Ambiguous<u8> for T {}
    <queue::Queued as Ambiguous<_>>::check();

    let got = Rc::new(RefCell::new(Vec::new()));
    let g = Rc::clone(&got);
    let hwnd = window(move |m| {
        if m.id == WM_APP + 4 || m.id == WM_COMMAND {
            g.borrow_mut().push((m.id, m.w));
            return 0;
        }
        m.def()
    });
    assert!(super::post_app(hwnd, WM_APP + 4, 0, 0));
    // A peek leaves it there, PM_REMOVE or not, as a plain copy.
    let seen = queue::peek(hwnd, (0, 0), PM_REMOVE).expect("a message");
    assert_eq!(seen.message, WM_APP + 4);
    let q = queue::take(hwnd, (0, 0)).expect("still there");
    assert!(queue::take(hwnd, (0, 0)).is_none(), "taken once");
    // Not one of the table's keys: given back, to dispatch.
    let table = queue::AccelTable::new(&[ACCEL {
        fVirt: FVIRTKEY,
        key: VK_F5,
        cmd: 9,
    }])
    .unwrap();
    let q = queue::translate_accelerator(hwnd, table.handle(), q).expect("not a key");
    queue::dispatch(q);
    assert_eq!(*got.borrow(), vec![(WM_APP + 4, 0)]);
    // One of its keys: used up, its command sent.
    assert!(super::post_plain(hwnd, WM_KEYDOWN, usize::from(VK_F5), 0));
    let q = queue::take(hwnd, (0, 0)).expect("the key");
    assert!(queue::translate_accelerator(hwnd, table.handle(), q).is_none());
    assert_eq!(got.borrow().last(), Some(&(WM_COMMAND, 0x1_0009)));
    super::destroy(hwnd);
}

#[test]
fn a_control_keeps_the_image_lists_it_shows_alive() {
    use super::imagelist::{self, ImageList};
    use super::{lv, tv};
    use windows_sys::Win32::UI::Controls::{
        ILC_COLOR32, LVSIL_NORMAL, LVSIL_SMALL, TVM_GETIMAGELIST, TVSIL_NORMAL,
    };
    let list = || ImageList::create(16, 16, ILC_COLOR32, 1, 1).expect("an image list");
    // A list view takes a list whole, never one held elsewhere too.
    let (view, parent) = hidden_control(lv::CLASS, 0);
    let shared = list();
    assert!(!lv::set_image_list(
        view,
        LVSIL_NORMAL,
        Some(shared.clone())
    ));
    assert!(lv::set_image_list(view, LVSIL_NORMAL, Some(list())));
    // Replaced: the one before is goz-gui's again (and destroyed).
    assert!(lv::set_image_list(view, LVSIL_NORMAL, Some(shared)));
    assert!(lv::set_image_list(view, LVSIL_NORMAL, None));
    // A system list goes in with a reference of its own.
    let small = imagelist::system(1).expect("the small icons");
    assert!(lv::set_system_image_list(view, LVSIL_SMALL, small));
    assert!(lv::set_system_image_list(view, LVSIL_SMALL, small));
    super::destroy(parent);
    assert!(
        imagelist::icon_size(&small).is_some(),
        "the system list lives"
    );
    // A tree shows a list that goz-gui let go: the tree keeps it.
    let (tree, parent) = hidden_control(tv::CLASS, 0);
    let mine = list();
    assert!(tv::set_image_list(tree, TVSIL_NORMAL, Some(&mine)));
    drop(mine);
    // SAFETY: TVM_GETIMAGELIST takes a kind and gives a handle.
    let shown = unsafe { super::send_raw(tree, TVM_GETIMAGELIST, TVSIL_NORMAL as usize, 0) };
    assert_ne!(shown, 0);
    let mut size = (0, 0);
    // SAFETY: the list the tree shows, which `tv::set_image_list` keeps
    // alive while it does; two out-values.
    let ok = unsafe {
        windows_sys::Win32::UI::Controls::ImageList_GetIconSize(shown, &mut size.0, &mut size.1)
    };
    assert!(ok != 0 && size == (16, 16));
    assert!(!tv::set_image_list(view, TVSIL_NORMAL, None), "gone");
    super::destroy(parent);
}

/// A hidden tree view, child of a hidden top-level window of the test
/// class whose messages go to `parent`: (the tree, its parent).
fn hidden_tree(parent: impl Fn(&Msg) -> isize + 'static) -> (HWND, HWND) {
    use windows_sys::Win32::UI::Controls::ICC_TREEVIEW_CLASSES;
    super::sys::init_common_controls(ICC_TREEVIEW_CLASSES);
    let _ = Class::new(CLASS).register();
    let host = class::create(
        &Create {
            ex_style: 0,
            class: CLASS,
            title: "",
            style: WS_POPUP,
            rect: (0, 0, 200, 200),
            parent: core::ptr::null_mut(),
            menu: core::ptr::null_mut(),
        },
        parent,
    );
    assert!(!host.is_null());
    let tree = class::create_plain(&Create {
        ex_style: 0,
        class: super::tv::CLASS,
        title: "",
        style: WS_CHILD,
        rect: (0, 0, 200, 200),
        parent: host,
        menu: class::id_menu(1),
    });
    assert!(!tree.is_null());
    (tree, host)
}

#[test]
fn a_tree_item_acts_on_its_own_tree_and_only_while_it_is_there() {
    use super::tv::{self, After, Item, Select};
    use windows_sys::Win32::UI::Controls::{
        TVIF_EXPANDEDIMAGE, TVIF_INTEGRAL, TVIF_PARAM, TVIF_STATEEX, TVITEMEXW, TVITEMW,
        TVN_DELETEITEMW,
    };
    let item = |param: isize| TVITEMEXW {
        mask: TVIF_PARAM,
        lParam: param,
        ..Default::default()
    };
    // What the parent of tree A can do with A's first item while A deletes
    // its items, one by one.
    let watched: Rc<Cell<Option<Item>>> = Rc::new(Cell::new(None));
    let during = Rc::new(RefCell::new(Vec::new()));
    let (w, d) = (Rc::clone(&watched), Rc::clone(&during));
    let (a, host_a) = hidden_tree(move |m| {
        if let Some(n) = m.notify()
            && n.code() == TVN_DELETEITEMW
            && let Some(item) = w.get()
        {
            d.borrow_mut().push(tv::item_param(item));
        }
        m.def()
    });
    let (b, host_b) = hidden_tree(|m| m.def());
    let a1 = tv::insert(a, None, After::Last, item(1), Some("a1")).expect("a1");
    let a2 = tv::insert(a, Some(a1), After::Last, item(2), Some("a2")).expect("a2");
    let b1 = tv::insert(b, None, After::Last, item(10), Some("b1")).expect("b1");
    assert_eq!((a1.tree(), b1.tree()), (a, b));
    // Each item acts on its own tree.
    assert_eq!(tv::item_param(a2), Some(2));
    assert_eq!(tv::item_param(b1), Some(10));
    assert_eq!(tv::child(a1), Some(a2));
    // An item of A goes into nothing of B's.
    assert!(!tv::select(b, Select::Caret, Some(a1)));
    assert!(tv::insert(b, Some(a1), After::Last, item(3), Some("x")).is_none());
    assert!(tv::insert(b, None, After::Item(a1), item(3), Some("x")).is_none());
    // Nor after an item of another parent.
    assert!(tv::insert(a, None, After::Item(a2), item(3), Some("x")).is_none());
    assert!(tv::select(a, Select::Caret, Some(a2)));
    assert_eq!(tv::selection(a), Some(a2));
    // TVITEMEXW's own fields are never asked of a TVITEMW.
    let fields = TVITEMW {
        mask: TVIF_PARAM | TVIF_INTEGRAL | TVIF_STATEEX | TVIF_EXPANDEDIMAGE,
        ..Default::default()
    };
    let (got, _) = tv::get_item(a2, fields, 0).expect("read");
    assert_eq!(got.mask, TVIF_PARAM);
    assert_eq!(got.lParam, 2);
    assert!(tv::set_item(a2, fields, Some("a2 again")));
    // Deleted: refused from the moment it starts to go.
    watched.set(Some(a2));
    assert!(tv::delete_item(a1));
    assert_eq!(*during.borrow(), vec![None, None]);
    assert_eq!(tv::item_param(a1), None);
    assert!(!tv::delete_item(a2));
    assert_eq!(tv::child(a1), None);
    // B's goes on.
    assert_eq!(tv::item_param(b1), Some(10));
    // A tree destroyed takes its items with it.
    super::destroy(host_b);
    assert_eq!(tv::item_param(b1), None);
    assert!(!tv::delete_item(b1));
    super::destroy(host_a);
}

static FORGE: subclass::Key = subclass::Key::new("test forged length");

/// Subclasses `hwnd` to answer `len_msg` with 1, whatever the text: a
/// length a buffer would be sized by.
fn forge_length(hwnd: HWND, len_msg: u32) {
    assert!(subclass::set(hwnd, &FORGE, move |m| {
        if m.id == len_msg { 1 } else { m.def() }
    }));
}

#[test]
fn a_list_s_text_is_measured_by_its_own_class() {
    use super::{combo, listbox};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CB_GETLBTEXTLEN, CBS_DROPDOWNLIST, CBS_HASSTRINGS, LB_GETTEXTLEN, LBS_HASSTRINGS,
    };
    let long = "a text much longer than the one unit its subclass says";
    let (lb, parent) = hidden_control("ListBox", LBS_HASSTRINGS as u32);
    assert_eq!(listbox::add(lb, long), 0);
    forge_length(lb, LB_GETTEXTLEN);
    assert_eq!(listbox::text(lb, 0), long);
    assert_eq!(listbox::text(lb, 5), "");
    // Not a list box: nothing.
    assert_eq!(listbox::text(parent, 0), "");
    super::destroy(parent);

    let (c, parent) = hidden_control("ComboBox", (CBS_DROPDOWNLIST | CBS_HASSTRINGS) as u32);
    assert_eq!(combo::add(c, long), 0);
    forge_length(c, CB_GETLBTEXTLEN);
    let list = combo::info(c).expect("a combo box").hwndList;
    forge_length(list, LB_GETTEXTLEN);
    assert_eq!(combo::text(c, 0), long);
    assert_eq!(combo::text(parent, 0), "");
    super::destroy(parent);
}

#[test]
fn a_callback_item_s_text_is_copied_never_followed() {
    use super::lv;
    use windows_sys::Win32::UI::Controls::{
        LVIF_NORECOMPUTE, LVIF_TEXT, LVITEMW, LVM_INSERTITEMW, LVN_GETDISPINFOW, LVS_REPORT,
        NMLVDISPINFOW,
    };
    let _ = Class::new(CLASS).register();
    super::sys::init_common_controls(windows_sys::Win32::UI::Controls::ICC_LISTVIEW_CLASSES);
    let host = class::create(
        &Create {
            ex_style: 0,
            class: CLASS,
            title: "",
            style: WS_POPUP,
            rect: (0, 0, 200, 200),
            parent: core::ptr::null_mut(),
            menu: core::ptr::null_mut(),
        },
        |m| {
            if let Some(n) = m.notify()
                && n.code() == LVN_GETDISPINFOW
                && let Some(mut info) = n.get::<NMLVDISPINFOW>()
            {
                info.set_text(&"called back".encode_utf16().collect::<Vec<_>>());
                return 0;
            }
            m.def()
        },
    );
    let view = class::create_plain(&Create {
        ex_style: 0,
        class: lv::CLASS,
        title: "",
        style: WS_CHILD | LVS_REPORT,
        rect: (0, 0, 200, 200),
        parent: host,
        menu: class::id_menu(1),
    });
    let item = LVITEMW {
        mask: LVIF_TEXT,
        // LPSTR_TEXTCALLBACKW.
        pszText: core::ptr::without_provenance_mut(usize::MAX),
        ..Default::default()
    };
    // SAFETY: LVM_INSERTITEMW's lParam is an LVITEMW, here with the
    // callback marker as its text, read during the call.
    let at = unsafe { super::send_raw(view, LVM_INSERTITEMW, 0, (&raw const item) as isize) };
    assert_eq!(at, 0);
    // LVIF_NORECOMPUTE would give the marker back: it is dropped.
    let ask = LVITEMW {
        mask: LVIF_TEXT | LVIF_NORECOMPUTE,
        ..Default::default()
    };
    let (_, text) = lv::get_item(view, ask, 64).expect("the item");
    assert_eq!(text.as_deref(), Some("called back"));
    assert_eq!(lv::item_text(view, 0, 0, 64), "called back");
    assert_eq!(lv::item_text(view, 0, 0, 4), "cal");
    super::destroy(host);
}
