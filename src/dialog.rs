//! Modal dialogs made in memory, with no resource script: a DLGTEMPLATE and
//! its controls laid out in dialog units. The dialog manager then brings the
//! keyboard (Tab, Enter, Esc, access keys), the message font, and with
//! per-monitor DPI awareness the scaling for each monitor. Also the colours
//! that make a dialog follow goz-gui's theme.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    DT_CALCRECT, DT_END_ELLIPSIS, DT_HIDEPREFIX, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    HBRUSH, HDC, MONITOR_DEFAULTTONULL, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE,
    TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    BP_CHECKBOX, BP_RADIOBUTTON, BST_CHECKED, BST_INDETERMINATE, CDDS_ITEMPREPAINT, CDDS_POSTPAINT,
    CDDS_PREPAINT, CDIS_DISABLED, CDIS_FOCUS, CDIS_HOT, CDIS_SELECTED, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, HOTKEYF_ALT,
    HOTKEYF_CONTROL, HOTKEYF_EXT, HOTKEYF_SHIFT, ILD_TRANSPARENT, LVCF_FMT, LVCF_TEXT,
    LVCFMT_JUSTIFYMASK, LVCFMT_RIGHT, LVIF_IMAGE, LVIR_BOUNDS, LVIR_ICON, LVIR_LABEL,
    LVIS_SELECTED, LVITEMW, LVSIL_SMALL, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMLVCUSTOMDRAW, ODS_FOCUS,
    ODS_NOFOCUSRECT, ODS_SELECTED, ODT_LISTBOX, TBCD_CHANNEL, TS_DRAW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_MENU, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_3STATE, BS_AUTO3STATE, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_CHECKBOX, BS_GROUPBOX,
    BS_RADIOBUTTON, BS_TYPEMASK, CB_GETEDITSEL, CB_MSGMAX, DS_CONTROL, DS_MODALFRAME, DS_SETFONT,
    ES_MULTILINE, GW_OWNER, LBS_HASSTRINGS, LBS_OWNERDRAWFIXED, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, UISF_HIDEACCEL, UISF_HIDEFOCUS,
    USER_TIMER_MINIMUM, WM_CHAR, WM_CTLCOLORBTN, WM_CTLCOLORDLG, WM_CTLCOLOREDIT,
    WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_ENABLE, WM_ERASEBKGND, WM_INITDIALOG,
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MOUSEWHEEL, WM_NCPAINT, WM_NOTIFY, WM_PAINT,
    WM_QUERYUISTATE, WM_SETTINGCHANGE, WM_SHOWWINDOW, WM_SYSKEYDOWN, WM_TIMER,
    WM_WINDOWPOSCHANGING, WS_BORDER, WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE,
    WS_EX_CONTROLPARENT, WS_POPUP, WS_SYSMENU, WS_VISIBLE,
};

use crate::paint::fill;
use crate::theme::{self, Palette};
use crate::win::class::{Create, create_plain, id_menu};
use crate::win::hook::{self, Event, Hook, Kind};
use crate::win::uxtheme::{self, Theme};
use crate::win::{
    self, Msg, Notify, button, combo, dlg, edit, gdi, header, listbox, lv, subclass, sys, tv,
};

pub use crate::win::dlg::{Class, Template};

impl Template {
    /// A modal dialog of `cx` × `cy` dialog units in the message font, with
    /// a title bar and a close box; `style` adds to that (`WS_THICKFRAME`).
    pub fn new(title: &str, style: u32, cx: i16, cy: i16) -> Template {
        let style =
            style | WS_POPUP | WS_CAPTION | WS_SYSMENU | (DS_MODALFRAME | DS_SETFONT) as u32;
        dialog_template(title, style, 0, (cx, cy))
    }

    /// A page of another dialog, `cx` × `cy` dialog units in the message
    /// font: a child dialog (`DS_CONTROL`, `WS_EX_CONTROLPARENT`) whose
    /// controls the parent's Tab, arrow and access keys reach as its own.
    /// Made hidden with [`Template::create_child`].
    pub fn child(cx: i16, cy: i16) -> Template {
        let style = WS_CHILD | WS_CLIPSIBLINGS | (DS_CONTROL | DS_SETFONT) as u32;
        dialog_template("", style, WS_EX_CONTROLPARENT, (cx, cy))
    }

    /// Shows the dialog, owned by `owner`, until it ends, its messages going
    /// to `handler` (a dialog procedure's answers: nonzero for a message it
    /// handled); returns what [`dlg::end`] was given (-1 if it could not be
    /// made).
    pub fn show(&self, owner: HWND, handler: &dyn Fn(&Msg) -> isize) -> isize {
        crate::trace::mark("dialog run");
        let ended = self.modal(owner, handler);
        crate::trace::mark("dialog ended");
        ended
    }
}

/// A dialog template in the message font, which dialogs use in Windows' own
/// programs, scaled by Options > UI's scale (which makes every dialog's
/// font, and so its dialog units, that much bigger).
fn dialog_template(title: &str, style: u32, ex_style: u32, size: (i16, i16)) -> Template {
    let (points, face) = message_font();
    let points = crate::custom::scaled_points(points);
    Template::with_styles(title, style, ex_style, size, points, &face)
}

/// The message font's face and size in points.
fn message_font() -> (u16, Vec<u16>) {
    let Some(ncm) = sys::non_client_metrics(96) else {
        return (9, "Segoe UI".encode_utf16().collect());
    };
    let lf = ncm.lfMessageFont;
    let face = win::until_nul(&lf.lfFaceName).to_vec();
    // At 96 DPI a point is 4/3 pixels.
    let points = (lf.lfHeight.unsigned_abs() * 72).div_ceil(96).max(1);
    (points as u16, face)
}

/// How a dialog looks: dark or light, and whether it follows the system's
/// app theme (View > Theme > User Default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    pub follow_system: bool,
}

impl Look {
    /// The look after `m`, if it changes it: only a `WM_SETTINGCHANGE` of
    /// the app theme, when the look follows the system.
    pub fn after_setting_change(self, m: &Msg) -> Option<Look> {
        if !self.follow_system || !m.setting_is("ImmersiveColorSet") {
            return None;
        }
        let dark = theme::system_dark();
        (dark != self.dark).then_some(Look { dark, ..self })
    }
}

thread_local! {
    /// How the window looks now, for the dialogs that are not handed a
    /// look (message boxes).
    static APP_LOOK: Cell<Option<Look>> = const { Cell::new(None) };
}

/// The window's look changed (or was first set): dialogs opened from now on
/// without a look of their own take it.
pub fn set_app_look(look: Look) {
    APP_LOOK.set(Some(look));
}

/// The window's look, as [`set_app_look`] last set it; the system's app
/// theme before any window set one.
pub fn app_look() -> Look {
    APP_LOOK.get().unwrap_or_else(|| Look {
        dark: theme::system_dark(),
        follow_system: true,
    })
}

/// Makes a dialog and its controls dark or light: the title bar, buttons,
/// text boxes and lists. Its face and text follow from [`ctl_color`].
pub fn apply_theme(dlg: HWND, dark: bool) {
    theme_controls(dlg, dark, false);
}

/// [`apply_theme`] for a dialog just made (from its WM_INITDIALOG): in the
/// light theme its buttons, boxes and combo boxes are left with the visual
/// style they were made with, which is the light one. Setting it again
/// (`SetWindowTheme`) makes each control load its theme over and measure
/// itself again: about half a millisecond a combo box.
pub fn theme_new(dlg: HWND, dark: bool) {
    theme_controls(dlg, dark, true);
}

