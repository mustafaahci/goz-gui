//! Tools > Options, laid out as Everything 1.5's dialog: a tree of pages on
//! the left, the chosen page on the right on a tab that bears its name,
//! Restore Defaults under it, and OK, Cancel and Apply. The dialog can be
//! resized and opens where it was (`options_x`, `_y`, `_wide`, `_high`,
//! `_maximized`), on the page shown last (`options_last_page`).
//!
//! Each page is a [`Page`], in a module of its own (`window/opts/*.rs`, the
//! list in `window/opts.rs`): it lays its controls out in a child dialog of
//! [`PAGE_W`] × [`PAGE_H`] dialog units (with [`Rows`]), fills them from the
//! window's settings, greys what does not apply, checks what was typed,
//! applies what it shows, and puts its defaults back. A page's window is
//! made the first time it is shown; OK and Apply check the pages changed
//! since they were filled or last applied, then apply them.
//!
//! Re-entrancy: the dialog is modal and pumps messages. Pages get the
//! window's `App` and may call it freely; nothing here holds a `RefCell`
//! borrow across a page's method, and a page must not hold one of its own
//! across a call that sends messages.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, EndPaint, HFONT,
    InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{
    ETDT_DISABLE, ETDT_ENABLE, ETDT_USETABTEXTURE, EnableThemeDialogTexture, HTREEITEM, NMHDR,
    NMTREEVIEWW, TCIF_TEXT, TCITEMW, TCM_GETITEMRECT, TCM_INSERTITEMW, TCM_SETITEMW, TVE_EXPAND,
    TVGN_CARET, TVI_LAST, TVI_ROOT, TVIF_PARAM, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0,
    TVITEMW, TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_INSERTITEMW, TVM_SELECTITEM, TVM_SETEXTENDEDSTYLE,
    TVN_SELCHANGEDW, TVS_EX_DOUBLEBUFFER, TVS_HASLINES, TVS_LINESATROOT, TVS_SHOWSELALWAYS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX,
    CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    EN_CHANGE, ES_AUTOHSCROLL, ES_NUMBER, EndDialog, GWLP_USERDATA, GetClientRect, GetDlgItem,
    GetWindowRect, IDCANCEL, IDOK, MB_ICONWARNING, MB_OK, MINMAXINFO, MapDialogRect, SW_HIDE,
    SW_SHOWNA, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, WM_COMMAND, WM_DESTROY, WM_ERASEBKGND, WM_GETFONT,
    WM_GETMINMAXINFO, WM_INITDIALOG, WM_NCDESTROY, WM_NOTIFY, WM_PAINT, WM_SETTINGCHANGE, WM_SIZE,
    WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_GROUP, WS_TABSTOP, WS_THICKFRAME, WS_VSCROLL,
};

use crate::dialog::{
    self, Class, GROW_X, GROW_Y, Look, MOVE_X, MOVE_Y, Template, Units, anchor, apply_theme,
    center_on_owner, common, state,
};
use crate::places::{self, Which};
use crate::theme::Palette;
use crate::wide::{window_text, wz};
use crate::window::App;

/// The room a page's rows have, in dialog units: right of the tree, inside
/// the tab, above Restore Defaults (which the dialog puts at the bottom
/// right of every page).
pub const PAGE_W: i16 = 272;
pub const PAGE_H: i16 = 238;
/// A page's window as the dialog's template places it, Restore Defaults
/// included.
const PAGE_AREA_H: i16 = 263;

/// The dialog's own size in dialog units: Everything's 576 × 512 pixel
/// client area at 100 %.
const DLG_W: i16 = 384;
const DLG_H: i16 = 315;

const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;
/// Everything's control ids. A page's own controls take any other id.
const TREE: u16 = 1000;
const APPLY: u16 = 1001;
const TAB: u16 = 1002;
pub const RESTORE: u16 = 1065;

/// Where the frame's controls go at the template's size, and how each
/// follows a larger dialog.
const TREE_AT: Units = (8, 7, 86, 279);
const TAB_AT: Units = (98, 7, 278, 279);
const PAGE_AT: Units = (101, 21, PAGE_W, PAGE_AREA_H);
/// Restore Defaults in a page, and its room from the page's bottom right
/// corner.
const RESTORE_SIZE: (i16, i16) = (70, 14);
const RESTORE_MARGIN: (i16, i16) = (7, 8);
const BUTTONS_AT: [(u16, Units); 3] = [
    (OK, (218, 293, 50, 14)),
    (CANCEL, (272, 293, 50, 14)),
    (APPLY, (326, 293, 50, 14)),
];

/// Everything's order of its pages (`Page::order`); a feature's page takes
/// the number of its place there. Under General: UI, Home, Search,
/// Results, View, Context Menu, Fonts and Colors, Keyboard. Then History,
/// Indexes and its pages, Plug-ins, Advanced.
#[allow(dead_code, reason = "the places of the features' pages (FRAMEWORK.md)")]
pub mod order {
    pub const GENERAL: u32 = 0;
    pub const UI: u32 = 10;
    pub const HOME: u32 = 20;
    pub const SEARCH: u32 = 30;
    pub const RESULTS: u32 = 40;
    pub const VIEW: u32 = 50;
    pub const CONTEXT_MENU: u32 = 60;
    pub const FONTS_AND_COLORS: u32 = 70;
    pub const KEYBOARD: u32 = 80;
    pub const HISTORY: u32 = 100;
    pub const INDEXES: u32 = 200;
    pub const ADVANCED: u32 = 900;
}