fn theme_controls(dlg: HWND, dark: bool, fresh: bool) {
    theme::set_titlebar(dlg, dark);
    for child in win::children(dlg) {
        theme_control(child, dark, fresh);
    }
    win::redraw(
        dlg,
        None,
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
    );
}

/// The subclasses dialogs' controls get here.
static GROUP: subclass::Key = subclass::Key::new("dark group box");
static HOTKEY: subclass::Key = subclass::Key::new("dark hotkey box");
static EDGE: subclass::Key = subclass::Key::new("dark edge");
static LIST: subclass::Key = subclass::Key::new("dark list header");
static LATER: subclass::Key = subclass::Key::new("combo list later");
static QUIET_SHOW: subclass::Key = subclass::Key::new("shown without activating");

/// Makes one control of a dialog dark or light, as [`apply_theme`] does
/// each; `fresh`: just made (see [`theme_new`]). For a control a dialog
/// makes after its others.
pub fn theme_control(child: HWND, dark: bool, fresh: bool) {
    let pal = Palette::new(dark);
    let class = win::class_name(child);
    // Controls still as they were made, which is light.
    if fresh
        && !dark
        && matches!(
            class.as_str(),
            "Button" | "Edit" | "ComboBox" | "msctls_hotkey32"
        )
    {
        return;
    }
    theme::allow(child, dark);
    match class.as_str() {
        // A themed group box draws its caption in the theme's dark text
        // whatever WM_CTLCOLORSTATIC says, and an unthemed one a white
        // etched frame: in the dark theme it is drawn here.
        "Button" if win::style(child) & BS_TYPEMASK as u32 == BS_GROUPBOX as u32 => {
            theme::set_class(child, None);
            subclass::set(child, &GROUP, move |m| group_proc(m, dark));
        }
        "Button" => theme::set_class(child, dark.then_some("DarkMode_Explorer")),
        // A hotkey box draws white whatever its parent says: in the dark
        // theme it is drawn here, its edge too.
        "msctls_hotkey32" => {
            subclass::set(child, &HOTKEY, move |m| hotkey_proc(m, dark));
            win::set_pos(
                child,
                core::ptr::null_mut(),
                (0, 0, 0, 0),
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        // A box of several lines has a scroll bar, which the Explorer style
        // makes dark; its edge, which Windows draws white, as a one-line
        // box's (`edge_proc`).
        "Edit" if win::style(child) & ES_MULTILINE as u32 != 0 => {
            theme::set_class(child, dark.then_some("DarkMode_Explorer"));
            set_dark_edge(child, dark);
        }
        // A list box: dark scroll bars with the dark Explorer style, its
        // edge as a list view's (Windows draws it white), and its items
        // drawn here (`LIST_BOX`, `paint_list_item`): under a dark style
        // Windows draws the selected one white on white.
        "ListBox" => {
            if dark || !fresh {
                theme::set_class(child, dark.then_some("DarkMode_Explorer"));
            }
            set_dark_edge(child, dark);
            fit_list_items(child);
        }
        "Edit" => theme::set_class(child, dark.then_some("DarkMode_CFD")),
        // Its list's colours come from WM_CTLCOLORLISTBOX.
        "ComboBox" => {
            theme::set_class(child, dark.then_some("DarkMode_CFD"));
            // Restyled, a box one can type in selects its text, which a
            // combo box shows with or without the keyboard: none unless the
            // box has the keyboard (Fonts and Colors' Size).
            let box_edit = win::find_child(child, core::ptr::null_mut(), Some("Edit"), None);
            if !box_edit.is_null() && win::focus() != box_edit {
                edit::set_sel(box_edit, -1, 0);
            }
        }
        "SysTreeView32" => {
            theme::set_class(
                child,
                Some(if dark {
                    "DarkMode_Explorer"
                } else {
                    "Explorer"
                }),
            );
            tv::set_bk_color(child, pal.bg);
            tv::set_text_color(child, pal.fg);
            tv::set_line_color(child, pal.dim);
        }
        "SysListView32" => {
            // Dark scroll bars come with the dark Explorer style; it shows
            // no selection once the list loses the keyboard, so dialogs
            // draw that themselves (see `list_custom_draw`).
            theme::set_class(
                child,
                Some(if dark {
                    "DarkMode_Explorer"
                } else {
                    "Explorer"
                }),
            );
            let list_header = lv::header(child);
            theme::allow(list_header, dark);
            theme::set_class(list_header, dark.then_some("DarkMode_ItemsView"));
            lv::set_bk_color(child, pal.bg);
            lv::set_text_bk_color(child, pal.bg);
            lv::set_text_color(child, pal.fg);
            // Setting it again only replaces the closure, with the new
            // theme.
            subclass::set(child, &LIST, move |m| list_proc(m, dark));
        }
        _ => {}
    }
}

/// A combo box's list still to be put in ([`fill_later`]): its items, and
/// the one chosen.
struct Later {
    items: Vec<String>,
    select: Option<usize>,
    /// Whether the chosen item was put in ahead of the others.
    ahead: bool,
}

/// [`fill_later`]'s timer's id on the combo box.
const LATER_TIMER: usize = 0x6c61;

/// Gives combo box `combo` its list `items` (`select` chosen, if any), put
/// in only once wanted. A themed (v6) list takes about half a millisecond
/// an item, whether or not it redraws (measured: 20 history formats, 15 ms;
/// a WinForms v6 combo box alike), which made the dialogs that fill several
/// as they open take 50 to 100 ms. So only the chosen item goes in now, for
/// the box to show; the rest follow once the dialog has been drawn (a timer
/// set as it shows: timers come after every paint waiting), each box's in a
/// message of its own, or at once when the box is used first (a key, a click, the wheel)
/// or asked anything about its list (`CB_GETCURSEL` and every other `CB_*`
/// message), so no caller can tell.
pub fn fill_later(combo_box: HWND, items: Vec<String>, select: Option<usize>) {
    let chosen = select.and_then(|i| items.get(i));
    if let Some(text) = chosen {
        combo::add(combo_box, text);
        combo::set_cur_sel(combo_box, 0);
    }
    let ahead = chosen.is_some();
    // The subclass holds the list until it puts it in, or drops it with the
    // combo box.
    let later = Rc::new(RefCell::new(Some(Later {
        items,
        select,
        ahead,
    })));
    let held = Rc::clone(&later);
    if !subclass::set(combo_box, &LATER, move |m| later_proc(m, &held)) {
        // Not subclassed: in at once.
        fill_now(combo_box, &later);
    } else if win::is_visible(win::parent(combo_box)) {
        // Made once the dialog shows: the list follows its first paint.
        win::set_timer(combo_box, LATER_TIMER, USER_TIMER_MINIMUM);
    }
}

/// Dialog `dlg` is being shown: the lists still to come ([`fill_later`])
/// go in once it has been drawn. Not before: a modal dialog without
/// `WS_VISIBLE` is shown only once its message queue is empty, so a timer
/// due before that would make it show later, not sooner.
fn lists_after_show(dlg: HWND) {
    for child in win::children(dlg) {
        if subclass::has(child, &LATER) {
            win::set_timer(child, LATER_TIMER, USER_TIMER_MINIMUM);
        }
    }
}

/// Whether `msg` is one that needs the whole list in first: one that uses
/// the box (keys, mouse) or asks about or changes its list.
fn wants_list(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_MOUSEWHEEL
    ) || (CB_GETEDITSEL..=CB_MSGMAX).contains(&msg)
}

/// [`fill_later`]'s subclass: puts the list in when it is wanted.
fn later_proc(m: &Msg, later: &RefCell<Option<Later>>) -> isize {
    match m.id {
        WM_TIMER if m.w == LATER_TIMER => {
            fill_now(m.hwnd, later);
            return 0;
        }
        id if wants_list(id) => fill_now(m.hwnd, later),
        _ => {}
    }
    m.def()
}

/// Puts the list of `later` in `combo_box` now, unseen (the box shows the
/// same item after), and takes the subclass off (first, so these messages
/// do not come back to it).
fn fill_now(combo_box: HWND, later: &RefCell<Option<Later>>) {
    let Some(later) = later.borrow_mut().take() else {
        return;
    };
    subclass::remove(combo_box, &LATER);
    win::kill_timer(combo_box, LATER_TIMER);
    win::set_redraw(combo_box, false);
    if later.ahead {
        // Not CB_RESETCONTENT, which also empties an editable box.
        combo::delete(combo_box, 0);
    }
    for text in &later.items {
        combo::add(combo_box, text);
    }
    if let Some(i) = later.select {
        combo::set_cur_sel(combo_box, i as i32);
    }
    win::set_redraw(combo_box, true);
}

/// A group box's subclass: in the dark theme its frame is a line of the
/// theme's border colour and its caption the theme's text, as Windows draws
/// neither.
fn group_proc(m: &Msg, dark: bool) -> isize {
    match m.id {
        // The box is see-through: what is inside it is its parent's.
        WM_ERASEBKGND if dark => 1,
        WM_PAINT if dark => {
            paint_dark_group(m.hwnd);
            0
        }
        _ => m.def(),
    }
}

fn paint_dark_group(hwnd: HWND) {
    let pal = Palette::new(true);
    let paint = gdi::Paint::begin(hwnd);
    let hdc = paint.hdc();
    let rc = win::client_rect(hwnd);
    let _font = gdi::Selected::new(hdc, win::font(hwnd));
    let text: Vec<u16> = win::text_wide(hwnd);
    let size = gdi::text_extent(hdc, &text);
    let dpi = crate::custom::window_dpi(hwnd) as i32;
    let px = |v: i32| v * dpi / 96;
    // The frame runs through the caption's middle, broken for it.
    let top = rc.top + size.cy / 2;
    let caption = RECT {
        left: rc.left + px(7),
        top: rc.top,
        right: (rc.left + px(7) + size.cx + px(4)).min(rc.right),
        bottom: rc.top + size.cy,
    };
    let line = |r: RECT| fill(hdc, &r, pal.border);
    line(RECT {
        left: rc.left,
        top,
        right: caption.left,
        bottom: top + 1,
    });
    line(RECT {
        left: caption.right,
        top,
        right: rc.right,
        bottom: top + 1,
    });
    line(RECT {
        top,
        right: rc.left + 1,
        ..rc
    });
    line(RECT {
        top,
        left: rc.right - 1,
        ..rc
    });
    line(RECT {
        top: rc.bottom - 1,
        ..rc
    });
    if !text.is_empty() {
        fill(hdc, &caption, pal.chrome);
        gdi::set_bk_mode(hdc, TRANSPARENT as i32);
        gdi::set_text_color(hdc, pal.fg);
        let mut tr = RECT {
            left: caption.left + px(2),
            ..caption
        };
        gdi::draw_text(hdc, &text, &mut tr, DT_SINGLELINE | DT_NOPREFIX);
    }
}

/// A hotkey box's subclass: in the dark theme its face, its edge and the
/// key it holds ("Ctrl + Shift + F", or "None") are drawn here in the
/// theme's colours, the caret after the text.
fn hotkey_proc(m: &Msg, dark: bool) -> isize {
    match m.id {
        WM_ERASEBKGND if dark => 1,
        WM_PAINT if dark => {
            paint_dark_hotkey(m.hwnd);
            0
        }
        WM_NCPAINT if dark => {
            paint_dark_edge(m.hwnd);
            0
        }
        _ => m.def(),
    }
}

/// The name of a key as a hotkey box shows it: `key` is its
/// `HKM_GETHOTKEY` (virtual key low, `HOTKEYF_*` high).
fn hotkey_text(key: u16) -> String {
    let (vk, mods) = (key & 0xff, (key >> 8) as u8);
    let mut parts = Vec::new();
    for (flag, key) in [
        (HOTKEYF_CONTROL, VK_CONTROL),
        (HOTKEYF_SHIFT, VK_SHIFT),
        (HOTKEYF_ALT, VK_MENU),
    ] {
        if mods & flag as u8 != 0 {
            parts.push(sys::key_name(key, false));
        }
    }
    if vk != 0 {
        parts.push(sys::key_name(vk, mods & HOTKEYF_EXT as u8 != 0));
    } else if !parts.is_empty() {
        // Modifiers held so far, the key still to come.
        parts.push(String::new());
    }
    if parts.is_empty() {
        "None".to_owned()
    } else {
        parts.join(" + ")
    }
}

fn paint_dark_hotkey(hwnd: HWND) {
    let pal = Palette::new(true);
    let (x, y, cx) = {
        let paint = gdi::Paint::begin(hwnd);
        let hdc = paint.hdc();
        let rc = win::client_rect(hwnd);
        let enabled = win::is_enabled(hwnd);
        fill(hdc, &rc, if enabled { pal.bg } else { pal.chrome });
        let _font = gdi::Selected::new(hdc, win::font(hwnd));
        let text: Vec<u16> = hotkey_text(win::hotkey::get(hwnd)).encode_utf16().collect();
        let size = gdi::text_extent(hdc, &text);
        gdi::set_bk_mode(hdc, TRANSPARENT as i32);
        gdi::set_text_color(hdc, if enabled { pal.fg } else { pal.dim });
        let x = rc.left + 1;
        let y = rc.top + (rc.bottom - rc.top - size.cy) / 2;
        let mut tr = RECT {
            left: x,
            top: y,
            right: rc.right,
            bottom: y + size.cy,
        };
        gdi::draw_text(hdc, &text, &mut tr, DT_SINGLELINE | DT_NOPREFIX);
        (x, y, size.cx)
    };
    if win::focus() == hwnd {
        sys::set_caret_pos(x + cx, y);
    }
}

/// A one-line edge in the theme's border colour, the box's face between it
/// and the client area, for a control drawn here whose edge Windows draws
/// light.
fn paint_dark_edge(hwnd: HWND) {
    let pal = Palette::new(true);
    let wr = win::window_rect(hwnd);
    let client = win::client_rect(hwnd);
    let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
    let dc = gdi::WindowDc::whole(hwnd);
    let hdc = dc.hdc();
    let all = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    let face = if win::is_enabled(hwnd) {
        pal.bg
    } else {
        pal.chrome
    };
    // The frame's width either side of the client area.
    let ex = ((w - client.right) / 2).max(0);
    let ey = ((h - client.bottom) / 2).max(0);
    fill(hdc, &RECT { bottom: ey, ..all }, face);
    fill(hdc, &RECT { top: h - ey, ..all }, face);
    fill(hdc, &RECT { right: ex, ..all }, face);
    fill(
        hdc,
        &RECT {
            left: w - ex,
            ..all
        },
        face,
    );
    fill(hdc, &RECT { bottom: 1, ..all }, pal.border);
    fill(hdc, &RECT { top: h - 1, ..all }, pal.border);
    fill(hdc, &RECT { right: 1, ..all }, pal.border);
    fill(hdc, &RECT { left: w - 1, ..all }, pal.border);
}

/// [`edge_proc`]'s kinds of edge: Windows' own (light), a dark box's, or a
/// dark list's.
const EDGE_LIGHT: usize = 0;
const EDGE_BOX: usize = 1;
const EDGE_LIST: usize = 2;

/// Gives a list box or a box of several lines with an edge (a sunken
/// `WS_EX_CLIENTEDGE`, which a dialog also makes of `WS_BORDER`) the dark
/// theme's edge while `dark`, and Windows' own again otherwise: Windows
/// draws that edge white whatever the control's style, where a one-line
/// box, a list view or a tree has a thin one.
fn set_dark_edge(control: HWND, dark: bool) {
    let edged =
        win::ex_style(control) & WS_EX_CLIENTEDGE != 0 || win::style(control) & WS_BORDER != 0;
    if !edged {
        return;
    }
    let kind = match (dark, win::class_name(control).as_str()) {
        (false, _) => EDGE_LIGHT,
        (true, "ListBox") => EDGE_LIST,
        (true, _) => EDGE_BOX,
    };
    // Setting it again only replaces the closure, with the new kind.
    subclass::set(control, &EDGE, move |m| edge_proc(m, kind));
    win::redraw(control, None, RDW_FRAME | RDW_INVALIDATE);
}

/// The dark edge [`edge_proc`] draws, outer line first, then the line
/// inside it (the rest of the sunken edge's two pixels): as Windows draws
/// a dark one-line box's (enabled or greyed), or a list view's.
fn dark_edge_colours(kind: usize, enabled: bool) -> (u32, u32) {
    let pal = Palette::new(true);
    match (kind, enabled) {
        (EDGE_LIST, true) => (theme::rgb(0x82, 0x87, 0x90), pal.bg),
        (EDGE_LIST, false) => (theme::rgb(0x82, 0x87, 0x90), pal.chrome),
        (_, true) => (theme::rgb(0x9b, 0x9b, 0x9b), theme::rgb(0x38, 0x38, 0x38)),
        (_, false) => (theme::rgb(0x40, 0x40, 0x40), theme::rgb(0x38, 0x38, 0x38)),
    }
}

/// The subclass of [`set_dark_edge`]: in the dark theme, its edge drawn
/// over Windows' after Windows has drawn the frame (and any scroll bars
/// inside it), and again when the control is greyed or enabled.
fn edge_proc(m: &Msg, kind: usize) -> isize {
    let hwnd = m.hwnd;
    match m.id {
        WM_NCPAINT if kind != EDGE_LIGHT => {
            let done = m.def();
            let rc = win::window_rect(hwnd);
            let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
            // The edge's width: where the client area starts (a sunken edge
            // has two pixels, a border one; scroll bars are on the other
            // sides).
            let origin = win::client_to_screen(hwnd, POINT { x: 0, y: 0 });
            let thick = (origin.x - rc.left).min(origin.y - rc.top);
            let (outer, inner) = dark_edge_colours(kind, win::is_enabled(hwnd));
            let dc = gdi::WindowDc::whole(hwnd);
            for (inset, colour) in [(0, outer), (1, inner)] {
                if inset >= thick {
                    break;
                }
                let (l, t, r, b) = (inset, inset, w - inset, h - inset);
                for edge in [
                    RECT {
                        left: l,
                        top: t,
                        right: r,
                        bottom: t + 1,
                    },
                    RECT {
                        left: l,
                        top: b - 1,
                        right: r,
                        bottom: b,
                    },
                    RECT {
                        left: l,
                        top: t,
                        right: l + 1,
                        bottom: b,
                    },
                    RECT {
                        left: r - 1,
                        top: t,
                        right: r,
                        bottom: b,
                    },
                ] {
                    fill(dc.hdc(), &edge, colour);
                }
            }
            done
        }
        // Greyed or enabled: the edge's colours change too (the control
        // redraws only its inside).
        WM_ENABLE if kind != EDGE_LIGHT => {
            let done = m.def();
            win::redraw(hwnd, None, RDW_FRAME | RDW_INVALIDATE);
            done
        }
        _ => m.def(),
    }
}

/// A dialog list's subclass: in the dark theme it gives its header's item
/// text the theme's text colour, which the dark header style leaves dark.
/// The header reports its drawing to the list, its parent.
fn list_proc(m: &Msg, dark: bool) -> isize {
    if dark
        && let Some(n) = m.notify()
        && n.from() == lv::header(m.hwnd)
        && let Some(cd) = n.get::<NMCUSTOMDRAW>()
    {
        match cd.dwDrawStage {
            CDDS_PREPAINT => return CDRF_NOTIFYITEMDRAW as isize,
            CDDS_ITEMPREPAINT => {
                gdi::set_text_color(cd.hdc, Palette::new(true).fg);
                return CDRF_DODEFAULT as isize;
            }
            _ => {}
        }
    }
    m.def()
}

/// Sizes a dialog list's last column to take the width the columns before
/// it leave of the list's inside, at least as wide as its header's text.
///
/// That room changes with the list's size *and* with its rows: rows enough
/// to bring the vertical scroll bar (or few enough to take it away) change
/// it, so this follows both. The width is worked out here rather than with
/// `LVSCW_AUTOSIZE_USEHEADER`, which fills the room only while the rows are
/// narrower: with rows in, it sizes the last column to its widest text (a
/// built-in filter's list of extensions, a long path), and the list gets a
/// horizontal scroll bar over its last row.
pub fn fill_last_column(list: HWND) {
    let columns = header::item_count(lv::header(list));
    if columns == 0 {
        return;
    }
    let last = (columns - 1) as i32;
    let title = lv::get_column(list, last, LVCF_TEXT)
        .and_then(|(_, text)| text)
        .unwrap_or_default();
    let label = lv::string_width(list, &title);
    let floor = label + HEADER_MARGIN * crate::custom::window_dpi(list) as i32 / 96;
    // Fitting the column can take the horizontal bar away, and with it the
    // need for the vertical one: then once more for the new room (it
    // settles by the second time).
    for _ in 0..3 {
        let rc = win::client_rect(list);
        let others = (0..last).map(|c| lv::column_width(list, c)).sum();
        let wide = last_column_width(rc.right - rc.left, others, floor);
        if lv::column_width(list, last) == wide {
            break;
        }
        lv::set_column_width(list, last, wide);
    }
}

/// The room around a header's text, at 96 DPI (the header's margins).
const HEADER_MARGIN: i32 = 12;

/// The last column's width: what the other columns leave of the list's
/// inside, but never less than `floor` (its header's text) when the others
/// were made wider than the list.
fn last_column_width(inside: i32, others: i32, floor: i32) -> i32 {
    (inside - others).max(floor)
}

/// A dialog list's own `NM_CUSTOMDRAW` (which its dialog gets). Returns
/// the answer for DWLP_MSGRESULT.
///
/// In the dark theme selected rows are painted here in goz-gui's selection
/// colours, with the keyboard and without, as the results list shows them:
/// the dark style shows no selection once the list loses the keyboard, and
/// paints its own over any colours given. The light style's selection
/// colours are goz-gui's already. In both, the list is cleared below its
/// last row, where the style draws each column's edge on down (as the
/// results list does, `paint::below_rows`).
pub fn list_custom_draw(list: HWND, cd: &NMLVCUSTOMDRAW, dark: bool) -> isize {
    match cd.nmcd.dwDrawStage {
        CDDS_PREPAINT => (CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT) as isize,
        CDDS_ITEMPREPAINT if dark => {
            let row = cd.nmcd.dwItemSpec as i32;
            if lv::item_state(list, row, LVIS_SELECTED) & LVIS_SELECTED == 0 {
                return CDRF_DODEFAULT as isize;
            }
            paint_selected_row(list, cd.nmcd.hdc, row);
            CDRF_SKIPDEFAULT as isize
        }
        CDDS_POSTPAINT => {
            clear_below_rows(list, cd.nmcd.hdc, Palette::new(dark).bg);
            CDRF_DODEFAULT as isize
        }
        _ => CDRF_DODEFAULT as isize,
    }
}

/// Fills a list below its last row (or its header, when it has none).
fn clear_below_rows(list: HWND, hdc: HDC, bg: u32) {
    let client = win::client_rect(list);
    let header = win::window_rect(lv::header(list));
    let mut top = header.bottom - header.top;
    let count = lv::item_count(list);
    if count > 0
        && let Some(last) = lv::item_rect(list, count as i32 - 1, LVIR_BOUNDS as i32)
    {
        top = top.max(last.bottom);
    }
    if top < client.bottom {
        fill(hdc, &RECT { top, ..client }, bg);
    }
}

/// Paints a selected row of a dark dialog list: the row in the selection
/// colour, each column's text where the list puts it.
fn paint_selected_row(list: HWND, hdc: HDC, row: i32) {
    let pal = Palette::new(true);
    let focused = win::focus() == list;
    let bounds = lv::item_rect(list, row, LVIR_BOUNDS as i32).unwrap_or_default();
    fill(
        hdc,
        &bounds,
        if focused { pal.sel } else { pal.sel_inactive },
    );
    // The row's icon, when the list has them.
    let it = LVITEMW {
        mask: LVIF_IMAGE,
        iItem: row,
        ..Default::default()
    };
    if let Some((it, _)) = lv::get_item(list, it, 0)
        && it.iImage >= 0
        && let Some(icon) = lv::item_rect(list, row, LVIR_ICON as i32)
    {
        lv::draw_image(
            list,
            LVSIL_SMALL,
            it.iImage,
            hdc,
            (icon.left, icon.top),
            ILD_TRANSPARENT,
        );
    }
    let columns = header::item_count(lv::header(list)) as i32;
    let pad = |v: i32| v * crate::custom::window_dpi(list) as i32 / 96;
    gdi::set_bk_mode(hdc, TRANSPARENT as i32);
    gdi::set_text_color(hdc, pal.fg);
    for col in 0..columns {
        // Column 0's label leaves out the margin before it, as the list
        // draws its text.
        let part = if col == 0 { LVIR_LABEL } else { LVIR_BOUNDS } as i32;
        let mut rc = lv::subitem_rect(list, row, col, part).unwrap_or_default();
        let text: Vec<u16> = lv::item_text(list, row, col, 512).encode_utf16().collect();
        rc.left += pad(if col == 0 { LABEL_INSET } else { CELL_INSET });
        rc.right -= pad(CELL_INSET);
        // A right-aligned column (a count) stays right-aligned.
        let fmt = lv::get_column(list, col, LVCF_FMT).map_or(0, |(c, _)| c.fmt);
        let align = if fmt & LVCFMT_JUSTIFYMASK == LVCFMT_RIGHT {
            DT_RIGHT
        } else {
            0
        };
        gdi::draw_text(
            hdc,
            &text,
            &mut rc,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | align,
        );
    }
}

/// Where a list view puts a row's text, in pixels at 96 DPI: from the left
/// of column 0's label, and of the other columns' cells.
const LABEL_INSET: i32 = 2;
const CELL_INSET: i32 = 6;

thread_local! {
    /// The dark face and text box brushes, made once.
    static BRUSHES: Cell<Option<(HBRUSH, HBRUSH)>> = const { Cell::new(None) };
}

/// The answer to a dialog's `WM_CTLCOLOR*` message `msg` for the DC `hdc`:
/// in the dark theme its colours (the face of goz-gui's chrome, text boxes
/// like the search box); in the light theme `None`, for the system's.
pub fn ctl_color(msg: u32, hdc: HDC, dark: bool) -> Option<isize> {
    if !dark {
        return None;
    }
    let pal = Palette::new(true);
    let (face, field) = BRUSHES.get().unwrap_or_else(|| {
        // Two brushes kept for the life of the thread.
        let made = (
            gdi::create_solid_brush(pal.chrome),
            gdi::create_solid_brush(pal.bg),
        );
        BRUSHES.set(Some(made));
        made
    });
    let (bg, brush) = if msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX {
        (pal.bg, field)
    } else {
        (pal.chrome, face)
    };
    gdi::set_text_color(hdc, pal.fg);
    gdi::set_bk_color(hdc, bg);
    Some(brush as isize)
}

/// What a check box or radio button is, for drawing it here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    Box,
    Radio,
}