/// Why OK or Apply cannot go on: the control to put the keyboard in, and
/// what to tell the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub control: u16,
    pub message: String,
}

/// One page of the dialog. Every method runs on the UI thread with the
/// page's window (`page`, a child dialog) made from [`Page::build`]'s
/// template; control ids are the page's own (any but [`LABEL`], which
/// [`Rows`] gives labels, and [`RESTORE`], the dialog's Restore Defaults).
pub trait Page {
    /// Its name in the tree and on its tab: Everything's page name. Also
    /// what `options_last_page` keeps.
    fn name(&self) -> &'static str;

    /// The page it sits under in the tree (Everything's General holds UI,
    /// Home, Search, Results, View, ... and Keyboard), or `None` at the top.
    fn parent(&self) -> Option<&'static str> {
        None
    }

    /// Its place among the pages: `order::*`.
    fn order(&self) -> u32;

    /// Lays out its controls, top to bottom.
    fn build(&self, rows: &mut Rows<'_>);

    /// Sets its controls from the window's settings as they are now, as
    /// the page is first shown. (No change is noted meanwhile.)
    fn fill(&self, app: &App, page: HWND);

    /// Greys what does not apply: after `fill`, `defaults` and every
    /// change on the page.
    fn update_enabled(&self, _page: HWND) {}

    /// Whether a `WM_COMMAND` from control `id` with notification `code`
    /// is a change (Apply lights up and the page is applied at OK). By
    /// default a click, a pick in a list or typing.
    fn changed(&self, _page: HWND, _id: u16, code: u32) -> bool {
        matches!(code, BN_CLICKED | CBN_SELCHANGE | EN_CHANGE)
    }

    /// Checks what was typed before anything is applied (OK, Apply).
    fn validate(&self, _page: HWND) -> Result<(), Invalid> {
        Ok(())
    }

    /// Applies what its controls show to the window, the settings file
    /// following: at OK or Apply, when one of them changed since the page
    /// was filled or last applied. What could not be done goes in
    /// `problems`, which the status bar then shows.
    fn apply(&self, app: &App, page: HWND, problems: &mut Vec<String>);

    /// Restore Defaults: puts the defaults in its controls (applied at OK
    /// or Apply like any change). (No change is noted meanwhile.)
    fn defaults(&self, page: HWND);

    /// Any message to the page's window, first: a button of its own (`Some`
    /// takes it, so it is no change), `WM_NOTIFY`, ... `Some` is the
    /// dialog procedure's answer.
    fn message(&self, _app: &App, _page: HWND, _msg: u32, _w: WPARAM, _l: LPARAM) -> Option<isize> {
        None
    }
}

/// Lays a page's controls out top to bottom in dialog units, with
/// Everything's spacing: 8 units in from the edges, a check box every 13
/// units, a label and its box every 17. Labels get id 0xffff.
pub struct Rows<'a> {
    t: &'a mut Template,
    x: i16,
    y: i16,
    right: i16,
}

/// A label's id: none.
pub const LABEL: u16 = 0xffff;

impl<'a> Rows<'a> {
    fn new(t: &'a mut Template) -> Rows<'a> {
        Rows {
            t,
            x: 8,
            y: 7,
            right: PAGE_W - 8,
        }
    }

    /// Where the next row goes.
    #[allow(dead_code, reason = "for the features' pages (FRAMEWORK.md)")]
    pub fn y(&self) -> i16 {
        self.y
    }

    /// Leaves `dy` units before the next row.
    pub fn skip(&mut self, dy: i16) -> &mut Self {
        self.y += dy;
        self
    }

    /// Moves the rows that follow `dx` units right (a negative `dx` back).
    #[allow(dead_code, reason = "for the features' pages (FRAMEWORK.md)")]
    pub fn indent(&mut self, dx: i16) -> &mut Self {
        self.x += dx;
        self
    }

    /// Any control at (`x`, `y`) from the row's left and top. The row does
    /// not move on.
    pub fn control(
        &mut self,
        class: Class,
        text: &str,
        id: u16,
        style: u32,
        ex_style: u32,
        (x, y, cx, cy): Units,
    ) -> &mut Self {
        self.t.control(
            class,
            text,
            id,
            style,
            ex_style,
            (self.x + x, self.y + y, cx, cy),
        );
        self
    }

    /// The width left of the row's left edge to the page's right margin.
    pub fn width(&self) -> i16 {
        self.right - self.x
    }

    /// A check box across the page.
    pub fn check(&mut self, id: u16, label: &str) -> &mut Self {
        let w = self.width();
        self.control(
            Class::Button,
            label,
            id,
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            (0, 0, w, 10),
        );
        self.skip(13)
    }

    /// A label `label_w` wide and a drop-down list `combo_w` wide right of
    /// it.
    pub fn combo(&mut self, label: &str, id: u16, label_w: i16, combo_w: i16) -> &mut Self {
        self.control(Class::Static, label, LABEL, SS_LEFT, 0, (0, 2, label_w, 9));
        self.control(
            Class::Named("COMBOBOX"),
            "",
            id,
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            0,
            (label_w, 0, combo_w, 120),
        );
        self.skip(17)
    }

    /// A label `label_w` wide and a hotkey box `w` wide right of it.
    pub fn hotkey(&mut self, label: &str, id: u16, label_w: i16, w: i16) -> &mut Self {
        self.control(Class::Static, label, LABEL, SS_LEFT, 0, (0, 2, label_w, 9));
        self.control(
            Class::Named("msctls_hotkey32"),
            "",
            id,
            WS_TABSTOP,
            WS_EX_CLIENTEDGE,
            (label_w, 0, w, 13),
        );
        self.skip(17)
    }

    /// A check box `check_w` wide, a number box after it, and the unit
    /// after that: Everything's "Keep search history for [90] days".
    pub fn check_number(
        &mut self,
        check: u16,
        label: &str,
        check_w: i16,
        number: u16,
        unit: &str,
    ) -> &mut Self {
        self.control(
            Class::Button,
            label,
            check,
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            (0, 2, check_w, 10),
        );
        self.control(
            Class::Edit,
            "",
            number,
            WS_TABSTOP | (ES_AUTOHSCROLL | ES_NUMBER) as u32,
            WS_EX_CLIENTEDGE,
            (check_w, 0, 30, 13),
        );
        let w = (self.width() - check_w - 34).max(0);
        self.control(
            Class::Static,
            unit,
            LABEL,
            SS_LEFT,
            0,
            (check_w + 34, 2, w, 9),
        );
        self.skip(16)
    }

    /// Text across the page, `lines` lines high.
    pub fn text(&mut self, text: &str, lines: i16) -> &mut Self {
        let w = self.width();
        self.control(Class::Static, text, LABEL, SS_LEFT, 0, (0, 0, w, 9 * lines));
        self.skip(9 * lines + 4)
    }

    /// A push button `w` wide.
    #[allow(dead_code, reason = "for the features' pages (FRAMEWORK.md)")]
    pub fn button(&mut self, id: u16, label: &str, w: i16) -> &mut Self {
        self.control(Class::Button, label, id, WS_TABSTOP, 0, (0, 0, w, 14));
        self.skip(18)
    }

    /// A group box named `label` around the rows `body` lays out (indented
    /// within it). `body` runs twice: once to measure, once for real.
    pub fn group(&mut self, label: &str, body: impl Fn(&mut Rows<'_>)) -> &mut Self {
        let mut scratch = Template::child(PAGE_W, PAGE_H);
        let inner = {
            let mut r = Rows {
                t: &mut scratch,
                x: self.x + 8,
                y: self.y + 12,
                right: self.right - 8,
            };
            body(&mut r);
            r.y
        };
        let h = inner - self.y + 2;
        let w = self.width();
        self.control(
            Class::Button,
            label,
            LABEL,
            BS_GROUPBOX as u32,
            0,
            (0, 0, w, h),
        );
        let mut r = Rows {
            t: &mut *self.t,
            x: self.x + 8,
            y: self.y + 12,
            right: self.right - 8,
        };
        body(&mut r);
        self.skip(h + 7)
    }
}

// ---- What pages use on their controls ----

/// Control `id` of `page`.
pub fn item(page: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(page, i32::from(id)) }
}

/// Sends `msg` to control `id` of `page`.
pub fn send(page: HWND, id: u16, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a control of this thread's dialog; any pointer
    // in `l` is the caller's and outlives the synchronous call.
    unsafe { SendMessageW(item(page, id), msg, w, l) }
}

pub fn set_check(page: HWND, id: u16, on: bool) {
    send(page, id, BM_SETCHECK, usize::from(on), 0);
}

pub fn checked(page: HWND, id: u16) -> bool {
    send(page, id, BM_GETCHECK, 0, 0) == 1
}

pub fn enable(page: HWND, id: u16, on: bool) {
    // SAFETY: a control of this thread's dialog.
    unsafe { EnableWindow(item(page, id), on.into()) };
}

/// Fills a drop-down list with `items`' labels and selects `current`'s.
pub fn fill_combo<T: Copy + PartialEq>(page: HWND, id: u16, items: &[(T, &str)], current: T) {
    send(page, id, CB_RESETCONTENT, 0, 0);
    for (_, label) in items {
        let text = wz(label);
        send(page, id, CB_ADDSTRING, 0, text.as_ptr() as isize);
    }
    select_combo(page, id, items, current);
}

/// Selects `current`'s label in a list [`fill_combo`] filled.
pub fn select_combo<T: Copy + PartialEq>(page: HWND, id: u16, items: &[(T, &str)], current: T) {
    let at = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
    send(page, id, CB_SETCURSEL, at, 0);
}

/// The value of the label selected in a list [`fill_combo`] filled.
pub fn combo_value<T: Copy>(page: HWND, id: u16, items: &[(T, &str)]) -> T {
    let at = send(page, id, CB_GETCURSEL, 0, 0).max(0) as usize;
    items[at.min(items.len() - 1)].0
}

pub fn set_text(page: HWND, id: u16, text: &str) {
    let text = wz(text);
    // SAFETY: a NUL-terminated string for a control of this thread's dialog.
    unsafe { SetWindowTextW(item(page, id), text.as_ptr()) };
}

pub fn text(page: HWND, id: u16) -> String {
    window_text(item(page, id))
}

/// Days as typed, within 1..=36500; `fallback` if not a number.
pub fn days(text: &str, fallback: u32) -> u32 {
    text.trim()
        .parse::<u32>()
        .map_or(fallback, |d| d.clamp(1, 36_500))
}

// ---- The tree's order ----

/// Where each page goes in the tree, top to bottom: its index in `pages`
/// (name, parent, order) and whether it is under another. Top pages go by
/// their order, each followed by those under it by theirs; a page under a
/// page that is not there goes at the top.
fn tree_order(pages: &[(&str, Option<&str>, u32)]) -> Vec<(usize, bool)> {
    let known = |name: &str| pages.iter().any(|&(n, ..)| n == name);
    let top_of = |i: usize| match pages[i].1 {
        Some(p) if known(p) && p != pages[i].0 => Some(p),
        _ => None,
    };
    let mut tops: Vec<usize> = (0..pages.len()).filter(|&i| top_of(i).is_none()).collect();
    tops.sort_by_key(|&i| (pages[i].2, i));
    let mut out = Vec::with_capacity(pages.len());
    for t in tops {
        out.push((t, false));
        let mut under: Vec<usize> = (0..pages.len())
            .filter(|&i| top_of(i) == Some(pages[t].0))
            .collect();
        under.sort_by_key(|&i| (pages[i].2, i));
        out.extend(under.into_iter().map(|i| (i, true)));
    }
    out
}

// ---- The dialog ----

/// A page as the dialog holds it.
struct Held {
    page: Box<dyn Page>,
    /// Its window, once shown.
    hwnd: Cell<HWND>,
    /// Changed since it was filled or last applied.
    dirty: Cell<bool>,
    item: Cell<HTREEITEM>,
}

/// What a page's window knows of the dialog (its GWLP_USERDATA).
struct PageRef {
    dialog: *const Dialog<'static>,
    index: usize,
}

struct Dialog<'a> {
    app: &'a App,
    pages: Vec<Held>,
    /// One per page, made once: their places stay put.
    refs: Vec<PageRef>,
    /// The page shown.
    current: Cell<Option<usize>>,
    look: Cell<Look>,
    /// Controls being set by the dialog itself: no change to note.
    filling: Cell<bool>,
    /// The dialog's least size, its template's.
    min: Cell<(i32, i32)>,
    frame: Cell<HWND>,
}