/// Whether `button` is a check box or a radio button (`None` for any other
/// button).
pub fn check_kind(button: HWND) -> Option<Check> {
    let kind = win::style(button) & BS_TYPEMASK as u32;
    match kind as i32 {
        BS_CHECKBOX | BS_AUTOCHECKBOX | BS_3STATE | BS_AUTO3STATE => Some(Check::Box),
        BS_RADIOBUTTON | BS_AUTORADIOBUTTON => Some(Check::Radio),
        _ => None,
    }
}

/// Whether Options > UI's Scale draws `hwnd` at another DPI than its
/// monitor's: then Windows draws a check box's or radio button's mark at the
/// monitor's size, small beside its scaled text, and goz-gui draws it.
pub fn ui_scaled(hwnd: HWND) -> bool {
    let monitor = sys::dpi_for_window(hwnd).max(96);
    crate::custom::window_dpi(hwnd) != monitor
}

/// The colours [`paint_check`] draws with: the face behind (`None`: the
/// parent's, as a see-through button shows it), the text, and the text
/// greyed.
#[derive(Clone, Copy)]
pub struct CheckInk {
    pub face: Option<u32>,
    pub fg: u32,
    pub dim: u32,
}

/// A dialog's check box or radio button's `NM_CUSTOMDRAW` (which its
/// dialog gets for every paint of it, state changes included), drawn here
/// ([`paint_check`]) when Windows would not draw it right: a radio button in
/// the dark theme (the dark style draws its text grey, as if it were
/// greyed), and either at a UI scale (Windows draws the mark at the
/// monitor's size). `None` for any other notification or control, or when
/// Windows draws it.
fn button_custom_draw(n: &Notify, dark: bool) -> Option<isize> {
    if n.code() != NM_CUSTOMDRAW || !n.sender_is("Button") {
        return None;
    }
    let button = n.from();
    let kind = check_kind(button)?;
    if !(ui_scaled(button) || dark && kind == Check::Radio) {
        return None;
    }
    let cd = n.get::<NMCUSTOMDRAW>()?;
    if cd.dwDrawStage != CDDS_PREPAINT {
        return Some(CDRF_DODEFAULT as isize);
    }
    let ink = if dark {
        let pal = Palette::new(true);
        CheckInk {
            face: Some(pal.chrome),
            fg: pal.fg,
            dim: pal.dim,
        }
    } else {
        CheckInk {
            face: None,
            fg: gdi::sys_color(COLOR_WINDOWTEXT),
            dim: gdi::sys_color(COLOR_GRAYTEXT),
        }
    };
    paint_check(button, &cd, kind, ink);
    Some(CDRF_SKIPDEFAULT as isize)
}