/// Shows the dialog over the window, with `pages` in the tree, on the
/// page named `first` (the first page if none is). Returns the name of the
/// page shown last.
pub fn show(app: &App, pages: Vec<Box<dyn Page>>, first: &str) -> String {
    let order = tree_order(
        &pages
            .iter()
            .map(|p| (p.name(), p.parent(), p.order()))
            .collect::<Vec<_>>(),
    );
    let pages: Vec<Held> = {
        let mut slots: Vec<Option<Box<dyn Page>>> = pages.into_iter().map(Some).collect();
        order
            .iter()
            .filter_map(|&(i, _)| slots[i].take())
            .map(|page| Held {
                page,
                hwnd: Cell::new(core::ptr::null_mut()),
                dirty: Cell::new(false),
                item: Cell::new(0),
            })
            .collect()
    };
    let nested: Vec<bool> = order.iter().map(|&(_, n)| n).collect();
    let start = pages
        .iter()
        .position(|h| h.page.name().eq_ignore_ascii_case(first))
        .unwrap_or(0);
    let mut dialog = Dialog {
        app,
        pages,
        refs: Vec::new(),
        current: Cell::new(None),
        look: Cell::new(dialog::app_look()),
        filling: Cell::new(false),
        min: Cell::new((0, 0)),
        frame: Cell::new(core::ptr::null_mut()),
    };
    // The pages' windows find the dialog through these; it stays where it
    // is until `show` returns, after they are gone.
    let me = (&raw const dialog).cast::<Dialog<'static>>();
    dialog.refs = (0..dialog.pages.len())
        .map(|index| PageRef { dialog: me, index })
        .collect();

    let mut t = Template::new("Options", WS_THICKFRAME, DLG_W, DLG_H);
    let tree_style = WS_TABSTOP | TVS_HASLINES | TVS_LINESATROOT | TVS_SHOWSELALWAYS;
    t.control(
        Class::Named("SysTreeView32"),
        "",
        TREE,
        tree_style | WS_GROUP,
        WS_EX_CLIENTEDGE,
        TREE_AT,
    );
    for (id, at) in BUTTONS_AT {
        let (label, style) = match id {
            OK => ("OK", WS_TABSTOP | WS_GROUP | BS_DEFPUSHBUTTON as u32),
            CANCEL => ("Cancel", WS_TABSTOP),
            _ => ("&Apply", WS_TABSTOP),
        };
        t.control(Class::Button, label, id, style, 0, at);
    }
    // Last, so at the bottom of the z-order: everything else is drawn over
    // it, the pages included. No tab stop: it only names the page.
    t.control(
        Class::Named("SysTabControl32"),
        "",
        TAB,
        WS_CLIPSIBLINGS,
        0,
        TAB_AT,
    );
    let state = Init {
        dialog: &dialog,
        nested,
        start,
    };
    t.run(app.hwnd, Some(frame_proc), (&raw const state) as LPARAM);
    let last = dialog.current.get().unwrap_or(start);
    dialog
        .pages
        .get(last)
        .map(|h| h.page.name().to_owned())
        .unwrap_or_default()
}

/// What WM_INITDIALOG gets.
struct Init<'a, 'b> {
    dialog: &'a Dialog<'b>,
    /// Each page (in the dialog's order) sits under the one before it.
    nested: Vec<bool>,
    start: usize,
}

fn ctrl(dlg: HWND, id: u16) -> HWND {
    item(dlg, id)
}

/// `units` in pixels of `dlg`.
fn px(dlg: HWND, (x, y, cx, cy): Units) -> RECT {
    let mut r = RECT {
        left: x.into(),
        top: y.into(),
        right: i32::from(x) + i32::from(cx),
        bottom: i32::from(y) + i32::from(cy),
    };
    // SAFETY: converts within this thread's dialog.
    unsafe { MapDialogRect(dlg, &mut r) };
    r
}

impl Dialog<'_> {
    fn init(&self, dlg: HWND, nested: &[bool], start: usize) {
        self.frame.set(dlg);
        let tree = ctrl(dlg, TREE);
        send(
            dlg,
            TREE,
            TVM_SETEXTENDEDSTYLE,
            TVS_EX_DOUBLEBUFFER as usize,
            TVS_EX_DOUBLEBUFFER as isize,
        );
        let mut parent: HTREEITEM = TVI_ROOT;
        let mut tops = Vec::new();
        for (i, held) in self.pages.iter().enumerate() {
            let under = nested.get(i).copied().unwrap_or(false);
            let mut text = wz(held.page.name());
            let ins = TVINSERTSTRUCTW {
                hParent: if under { parent } else { TVI_ROOT },
                hInsertAfter: TVI_LAST,
                Anonymous: TVINSERTSTRUCTW_0 {
                    item: TVITEMW {
                        mask: TVIF_TEXT | TVIF_PARAM,
                        pszText: text.as_mut_ptr(),
                        lParam: i as LPARAM,
                        // SAFETY: the rest of TVITEMW is plain data.
                        ..unsafe { core::mem::zeroed() }
                    },
                },
            };
            // SAFETY: the struct and its text outlive the synchronous call.
            let h = unsafe { SendMessageW(tree, TVM_INSERTITEMW, 0, (&raw const ins) as isize) };
            held.item.set(h);
            if !under {
                parent = h;
                tops.push(h);
            }
        }
        // Everything shows every page's place open.
        for h in tops {
            // SAFETY: an item of this thread's tree.
            unsafe { SendMessageW(tree, TVM_EXPAND, TVE_EXPAND as usize, h) };
        }
        // The tab that names the page.
        let mut name = wz("");
        let tab = TCITEMW {
            mask: TCIF_TEXT,
            pszText: name.as_mut_ptr(),
            // SAFETY: the rest of TCITEMW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        send(dlg, TAB, TCM_INSERTITEMW, 0, (&raw const tab) as isize);
        // SAFETY: the subclass's data is this dialog, which outlives it
        // (the subclass goes at the tab's WM_NCDESTROY, before `show`
        // returns).
        unsafe {
            SetWindowSubclass(
                ctrl(dlg, TAB),
                Some(tab_proc),
                1,
                (self as *const Dialog<'_>).cast::<()>() as usize,
            )
        };
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::Options, core::ptr::null_mut(), self.min.get());
        self.layout(dlg);
        enable(dlg, APPLY, false);
        self.select(start);
    }

    /// Selects page `i` in the tree, which shows it (TVN_SELCHANGED).
    fn select(&self, i: usize) {
        let Some(held) = self.pages.get(i) else {
            return;
        };
        let tree = ctrl(self.frame.get(), TREE);
        // SAFETY: an item of this thread's tree.
        unsafe {
            SendMessageW(tree, TVM_SELECTITEM, TVGN_CARET as usize, held.item.get());
            SendMessageW(tree, TVM_ENSUREVISIBLE, 0, held.item.get());
        }
        // Selecting the item already selected notifies nothing.
        if self.current.get() != Some(i) {
            self.show_page(i);
        }
    }

    /// The page area: right of the tree, inside the tab, above Restore
    /// Defaults, as the dialog is sized now.
    fn page_rect(&self, dlg: HWND) -> RECT {
        let mut client = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetClientRect(dlg, &mut client) };
        let base = px(dlg, (0, 0, DLG_W, DLG_H));
        let grew = (
            (client.right - base.right).max(0),
            (client.bottom - base.bottom).max(0),
        );
        let r = px(dlg, PAGE_AT);
        RECT {
            right: r.right + grew.0,
            bottom: r.bottom + grew.1,
            ..r
        }
    }

    fn layout(&self, dlg: HWND) {
        let mut controls = vec![(TREE, TREE_AT, GROW_Y), (TAB, TAB_AT, GROW_X | GROW_Y)];
        controls.extend(BUTTONS_AT.map(|(id, at)| (id, at, MOVE_X | MOVE_Y)));
        anchor(dlg, (DLG_W, DLG_H), &controls);
        let r = self.page_rect(dlg);
        for held in &self.pages {
            let h = held.hwnd.get();
            if !h.is_null() {
                // SAFETY: moves this thread's page.
                unsafe {
                    SetWindowPos(
                        h,
                        core::ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                };
            }
        }
        // SAFETY: the tab's frame is drawn for the whole of it.
        unsafe { InvalidateRect(ctrl(dlg, TAB), core::ptr::null(), 1) };
    }

    /// Makes page `i`'s window: its controls, filled and themed.
    fn make_page(&self, i: usize) -> HWND {
        let held = &self.pages[i];
        let mut t = Template::child(PAGE_W, PAGE_AREA_H);
        held.page.build(&mut Rows::new(&mut t));
        // Restore Defaults, last in the page's Tab order as Everything's.
        let (w, h) = RESTORE_SIZE;
        let (mx, my) = RESTORE_MARGIN;
        t.control(
            Class::Button,
            "&Restore Defaults",
            RESTORE,
            WS_TABSTOP | WS_GROUP,
            0,
            (PAGE_W - mx - w, PAGE_AREA_H - my - h, w, h),
        );
        let dlg = self.frame.get();
        let hwnd = t.create_child(dlg, Some(page_proc), (&raw const self.refs[i]) as LPARAM);
        if hwnd.is_null() {
            return hwnd;
        }
        held.hwnd.set(hwnd);
        self.filling.set(true);
        held.page.fill(self.app, hwnd);
        self.filling.set(false);
        held.page.update_enabled(hwnd);
        self.theme_page(hwnd, self.look.get().dark);
        let r = self.page_rect(dlg);
        // SAFETY: places the page over the tab, right after the tree in
        // the z-order: its controls come next in the Tab order, and it is
        // drawn over the tab.
        unsafe {
            SetWindowPos(
                hwnd,
                ctrl(dlg, TREE),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOACTIVATE,
            )
        };
        hwnd
    }

    fn theme_page(&self, page: HWND, dark: bool) {
        // SAFETY: the light theme draws the tab's own face behind the
        // page's controls, as on any tab; the dark theme paints its own.
        let flags = if dark {
            ETDT_DISABLE
        } else {
            ETDT_ENABLE | ETDT_USETABTEXTURE
        };
        // SAFETY: as above, on this thread's page.
        unsafe { EnableThemeDialogTexture(page, flags) };
        apply_theme(page, dark);
    }

    /// Shows page `i` (making it the first time) instead of the one shown.
    fn show_page(&self, i: usize) {
        let Some(held) = self.pages.get(i) else {
            return;
        };
        let mut hwnd = held.hwnd.get();
        if hwnd.is_null() {
            hwnd = self.make_page(i);
        }
        let old = self.current.replace(Some(i));
        // SAFETY: shows and hides this thread's pages.
        unsafe {
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_SHOWNA);
            }
            if let Some(o) = old.filter(|&o| o != i) {
                let h = self.pages[o].hwnd.get();
                if !h.is_null() {
                    ShowWindow(h, SW_HIDE);
                }
            }
        }
        let dlg = self.frame.get();
        let mut name = wz(held.page.name());
        let tab = TCITEMW {
            mask: TCIF_TEXT,
            pszText: name.as_mut_ptr(),
            // SAFETY: the rest of TCITEMW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        send(dlg, TAB, TCM_SETITEMW, 0, (&raw const tab) as isize);
        // SAFETY: repaints this thread's tab.
        unsafe { InvalidateRect(ctrl(dlg, TAB), core::ptr::null(), 1) };
    }

    /// A control of page `i` changed.
    fn page_changed(&self, i: usize) {
        let held = &self.pages[i];
        held.dirty.set(true);
        held.page.update_enabled(held.hwnd.get());
        enable(self.frame.get(), APPLY, true);
    }

    /// OK and Apply: checks the pages changed, then applies them. False
    /// when a check failed (its page is shown, the user told why).
    fn apply(&self) -> bool {
        let dlg = self.frame.get();
        for (i, held) in self.pages.iter().enumerate() {
            let h = held.hwnd.get();
            if h.is_null() || !held.dirty.get() {
                continue;
            }
            if let Err(bad) = held.page.validate(h) {
                self.select(i);
                crate::msgbox::show(dlg, &bad.message, "Options", MB_OK | MB_ICONWARNING);
                let c = item(h, bad.control);
                if !c.is_null() {
                    // SAFETY: focuses a control of this thread's page.
                    unsafe { SetFocus(c) };
                }
                return false;
            }
        }
        let mut problems = Vec::new();
        let mut any = false;
        for held in &self.pages {
            let h = held.hwnd.get();
            if h.is_null() || !held.dirty.replace(false) {
                continue;
            }
            held.page.apply(self.app, h, &mut problems);
            any = true;
        }
        enable(dlg, APPLY, false);
        if any {
            self.app.options_applied(problems);
        }
        // The theme may have changed: the dialog follows the window's.
        let look = dialog::app_look();
        if look != self.look.get() {
            self.retheme(look);
        }
        true
    }

    fn retheme(&self, look: Look) {
        self.look.set(look);
        let dlg = self.frame.get();
        apply_theme(dlg, look.dark);
        for held in &self.pages {
            let h = held.hwnd.get();
            if !h.is_null() {
                self.theme_page(h, look.dark);
            }
        }
    }

    /// Restore Defaults on page `i`.
    fn restore_defaults(&self, i: usize) {
        let held = &self.pages[i];
        let h = held.hwnd.get();
        if h.is_null() {
            return;
        }
        self.filling.set(true);
        held.page.defaults(h);
        self.filling.set(false);
        self.page_changed(i);
    }
}