/// The visual style's state of a check box's or radio button's mark (the
/// `CBS_*` and `RBS_*` numbers are alike): `check` is its `BM_GETCHECK`.
pub fn check_state(check: u32, disabled: bool, pressed: bool, hot: bool) -> i32 {
    let base = match check {
        BST_CHECKED => 4,
        BST_INDETERMINATE => 8,
        _ => 0,
    };
    let state = if disabled {
        4
    } else if pressed {
        3
    } else if hot {
        2
    } else {
        1
    };
    base + state
}

/// The channel of a dark trackbar: its edge and its face, as the light
/// style's (#d6d6d6 round #e7eaea on the #f0f0f0 face) turned for the dark
/// face.
const DARK_CHANNEL: (u32, u32) = (theme::rgb(0x5f, 0x5f, 0x5f), theme::rgb(0x3a, 0x3a, 0x3a));

/// A trackbar's `NM_CUSTOMDRAW` (Options > UI's Scale) in the dark theme:
/// the visual style draws its channel light grey on any face, so the
/// channel is drawn here in the dark theme's colours; the thumb and the
/// ticks stay the style's. `None` for anything else or the light theme.
fn trackbar_custom_draw(n: &Notify, dark: bool) -> Option<isize> {
    if !dark || n.code() != NM_CUSTOMDRAW || !n.sender_is(win::trackbar::CLASS) {
        return None;
    }
    let cd = n.get::<NMCUSTOMDRAW>()?;
    Some(match cd.dwDrawStage {
        CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as isize,
        CDDS_ITEMPREPAINT if cd.dwItemSpec == TBCD_CHANNEL as usize => {
            let (edge, face) = DARK_CHANNEL;
            fill(cd.hdc, &cd.rc, edge);
            let inside = RECT {
                left: cd.rc.left + 1,
                top: cd.rc.top + 1,
                right: cd.rc.right - 1,
                bottom: cd.rc.bottom - 1,
            };
            fill(cd.hdc, &inside, face);
            CDRF_SKIPDEFAULT as isize
        }
        _ => CDRF_DODEFAULT as isize,
    })
}

/// Draws a check box or radio button for its `NM_CUSTOMDRAW` (`cd`, its
/// pre-paint): the visual style's own mark for its state at the window's
/// DPI with the UI scale (the dark style's in the dark theme), the text in
/// `ink` (dim when greyed), the access key's underline and the focus
/// rectangle as the keyboard cues say.
pub fn paint_check(button: HWND, cd: &NMCUSTOMDRAW, kind: Check, ink: CheckInk) {
    let hdc = cd.hdc;
    let rc = cd.rc;
    let state = cd.uItemState;
    let disabled = state & CDIS_DISABLED != 0;
    let part_state = check_state(
        button::check(button),
        disabled,
        state & CDIS_SELECTED != 0,
        state & CDIS_HOT != 0,
    );
    let part = match kind {
        Check::Box => BP_CHECKBOX,
        Check::Radio => BP_RADIOBUTTON,
    };
    match ink.face {
        Some(face) => fill(hdc, &rc, face),
        None => uxtheme::draw_parent_background(button, hdc, &rc),
    }
    let dpi = crate::custom::window_dpi(button);
    let px = |v: i32| v * dpi as i32 / 96;
    let theme = Theme::open(button, "Button", dpi);
    let size = theme
        .as_ref()
        .and_then(|t| t.part_size(hdc, part, part_state, TS_DRAW))
        .unwrap_or(windows_sys::Win32::Foundation::SIZE {
            cx: px(13),
            cy: px(13),
        });
    let top = rc.top + (rc.bottom - rc.top - size.cy) / 2;
    let glyph = RECT {
        left: rc.left,
        top,
        right: rc.left + size.cx,
        bottom: top + size.cy,
    };
    if let Some(theme) = &theme {
        theme.draw_background(hdc, part, part_state, &glyph, None);
    }
    drop(theme);
    let _font = gdi::Selected::new(hdc, win::font(button));
    let text = win::text_wide(button);
    let cues = win::send_plain(button, WM_QUERYUISTATE, 0, 0) as u32;
    let mut format = DT_SINGLELINE | DT_VCENTER;
    if cues & UISF_HIDEACCEL != 0 {
        format |= DT_HIDEPREFIX;
    }
    gdi::set_bk_mode(hdc, TRANSPARENT as i32);
    gdi::set_text_color(hdc, if disabled { ink.dim } else { ink.fg });
    let mut tr = RECT {
        left: glyph.right + px(3),
        ..rc
    };
    gdi::draw_text(hdc, &text, &mut tr, format);
    if state & CDIS_FOCUS != 0 && cues & UISF_HIDEFOCUS == 0 {
        // Round the text, as Windows draws it.
        let mut measured = tr;
        gdi::draw_text(hdc, &text, &mut measured, format | DT_CALCRECT);
        let focus = RECT {
            left: measured.left - 1,
            top: tr.top + (tr.bottom - tr.top - (measured.bottom - measured.top)) / 2 - 1,
            right: measured.right + 1,
            bottom: tr.top + (tr.bottom - tr.top + (measured.bottom - measured.top)) / 2 + 1,
        };
        gdi::draw_focus_rect(hdc, &focus);
    }
}