unsafe extern "system" fn frame_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Init` that `show` keeps alive through the
        // modal call; its dialog outlives the window.
        let init = unsafe { &*(l as *const Init<'_, '_>) };
        let d = init.dialog;
        // SAFETY: as above.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, (&raw const *d) as isize) };
        d.init(dlg, &init.nested, init.start);
        return 1;
    }
    // SAFETY: GWLP_USERDATA is the `Dialog` from `show`, or null.
    let Some(d) = (unsafe { state::<Dialog<'_>>(dlg) }) else {
        return 0;
    };
    if msg == WM_SETTINGCHANGE {
        if let Some(new) = d.look.get().after_setting_change(l) {
            d.retheme(new);
        }
        return 1;
    }
    if let Some(answer) = common(dlg, msg, w, l, &d.look) {
        return answer;
    }
    match msg {
        WM_SIZE => {
            d.layout(dlg);
            1
        }
        WM_GETMINMAXINFO => {
            let (mw, mh) = d.min.get();
            // SAFETY: WM_GETMINMAXINFO's lParam is a MINMAXINFO to fill.
            let mmi = unsafe { &mut *(l as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = mw;
            mmi.ptMinTrackSize.y = mh;
            1
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lParam is an NMHDR from a control.
            let hdr = unsafe { &*(l as *const NMHDR) };
            if hdr.idFrom == usize::from(TREE) && hdr.code == TVN_SELCHANGEDW {
                // SAFETY: TVN_SELCHANGED's lParam is an NMTREEVIEWW.
                let nm = unsafe { &*(l as *const NMTREEVIEWW) };
                if let Ok(i) = usize::try_from(nm.itemNew.lParam) {
                    d.show_page(i);
                }
            }
            0
        }
        WM_COMMAND => {
            match (w & 0xffff) as u16 {
                OK => {
                    if d.apply() {
                        // SAFETY: ends this modal dialog.
                        unsafe { EndDialog(dlg, 1) };
                    }
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                APPLY => {
                    d.apply();
                }
                _ => return 0,
            }
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::Options, core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

unsafe extern "system" fn page_proc(page: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is one of the dialog's `PageRef`s, which stay where
        // they are until the dialog has ended.
        unsafe { SetWindowLongPtrW(page, GWLP_USERDATA, l) };
        return 0;
    }
    // SAFETY: GWLP_USERDATA is a `PageRef` from `make_page`, or null.
    let Some(r) = (unsafe { state::<PageRef>(page) }) else {
        return 0;
    };
    // SAFETY: the dialog outlives its pages' windows (they are its
    // children).
    let d = unsafe { &*r.dialog };
    let held = &d.pages[r.index];
    match msg {
        // Only top-level windows hear of it; the frame re-themes the pages.
        WM_SETTINGCHANGE => return 0,
        // Restore Defaults keeps to the page's bottom right corner.
        WM_SIZE => {
            place_restore(page);
            return 1;
        }
        WM_COMMAND if (w & 0xffff) as u16 == RESTORE => {
            if ((w >> 16) & 0xffff) as u32 == BN_CLICKED {
                d.restore_defaults(r.index);
            }
            return 1;
        }
        _ => {}
    }
    if let Some(answer) = held.page.message(d.app, page, msg, w, l) {
        return answer;
    }
    if let Some(answer) = common(page, msg, w, l, &d.look) {
        return answer;
    }
    if msg == WM_COMMAND && !d.filling.get() {
        let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
        if held.page.changed(page, id, code) {
            d.page_changed(r.index);
        }
        return 1;
    }
    0
}

/// Puts a page's Restore Defaults at its bottom right corner, as far in
/// as the template has it.
fn place_restore(page: HWND) {
    let button = item(page, RESTORE);
    if button.is_null() {
        return;
    }
    let (w, h) = RESTORE_SIZE;
    let (mx, my) = RESTORE_MARGIN;
    let room = px(page, (0, 0, w + mx, h + my));
    let size = px(page, (0, 0, w, h));
    let mut client = RECT::default();
    // SAFETY: reads and moves this thread's page and its button.
    unsafe {
        GetClientRect(page, &mut client);
        SetWindowPos(
            button,
            core::ptr::null_mut(),
            client.right - room.right,
            client.bottom - room.bottom,
            size.right,
            size.bottom,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

/// The tab that names the page: Windows' own in the light theme; in the
/// dark one drawn here (the control has no dark style), as a tab in the
/// header's colour joined to a frame around the page.
unsafe extern "system" fn tab_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the dialog, which outlives the subclass (removed
    // at WM_NCDESTROY); the messages' parameters are read as defined.
    unsafe {
        let d = &*(data as *const Dialog<'_>);
        match msg {
            WM_ERASEBKGND if d.look.get().dark => return 1,
            WM_PAINT if d.look.get().dark => {
                paint_dark_tab(hwnd, d);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(tab_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

fn paint_dark_tab(hwnd: HWND, d: &Dialog<'_>) {
    let pal = Palette::new(true);
    // SAFETY: a standard WM_PAINT on this thread's tab; the font selected
    // is put back.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        crate::paint::fill(hdc, &client, pal.chrome);
        let mut tab = RECT::default();
        SendMessageW(hwnd, TCM_GETITEMRECT, 0, (&raw mut tab) as isize);
        // The frame around the page, from just under the tab.
        let frame = RECT {
            left: client.left,
            top: tab.bottom,
            right: client.right,
            bottom: client.bottom,
        };
        let line = |r: RECT| crate::paint::fill(hdc, &r, pal.line);
        line(RECT {
            bottom: frame.top + 1,
            ..frame
        });
        line(RECT {
            top: frame.bottom - 1,
            ..frame
        });
        line(RECT {
            right: frame.left + 1,
            ..frame
        });
        line(RECT {
            left: frame.right - 1,
            ..frame
        });
        // The tab, a little taller than the row, open towards the page.
        let t = RECT {
            top: tab.top - 2,
            bottom: tab.bottom + 1,
            ..tab
        };
        crate::paint::fill(hdc, &t, pal.chrome);
        line(RECT {
            bottom: t.top + 1,
            ..t
        });
        line(RECT {
            right: t.left + 1,
            ..t
        });
        line(RECT {
            left: t.right - 1,
            ..t
        });
        let name = d
            .current
            .get()
            .map(|i| d.pages[i].page.name())
            .unwrap_or_default();
        let mut text: Vec<u16> = name.encode_utf16().collect();
        let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, pal.fg);
        let mut tr = t;
        DrawTextW(
            hdc,
            text.as_mut_ptr(),
            text.len() as i32,
            &mut tr,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
        EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_clamped_numbers() {
        assert_eq!(days("90", 7), 90);
        assert_eq!(days(" 0 ", 7), 1);
        assert_eq!(days("999999", 7), 36_500);
        assert_eq!(days("x", 7), 7);
    }

    #[test]
    fn the_tree_follows_everythings_order() {
        // Given in any order, as the features add them.
        let pages = [
            ("History", None, order::HISTORY),
            ("View", Some("General"), order::VIEW),
            ("General", None, order::GENERAL),
            ("Keyboard", Some("General"), order::KEYBOARD),
            ("UI", Some("General"), order::UI),
            ("Search", Some("General"), order::SEARCH),
        ];
        let names: Vec<(&str, bool)> = tree_order(&pages)
            .into_iter()
            .map(|(i, nested)| (pages[i].0, nested))
            .collect();
        assert_eq!(
            names,
            [
                ("General", false),
                ("UI", true),
                ("Search", true),
                ("View", true),
                ("Keyboard", true),
                ("History", false),
            ]
        );
        // A page under one that is not there goes at the top; so does one
        // under itself.
        let odd = [
            ("A", Some("Missing"), 5),
            ("B", None, 1),
            ("C", Some("C"), 3),
        ];
        let names: Vec<(&str, bool)> = tree_order(&odd)
            .into_iter()
            .map(|(i, nested)| (odd[i].0, nested))
            .collect();
        assert_eq!(names, [("B", false), ("C", false), ("A", false)]);
    }

    #[test]
    fn rows_stack_with_everythings_spacing() {
        let mut t = Template::child(PAGE_W, PAGE_H);
        let mut rows = Rows::new(&mut t);
        assert_eq!(rows.y(), 7);
        rows.check(100, "One").check(101, "Two");
        assert_eq!(rows.y(), 7 + 26);
        rows.combo("Label:", 102, 60, 100);
        assert_eq!(rows.y(), 7 + 26 + 17);
        let before = rows.y();
        rows.group("Group", |r| {
            r.check(103, "In");
            r.check(104, "In too");
        });
        // The box: 12 units of caption, two rows, 2 under them; 7 after.
        assert_eq!(rows.y(), before + (12 + 26 + 2) + 7);
        // The group's rows are indented within it.
        let mut t = Template::child(PAGE_W, PAGE_H);
        let mut rows = Rows::new(&mut t);
        rows.indent(10);
        assert_eq!(rows.width(), PAGE_W - 16 - 10);
    }
}