/// What every themed dialog handles alike: its colours (`WM_CTLCOLOR*`),
/// check boxes and radio buttons Windows would not draw right (`WM_NOTIFY`,
/// see `button_custom_draw`),
/// following the system's theme (`WM_SETTINGCHANGE`) when `look` says so,
/// and the lists [`fill_later`] put off, once it shows (`WM_SHOWWINDOW`).
/// `Some` is the dialog procedure's answer. Its start and its showing are
/// marks of `GOZ_GUI_TRACE`'s timeline (`crate::trace`).
pub fn common(m: &Msg, look: &Cell<Look>) -> Option<isize> {
    let dlg = m.hwnd;
    match m.id {
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN
        | WM_CTLCOLORLISTBOX => ctl_color(m.id, m.w as HDC, look.get().dark),
        WM_NOTIFY => {
            let dark = look.get().dark;
            let n = m.notify()?;
            let answer = button_custom_draw(&n, dark).or_else(|| trackbar_custom_draw(&n, dark))?;
            dlg::set_result(dlg, answer);
            Some(1)
        }
        WM_SETTINGCHANGE => {
            if let Some(new) = look.get().after_setting_change(m) {
                look.set(new);
                apply_theme(dlg, new.dark);
            }
            Some(0)
        }
        // Shown: the lists still to come follow (left to the dialog too).
        WM_SHOWWINDOW if m.w != 0 => {
            crate::trace::mark("dialog shown");
            lists_after_show(dlg);
            None
        }
        WM_INITDIALOG => {
            crate::trace::mark("dialog init");
            None
        }
        WM_DRAWITEM => {
            let dis = m.draw_item()?;
            (dis.CtlType == ODT_LISTBOX && win::class_is(dis.hwndItem, "ListBox")).then(|| {
                paint_list_item(&dis, look.get().dark);
                1
            })
        }
        _ => None,
    }
}

/// The style a dialog's list box takes (with `WS_TABSTOP`, `LBS_NOTIFY`
/// and the like): its items drawn by the dialog ([`paint_list_item`],
/// through [`common`]), their texts kept by the list.
pub const LIST_BOX: u32 = (LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32;

/// An owner-drawn list box's rows as high as its font's, as a list box
/// that draws its own (it is told its rows' height before it has a font).
fn fit_list_items(list: HWND) {
    if win::style(list) & LBS_OWNERDRAWFIXED as u32 == 0 {
        return;
    }
    let tm = {
        let dc = gdi::WindowDc::get(list);
        let _font = gdi::Selected::new(dc.hdc(), win::font(list));
        gdi::text_metrics(dc.hdc())
    };
    if tm.tmHeight > 0 {
        listbox::set_item_height(list, 0, tm.tmHeight);
    }
}

/// The colours of a list box's item: selected, Windows' selection; else
/// the theme's field (greyed text when the list is).
fn list_item_colours(dark: bool, selected: bool, enabled: bool) -> (u32, u32) {
    let pal = Palette::new(dark);
    let sys = gdi::sys_color;
    match (selected, dark, enabled) {
        (true, ..) => (sys(COLOR_HIGHLIGHT), sys(COLOR_HIGHLIGHTTEXT)),
        (false, true, true) => (pal.bg, pal.fg),
        (false, true, false) => (pal.bg, pal.dim),
        (false, false, true) => (sys(COLOR_WINDOW), sys(COLOR_WINDOWTEXT)),
        (false, false, false) => (sys(COLOR_WINDOW), sys(COLOR_GRAYTEXT)),
    }
}

/// Draws an item of a dialog's list box ([`LIST_BOX`]) as Windows draws a
/// plain one: its text a little in from the left, Windows' selection
/// colours, the focus rectangle when the list has the keyboard.
fn paint_list_item(dis: &DRAWITEMSTRUCT, dark: bool) {
    let hdc = dis.hDC;
    let rc = dis.rcItem;
    let focus = dis.itemState & ODS_FOCUS != 0 && dis.itemState & ODS_NOFOCUSRECT == 0;
    // No items: only the focus rectangle, where the first would be.
    if dis.itemID == u32::MAX {
        if focus {
            gdi::draw_focus_rect(hdc, &rc);
        }
        return;
    }
    let selected = dis.itemState & ODS_SELECTED != 0;
    let enabled = win::is_enabled(dis.hwndItem);
    let (bg, fg) = list_item_colours(dark, selected, enabled);
    fill(hdc, &rc, bg);
    let text: Vec<u16> = listbox::text(dis.hwndItem, dis.itemID as i32)
        .encode_utf16()
        .collect();
    if !text.is_empty() {
        gdi::set_bk_mode(hdc, TRANSPARENT as i32);
        gdi::set_text_color(hdc, fg);
        let mut tr = RECT {
            left: rc.left + LIST_TEXT_INSET,
            ..rc
        };
        gdi::draw_text(
            hdc,
            &text,
            &mut tr,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
    }
    if focus {
        gdi::draw_focus_rect(hdc, &rc);
    }
}

/// Where a list box draws an item's text: this many pixels in from its
/// left.
const LIST_TEXT_INSET: i32 = 2;

/// Whether the foreground window is goz-gui's. None at all (the foreground
/// changing hands, the user's window just closed) is not: then the system
/// may hand goz-gui the foreground, which only the user should.
fn foreground_is_ours() -> bool {
    let fg = win::foreground();
    !fg.is_null() && win::process_of(fg) == sys::process_id()
}

/// Keeps goz-gui from coming to the front on its own: while a `Quiet`
/// lives, and only when goz-gui was not the foreground app as it began
/// (a dialog opened by a command posted to the window, not by the user),
/// the windows this thread shows (the dialog, a message box or file dialog
/// over it) are not activated. A click on one of them activates it as
/// usual (Alt+Tab or the taskbar do not, until the window is clicked). A
/// thread hook: nothing outside goz-gui's own thread is touched.
///
/// Two parts: each top-level window made meanwhile is shown without being
/// activated (its first show gets `SWP_NOACTIVATE`), because showing a
/// window can hand the process the foreground before any hook is asked
/// (the foreground lock lets a process take it once the user has been
/// idle); and any activation neither a click nor the user asked for is
/// refused.
pub struct Quiet {
    _hook: Hook,
}

thread_local! {
    /// How many `Quiet`s live on this thread.
    static QUIET: Cell<u32> = const { Cell::new(0) };
}

impl Quiet {
    /// `None` when goz-gui is the foreground app (the user opened the
    /// dialog): nothing to hold back.
    pub fn begin() -> Option<Quiet> {
        if foreground_is_ours() {
            return None;
        }
        let hook = hook::set(Kind::Cbt, quiet_hook)?;
        QUIET.set(QUIET.get() + 1);
        Some(Quiet { _hook: hook })
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.set(QUIET.get().saturating_sub(1));
    }
}

/// Shows a new top-level window without activating it, while a `Quiet`
/// lives and goz-gui is not the foreground app; refuses an activation that
/// neither a click nor the user's switching to goz-gui asked for.
fn quiet_hook(event: &Event) -> Option<isize> {
    match *event {
        Event::Create { hwnd, style, .. } if style & WS_CHILD == 0 => {
            subclass::set(hwnd, &QUIET_SHOW, quiet_show_proc);
            None
        }
        // No foreground window at all is no permission either: it is how
        // the system hands goz-gui the foreground (a window of another
        // program closed, and goz-gui's is next), which only a click on
        // goz-gui's window should do.
        Event::Activate { by_mouse, .. } if !by_mouse && !foreground_is_ours() => Some(1),
        _ => None,
    }
}

/// The subclass of a window made while a `Quiet` lived: while one still
/// lives and goz-gui is not the foreground app, showing it does not
/// activate it.
fn quiet_show_proc(m: &Msg) -> isize {
    if m.id == WM_WINDOWPOSCHANGING
        && let Some(mut pos) = m.window_pos()
        && pos.flags & SWP_SHOWWINDOW != 0
        && QUIET.get() > 0
        && !foreground_is_ours()
    {
        pos.flags |= SWP_NOACTIVATE;
    }
    m.def()
}

/// How a control follows its dialog's size ([`anchor`]): moved right or
/// down, or grown wider or taller, by what the dialog grew.
pub const MOVE_X: u8 = 1;
pub const MOVE_Y: u8 = 2;
pub const GROW_X: u8 = 4;
pub const GROW_Y: u8 = 8;

/// A rectangle `(x, y, cx, cy)` (dialog units in a template).
pub type Units = (i16, i16, i16, i16);

/// `r` moved or grown by `(dx, dy)` as `how` says.
fn anchored(r: RECT, (dx, dy): (i32, i32), how: u8) -> RECT {
    let by = |bit: u8, d: i32| if how & bit != 0 { d } else { 0 };
    let (x, y) = (by(MOVE_X, dx), by(MOVE_Y, dy));
    RECT {
        left: r.left + x,
        top: r.top + y,
        right: r.right + x + by(GROW_X, dx),
        bottom: r.bottom + y + by(GROW_Y, dy),
    }
}

/// A rectangle in dialog units as a RECT, for [`dlg::map_rect`].
fn units_rect((x, y, cx, cy): Units) -> RECT {
    RECT {
        left: x.into(),
        top: y.into(),
        right: i32::from(x) + i32::from(cx),
        bottom: i32::from(y) + i32::from(cy),
    }
}

/// A control of window class `class` made in dialog `dlg` after the
/// template's (a costly one, made once the dialog has been drawn): visible,
/// `id`, `style` added, where `units` (dialog units) say, in the dialog's
/// font, and next after `after` in the tab order (its label, so that the
/// label's access key reaches it). Null if it could not be made.
pub fn make_control(
    dlg: HWND,
    class: &str,
    id: u16,
    style: u32,
    units: Units,
    after: HWND,
) -> HWND {
    let r = dlg::map_rect(dlg, units_rect(units));
    let made = create_plain(&Create {
        ex_style: 0,
        class,
        title: "",
        style: WS_CHILD | WS_VISIBLE | style,
        rect: (r.left, r.top, r.right - r.left, r.bottom - r.top),
        parent: dlg,
        menu: id_menu(usize::from(id)),
    });
    if !made.is_null() {
        win::set_font(made, win::font(dlg), false);
        win::set_pos(
            made,
            after,
            (0, 0, 0, 0),
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    made
}

/// Places `controls` for the dialog's size: each at its template
/// rectangle, moved or grown (`MOVE_X`, `GROW_Y`, ...) by what the client
/// area has over the template's `size`. In dialog units, so it holds at
/// every DPI. A combo box's height is its list's, as in the template.
pub fn anchor(dlg: HWND, size: (i16, i16), controls: &[(u16, Units, u8)]) {
    for &(id, units, how) in controls {
        let r = placed(dlg, size, units, how);
        let h = dlg::item(dlg, id);
        if !h.is_null() {
            win::move_to(h, &r, true);
        }
    }
}

/// Where [`anchor`] puts a control of template rectangle `units` that
/// follows the dialog's size as `how` says, in pixels of the dialog's
/// client area.
pub fn placed(dlg: HWND, size: (i16, i16), units: Units, how: u8) -> RECT {
    let px = |u: Units| dlg::map_rect(dlg, units_rect(u));
    let client = win::client_rect(dlg);
    let base = px((0, 0, size.0, size.1));
    let grew = (
        (client.right - base.right).max(0),
        (client.bottom - base.bottom).max(0),
    );
    anchored(px(units), grew, how)
}

/// Centres a dialog on its owner, kept inside the work area of the owner's
/// monitor. An owner on no monitor at all (moved off the screens) leaves it
/// centred where the owner is.
pub fn center_on_owner(dlg: HWND) {
    let owner = win::related(dlg, GW_OWNER);
    if owner.is_null() {
        return;
    }
    let o = win::window_rect(owner);
    let d = win::window_rect(dlg);
    let (w, h) = (d.right - d.left, d.bottom - d.top);
    let mut x = o.left + (o.right - o.left - w) / 2;
    let mut y = o.top + (o.bottom - o.top - h) / 2;
    let monitor = sys::monitor_from_rect(&o, MONITOR_DEFAULTTONULL);
    if !monitor.is_null()
        && let Some(mi) = sys::monitor_info(monitor)
    {
        let work = mi.rcWork;
        x = x.min(work.right - w).max(work.left);
        y = y.min(work.bottom - h).max(work.top);
    }
    win::set_pos(
        dlg,
        core::ptr::null_mut(),
        (x, y, 0, 0),
        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CB_ADDSTRING, CB_DELETESTRING, CB_FINDSTRINGEXACT, CB_GETCOUNT, CB_GETCURSEL, CB_GETLBTEXT,
        CB_SETCURSEL, CB_SHOWDROPDOWN, WM_GETTEXT, WM_MOVE, WM_SETFONT, WM_SIZE,
    };

    #[test]
    fn a_list_comes_in_before_it_is_used_or_read() {
        // Read, changed or dropped down by anyone: the list first.
        for msg in [
            CB_GETCURSEL,
            CB_SETCURSEL,
            CB_GETCOUNT,
            CB_GETLBTEXT,
            CB_FINDSTRINGEXACT,
            CB_SHOWDROPDOWN,
            CB_ADDSTRING,
            CB_DELETESTRING,
        ] {
            assert!(wants_list(msg), "{msg:#x}");
        }
        // Used: a click, a key, the wheel.
        for msg in [
            WM_LBUTTONDOWN,
            WM_KEYDOWN,
            WM_SYSKEYDOWN,
            WM_CHAR,
            WM_MOUSEWHEEL,
        ] {
            assert!(wants_list(msg), "{msg:#x}");
        }
        // Drawn, placed or its text (the chosen item, already in) read: not
        // yet.
        for msg in [WM_PAINT, WM_SETFONT, WM_SIZE, WM_MOVE, WM_TIMER, WM_GETTEXT] {
            assert!(!wants_list(msg), "{msg:#x}");
        }
    }

    #[test]
    fn check_marks_take_the_styles_states() {
        use windows_sys::Win32::UI::Controls::BST_UNCHECKED;
        use windows_sys::Win32::UI::Controls::{
            CBS_CHECKEDDISABLED, CBS_CHECKEDNORMAL, CBS_MIXEDHOT, CBS_UNCHECKEDHOT,
            CBS_UNCHECKEDNORMAL, CBS_UNCHECKEDPRESSED, RBS_CHECKEDPRESSED, RBS_UNCHECKEDDISABLED,
        };
        let (off, on, mixed) = (BST_UNCHECKED, BST_CHECKED, BST_INDETERMINATE);
        assert_eq!(check_state(off, false, false, false), CBS_UNCHECKEDNORMAL);
        assert_eq!(check_state(off, false, false, true), CBS_UNCHECKEDHOT);
        assert_eq!(check_state(off, false, true, true), CBS_UNCHECKEDPRESSED);
        assert_eq!(check_state(on, false, false, false), CBS_CHECKEDNORMAL);
        // Greyed wins over hot and pressed.
        assert_eq!(check_state(on, true, true, true), CBS_CHECKEDDISABLED);
        assert_eq!(check_state(mixed, false, false, true), CBS_MIXEDHOT);
        // A radio button's states are numbered alike.
        assert_eq!(check_state(on, false, true, false), RBS_CHECKEDPRESSED);
        assert_eq!(check_state(off, true, false, false), RBS_UNCHECKEDDISABLED);
    }

    #[test]
    fn dark_edges_are_a_one_line_boxs_or_a_list_views() {
        let grey = |v: u8| theme::rgb(v, v, v);
        // A box: the dark one-line box's edge, dimmer greyed.
        assert_eq!(dark_edge_colours(EDGE_BOX, true), (grey(0x9b), grey(0x38)));
        assert_eq!(dark_edge_colours(EDGE_BOX, false), (grey(0x40), grey(0x38)));
        // A list: a list view's edge round the list's face.
        let pal = Palette::new(true);
        assert_eq!(
            dark_edge_colours(EDGE_LIST, true),
            (theme::rgb(0x82, 0x87, 0x90), pal.bg)
        );
        assert_eq!(
            dark_edge_colours(EDGE_LIST, false),
            (theme::rgb(0x82, 0x87, 0x90), pal.chrome)
        );
    }

    #[test]
    fn the_last_column_takes_what_the_others_leave() {
        // Organize Filters' list, 601 px inside; 584 with the vertical bar.
        assert_eq!(last_column_width(601, 96, 50), 505);
        assert_eq!(last_column_width(584, 96, 50), 488);
        // Advanced Rename's: Name and New Name before Path.
        assert_eq!(last_column_width(480, 170 + 200, 40), 110);
        // The others made wider than the list: the header's text still shows.
        assert_eq!(last_column_width(601, 590, 50), 50);
        assert_eq!(last_column_width(601, 900, 50), 50);
    }

    #[test]
    fn anchored_controls_follow_the_dialog() {
        let r = RECT {
            left: 10,
            top: 20,
            right: 110,
            bottom: 40,
        };
        let at = |l, t, rr, b| RECT {
            left: l,
            top: t,
            right: rr,
            bottom: b,
        };
        let eq = |a: RECT, b: RECT| {
            (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
        };
        assert!(eq(anchored(r, (30, 50), 0), r));
        assert!(eq(anchored(r, (30, 50), GROW_X), at(10, 20, 140, 40)));
        assert!(eq(
            anchored(r, (30, 50), GROW_X | GROW_Y),
            at(10, 20, 140, 90)
        ));
        assert!(eq(
            anchored(r, (30, 50), MOVE_Y | GROW_X),
            at(10, 70, 140, 90)
        ));
        assert!(eq(
            anchored(r, (30, 50), MOVE_X | MOVE_Y),
            at(40, 70, 140, 90)
        ));
    }
}
