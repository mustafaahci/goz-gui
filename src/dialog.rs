//! Modal dialogs made in memory, with no resource script: a DLGTEMPLATE and
//! its controls laid out in dialog units. The dialog manager then brings the
//! keyboard (Tab, Enter, Esc, access keys), the message font, and with
//! per-monitor DPI awareness the scaling for each monitor. Also the colours
//! that make a dialog follow goz-gui's theme.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, DrawTextW, EndPaint, GetMonitorInfoW, GetTextExtentPoint32W, GetWindowDC, HBRUSH,
    HDC, HFONT, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect, PAINTSTRUCT, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC, SelectObject, SetBkColor,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, HDM_GETITEMCOUNT, HIMAGELIST, HKM_GETHOTKEY,
    HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_EXT, HOTKEYF_SHIFT, ILD_TRANSPARENT, ImageList_Draw,
    LVCF_FMT, LVCFMT_JUSTIFYMASK, LVCFMT_RIGHT, LVCOLUMNW, LVIF_IMAGE, LVIR_BOUNDS, LVIR_ICON,
    LVIR_LABEL, LVIS_SELECTED, LVITEMW, LVM_GETCOLUMNW, LVM_GETHEADER, LVM_GETIMAGELIST,
    LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETITEMTEXTW, LVM_GETITEMW,
    LVM_GETSUBITEMRECT, LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVSIL_SMALL,
    NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, NMLVCUSTOMDRAW, TVM_SETBKCOLOR, TVM_SETLINECOLOR,
    TVM_SETTEXTCOLOR,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::HiDpi::SystemParametersInfoForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyNameTextW, IsWindowEnabled, MAPVK_VK_TO_VSC, MapVirtualKeyW, VK_CONTROL,
    VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_GROUPBOX, BS_TYPEMASK, CBT_CREATEWNDW, CBTACTIVATESTRUCT, CallNextHookEx,
    CreateDialogIndirectParamW, DLGPROC, DS_CONTROL, DS_MODALFRAME, DS_SETFONT,
    DialogBoxIndirectParamW, ES_MULTILINE, GW_CHILD, GW_HWNDNEXT, GW_OWNER, GWL_STYLE,
    GWLP_USERDATA, GetClassNameW, GetClientRect, GetDlgItem, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HCBT_ACTIVATE, HCBT_CREATEWND,
    HHOOK, MapDialogRect, MoveWindow, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SendMessageW,
    SetCaretPos, SetWindowPos, SetWindowsHookExW, UnhookWindowsHookEx, WH_CBT, WINDOWPOS,
    WM_CTLCOLORBTN, WM_CTLCOLORDLG, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC,
    WM_ERASEBKGND, WM_GETFONT, WM_NCDESTROY, WM_NCPAINT, WM_NOTIFY, WM_PAINT, WM_SETTINGCHANGE,
    WM_WINDOWPOSCHANGING, WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CONTROLPARENT, WS_POPUP,
    WS_SYSMENU, WS_VISIBLE,
};

use crate::paint::fill;
use crate::theme::{self, Palette};
use crate::wide::is_wide;

/// A control's window class.
pub enum Class {
    Button,
    Edit,
    Static,
    /// A class by name, such as a common control's.
    Named(&'static str),
}

/// A dialog's template, built control by control.
pub struct Template {
    /// The template as 16-bit words, the unit its fields come in.
    words: Vec<u16>,
}

/// Where the control count sits in the template: after the style and the
/// extended style.
const COUNT_AT: usize = 4;

/// The message font's face and size in points, which dialogs use in
/// Windows' own programs.
fn message_font() -> (u16, Vec<u16>) {
    // SAFETY: a plain out-struct whose size is passed.
    let ncm = unsafe {
        let mut ncm: NONCLIENTMETRICSW = core::mem::zeroed();
        ncm.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
        let ok = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            (&raw mut ncm).cast(),
            0,
            96,
        );
        (ok != 0).then_some(ncm)
    };
    let Some(ncm) = ncm else {
        return (9, "Segoe UI".encode_utf16().collect());
    };
    let lf = ncm.lfMessageFont;
    let face: Vec<u16> = lf
        .lfFaceName
        .iter()
        .copied()
        .take_while(|&c| c != 0)
        .collect();
    // At 96 DPI a point is 4/3 pixels.
    let points = (lf.lfHeight.unsigned_abs() * 72).div_ceil(96).max(1);
    (points as u16, face)
}

impl Template {
    /// A modal dialog of `cx` × `cy` dialog units in the message font, with
    /// a title bar and a close box; `style` adds to that (`WS_THICKFRAME`).
    pub fn new(title: &str, style: u32, cx: i16, cy: i16) -> Template {
        let style =
            style | WS_POPUP | WS_CAPTION | WS_SYSMENU | (DS_MODALFRAME | DS_SETFONT) as u32;
        Template::with_styles(title, style, 0, cx, cy)
    }

    /// A page of another dialog, `cx` × `cy` dialog units in the message
    /// font: a child dialog (`DS_CONTROL`, `WS_EX_CONTROLPARENT`) whose
    /// controls the parent's Tab, arrow and access keys reach as its own.
    /// Made hidden with [`Template::create_child`].
    pub fn child(cx: i16, cy: i16) -> Template {
        let style = WS_CHILD | WS_CLIPSIBLINGS | (DS_CONTROL | DS_SETFONT) as u32;
        Template::with_styles("", style, WS_EX_CONTROLPARENT, cx, cy)
    }

    fn with_styles(title: &str, style: u32, ex_style: u32, cx: i16, cy: i16) -> Template {
        let mut t = Template { words: Vec::new() };
        t.dword(style);
        t.dword(ex_style);
        // The control count, then the position and size.
        for v in [0, 0, 0, cx, cy] {
            t.words.push(v as u16);
        }
        // No menu, the standard dialog class.
        t.words.extend([0, 0]);
        t.string(title);
        let (points, face) = message_font();
        t.words.push(points);
        t.words.extend(face);
        t.words.push(0);
        t
    }

    fn dword(&mut self, v: u32) {
        self.words.extend([v as u16, (v >> 16) as u16]);
    }

    fn string(&mut self, s: &str) {
        self.words.extend(s.encode_utf16());
        self.words.push(0);
    }

    /// Adds a visible control at `x, y` of `cx` × `cy` dialog units. Controls
    /// come in tab order.
    pub fn control(
        &mut self,
        class: Class,
        text: &str,
        id: u16,
        style: u32,
        ex_style: u32,
        (x, y, cx, cy): (i16, i16, i16, i16),
    ) -> &mut Template {
        // Each control starts on a 4-byte boundary.
        if self.words.len() % 2 == 1 {
            self.words.push(0);
        }
        self.dword(WS_CHILD | WS_VISIBLE | style);
        self.dword(ex_style);
        for v in [x, y, cx, cy] {
            self.words.push(v as u16);
        }
        self.words.push(id);
        match class {
            // The predefined classes go by their atoms.
            Class::Button => self.words.extend([0xffff, 0x0080]),
            Class::Edit => self.words.extend([0xffff, 0x0081]),
            Class::Static => self.words.extend([0xffff, 0x0082]),
            Class::Named(name) => self.string(name),
        }
        self.string(text);
        // No creation data.
        self.words.push(0);
        self.words[COUNT_AT] += 1;
        self
    }

    /// The template as the dialog functions take it: on a 4-byte boundary,
    /// which u32s are.
    fn aligned(&self) -> Vec<u32> {
        let mut aligned = vec![0u32; self.words.len().div_ceil(2)];
        for (i, &w) in self.words.iter().enumerate() {
            aligned[i / 2] |= u32::from(w) << (16 * (i % 2));
        }
        aligned
    }

    /// Shows the dialog, owned by `owner`, until it ends; returns what
    /// `EndDialog` was given (-1 if it could not be made). `param` reaches
    /// `proc` as WM_INITDIALOG's lParam.
    pub fn run(&self, owner: HWND, proc: DLGPROC, param: LPARAM) -> isize {
        let aligned = self.aligned();
        // SAFETY: a complete template that outlives the modal call, a dialog
        // procedure, and a window of this thread as the owner.
        unsafe {
            DialogBoxIndirectParamW(
                GetModuleHandleW(core::ptr::null()),
                aligned.as_ptr().cast(),
                owner,
                proc,
                param,
            )
        }
    }

    /// Makes a [`Template::child`] page in `parent` (hidden until shown);
    /// null if it could not be made. `param` reaches `proc` as
    /// WM_INITDIALOG's lParam.
    pub fn create_child(&self, parent: HWND, proc: DLGPROC, param: LPARAM) -> HWND {
        let aligned = self.aligned();
        // SAFETY: a complete template (the system copies what it needs
        // before returning), a dialog procedure, and a window of this
        // thread as the parent.
        unsafe {
            CreateDialogIndirectParamW(
                GetModuleHandleW(core::ptr::null()),
                aligned.as_ptr().cast(),
                parent,
                proc,
                param,
            )
        }
    }
}

/// How a dialog looks: dark or light, and whether it follows the system's
/// app theme (View > Theme > User Default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    pub follow_system: bool,
}

impl Look {
    /// The look after a `WM_SETTINGCHANGE` whose lParam is `l`, if it
    /// changes: only when it follows the system and the app theme changed.
    pub fn after_setting_change(self, l: LPARAM) -> Option<Look> {
        // SAFETY: WM_SETTINGCHANGE's lParam is null or a NUL-terminated
        // string naming the setting.
        let colours = l != 0 && unsafe { is_wide(l as *const u16, "ImmersiveColorSet") };
        if !self.follow_system || !colours {
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

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Makes a dialog and its controls dark or light: the title bar, buttons,
/// text boxes and lists. Its face and text follow from [`ctl_color`].
pub fn apply_theme(dlg: HWND, dark: bool) {
    theme::set_titlebar(dlg, dark);
    let pal = Palette::new(dark);
    // SAFETY: walks and sends to the children of this thread's dialog.
    unsafe {
        let mut child = GetWindow(dlg, GW_CHILD);
        while !child.is_null() {
            theme::allow(child, dark);
            match class_of(child).as_str() {
                // A themed group box draws its caption in the theme's dark
                // text whatever WM_CTLCOLORSTATIC says, and an unthemed one
                // a white etched frame: in the dark theme it is drawn here.
                "Button"
                    if GetWindowLongPtrW(child, GWL_STYLE) as u32 & BS_TYPEMASK as u32
                        == BS_GROUPBOX as u32 =>
                {
                    theme::set_class(child, None);
                    SetWindowSubclass(child, Some(group_proc), 1, usize::from(dark));
                }
                "Button" => theme::set_class(child, dark.then_some("DarkMode_Explorer")),
                // A hotkey box draws white whatever its parent says: in the
                // dark theme it is drawn here, its edge too.
                "msctls_hotkey32" => {
                    SetWindowSubclass(child, Some(hotkey_proc), 1, usize::from(dark));
                    SetWindowPos(
                        child,
                        core::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
                // A box of several lines has a scroll bar, which the
                // Explorer style makes dark (its edge is drawn by its
                // owner).
                "Edit" if GetWindowLongPtrW(child, GWL_STYLE) as u32 & ES_MULTILINE as u32 != 0 => {
                    theme::set_class(child, dark.then_some("DarkMode_Explorer"));
                }
                "Edit" => theme::set_class(child, dark.then_some("DarkMode_CFD")),
                // Its list's colours come from WM_CTLCOLORLISTBOX.
                "ComboBox" => theme::set_class(child, dark.then_some("DarkMode_CFD")),
                "SysTreeView32" => {
                    theme::set_class(
                        child,
                        Some(if dark {
                            "DarkMode_Explorer"
                        } else {
                            "Explorer"
                        }),
                    );
                    SendMessageW(child, TVM_SETBKCOLOR, 0, pal.bg as isize);
                    SendMessageW(child, TVM_SETTEXTCOLOR, 0, pal.fg as isize);
                    SendMessageW(child, TVM_SETLINECOLOR, 0, pal.dim as isize);
                }
                "SysListView32" => {
                    // Dark scroll bars come with the dark Explorer style;
                    // it shows no selection once the list loses the
                    // keyboard, so dialogs draw that themselves (see
                    // `list_custom_draw`).
                    theme::set_class(
                        child,
                        Some(if dark {
                            "DarkMode_Explorer"
                        } else {
                            "Explorer"
                        }),
                    );
                    let header = SendMessageW(child, LVM_GETHEADER, 0, 0) as HWND;
                    theme::allow(header, dark);
                    theme::set_class(header, dark.then_some("DarkMode_ItemsView"));
                    SendMessageW(child, LVM_SETBKCOLOR, 0, pal.bg as isize);
                    SendMessageW(child, LVM_SETTEXTBKCOLOR, 0, pal.bg as isize);
                    SendMessageW(child, LVM_SETTEXTCOLOR, 0, pal.fg as isize);
                    // The subclass's data is whether the theme is dark;
                    // setting it again only updates that.
                    SetWindowSubclass(child, Some(list_proc), 1, usize::from(dark));
                }
                _ => {}
            }
            child = GetWindow(child, GW_HWNDNEXT);
        }
        RedrawWindow(
            dlg,
            core::ptr::null(),
            core::ptr::null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
        );
    }
}

/// A group box's subclass: in the dark theme (`dark` set) its frame is a
/// line of the theme's border colour and its caption the theme's text,
/// as Windows draws neither.
unsafe extern "system" fn group_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    dark: usize,
) -> LRESULT {
    // SAFETY: `l` is read as each message defines it; the subclass goes
    // with the group box.
    unsafe {
        match msg {
            // The box is see-through: what is inside it is its parent's.
            WM_ERASEBKGND if dark != 0 => return 1,
            WM_PAINT if dark != 0 => {
                paint_dark_group(hwnd);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(group_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

fn paint_dark_group(hwnd: HWND) {
    let pal = Palette::new(true);
    // SAFETY: a standard WM_PAINT on this thread's group box; the font
    // selected is put back.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc);
        let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        let old = SelectObject(hdc, font);
        let text: Vec<u16> = crate::wide::window_text(hwnd).encode_utf16().collect();
        let mut size = SIZE::default();
        GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size);
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
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
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, pal.fg);
            let mut tr = RECT {
                left: caption.left + px(2),
                ..caption
            };
            DrawTextW(
                hdc,
                text.as_ptr(),
                text.len() as i32,
                &mut tr,
                DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        SelectObject(hdc, old);
        EndPaint(hwnd, &ps);
    }
}

/// A hotkey box's subclass: in the dark theme (`dark` set) its face, its
/// edge and the key it holds ("Ctrl + Shift + F", or "None") are drawn
/// here in the theme's colours, the caret after the text.
unsafe extern "system" fn hotkey_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    dark: usize,
) -> LRESULT {
    // SAFETY: `l` is read as each message defines it; the subclass goes
    // with the box.
    unsafe {
        match msg {
            WM_ERASEBKGND if dark != 0 => return 1,
            WM_PAINT if dark != 0 => {
                paint_dark_hotkey(hwnd);
                return 0;
            }
            WM_NCPAINT if dark != 0 => {
                paint_dark_edge(hwnd);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(hotkey_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

/// The name of a key as a hotkey box shows it: `key` is its
/// `HKM_GETHOTKEY` (virtual key low, `HOTKEYF_*` high).
fn hotkey_text(key: u16) -> String {
    let (vk, mods) = (key & 0xff, (key >> 8) as u8);
    let name = |vk: u16, extended: bool| -> String {
        // SAFETY: maps a key to its scan code and reads its name into the
        // buffer, with its length.
        unsafe {
            let scan = MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC);
            let l = (scan << 16) | if extended { 1 << 24 } else { 0 };
            let mut buf = [0u16; 64];
            let n = GetKeyNameTextW(l as i32, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        }
    };
    let mut parts = Vec::new();
    for (flag, key) in [
        (HOTKEYF_CONTROL, VK_CONTROL),
        (HOTKEYF_SHIFT, VK_SHIFT),
        (HOTKEYF_ALT, VK_MENU),
    ] {
        if mods & flag as u8 != 0 {
            parts.push(name(key, false));
        }
    }
    if vk != 0 {
        parts.push(name(vk, mods & HOTKEYF_EXT as u8 != 0));
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
    // SAFETY: a standard WM_PAINT on this thread's box; the font selected
    // is put back.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc);
        let enabled = IsWindowEnabled(hwnd) != 0;
        fill(hdc, &rc, if enabled { pal.bg } else { pal.chrome });
        let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        let old = SelectObject(hdc, font);
        let key = SendMessageW(hwnd, HKM_GETHOTKEY, 0, 0) as u16;
        let text: Vec<u16> = hotkey_text(key).encode_utf16().collect();
        let mut size = SIZE::default();
        GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, if enabled { pal.fg } else { pal.dim });
        let x = rc.left + 1;
        let y = rc.top + (rc.bottom - rc.top - size.cy) / 2;
        let mut tr = RECT {
            left: x,
            top: y,
            right: rc.right,
            bottom: y + size.cy,
        };
        DrawTextW(
            hdc,
            text.as_ptr(),
            text.len() as i32,
            &mut tr,
            DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
        EndPaint(hwnd, &ps);
        if GetFocus() == hwnd {
            SetCaretPos(x + size.cx, y);
        }
    }
}

/// A one-line edge in the theme's border colour, the box's face between it
/// and the client area, for a control drawn here whose edge Windows draws
/// light.
fn paint_dark_edge(hwnd: HWND) {
    let pal = Palette::new(true);
    // SAFETY: draws in this thread's window's frame, through its window DC,
    // released after.
    unsafe {
        let (mut wr, mut client) = (RECT::default(), RECT::default());
        GetWindowRect(hwnd, &mut wr);
        GetClientRect(hwnd, &mut client);
        let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
        let hdc = GetWindowDC(hwnd);
        let all = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let face = if IsWindowEnabled(hwnd) != 0 {
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
        ReleaseDC(hwnd, hdc);
    }
}

/// A dialog list's subclass: in the dark theme it gives its header's item
/// text the theme's text colour, which the dark header style leaves dark.
/// The header reports its drawing to the list, its parent.
unsafe extern "system" fn list_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    dark: usize,
) -> LRESULT {
    // SAFETY: `l` is read as each message defines it; the subclass goes
    // with the list.
    unsafe {
        match msg {
            WM_NOTIFY if dark != 0 => {
                let hdr = &*(l as *const NMHDR);
                let header = SendMessageW(hwnd, LVM_GETHEADER, 0, 0) as HWND;
                if hdr.hwndFrom == header && hdr.code == NM_CUSTOMDRAW {
                    let cd = &*(l as *const NMCUSTOMDRAW);
                    match cd.dwDrawStage {
                        CDDS_PREPAINT => return CDRF_NOTIFYITEMDRAW as LRESULT,
                        CDDS_ITEMPREPAINT => {
                            SetTextColor(cd.hdc, Palette::new(true).fg);
                            return CDRF_DODEFAULT as LRESULT;
                        }
                        _ => {}
                    }
                }
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(list_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
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
            let row = cd.nmcd.dwItemSpec;
            // SAFETY: asks this thread's list about one of its rows.
            let state =
                unsafe { SendMessageW(list, LVM_GETITEMSTATE, row, LVIS_SELECTED as isize) };
            if state as u32 & LVIS_SELECTED == 0 {
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
    let (mut client, mut header) = (RECT::default(), RECT::default());
    // SAFETY: reads the sizes of this thread's list, its header and its
    // last row into out-structs.
    let top = unsafe {
        GetClientRect(list, &mut client);
        GetWindowRect(SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND, &mut header);
        let mut top = header.bottom - header.top;
        let count = SendMessageW(list, LVM_GETITEMCOUNT, 0, 0);
        if count > 0 {
            let mut last = RECT {
                left: LVIR_BOUNDS as i32,
                ..RECT::default()
            };
            SendMessageW(
                list,
                LVM_GETITEMRECT,
                (count - 1) as usize,
                (&raw mut last) as isize,
            );
            top = top.max(last.bottom);
        }
        top
    };
    if top < client.bottom {
        fill(hdc, &RECT { top, ..client }, bg);
    }
}

/// Paints a selected row of a dark dialog list: the row in the selection
/// colour, each column's text where the list puts it.
fn paint_selected_row(list: HWND, hdc: HDC, row: usize) {
    let pal = Palette::new(true);
    // SAFETY: reads and paints this thread's list on the DC it gave for
    // the row; every out-struct and buffer outlives its call.
    unsafe {
        let focused = GetFocus() == list;
        let mut bounds = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        SendMessageW(list, LVM_GETITEMRECT, row, (&raw mut bounds) as isize);
        fill(
            hdc,
            &bounds,
            if focused { pal.sel } else { pal.sel_inactive },
        );
        // The row's icon, when the list has them.
        let images = SendMessageW(list, LVM_GETIMAGELIST, LVSIL_SMALL as usize, 0) as HIMAGELIST;
        if images != 0 {
            let mut it = LVITEMW {
                mask: LVIF_IMAGE,
                iItem: row as i32,
                ..core::mem::zeroed()
            };
            SendMessageW(list, LVM_GETITEMW, 0, (&raw mut it) as isize);
            if it.iImage >= 0 {
                let mut icon = RECT {
                    left: LVIR_ICON as i32,
                    ..RECT::default()
                };
                SendMessageW(list, LVM_GETITEMRECT, row, (&raw mut icon) as isize);
                ImageList_Draw(images, it.iImage, hdc, icon.left, icon.top, ILD_TRANSPARENT);
            }
        }
        let header = SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND;
        let columns = SendMessageW(header, HDM_GETITEMCOUNT, 0, 0).max(0) as i32;
        let pad = |v: i32| v * GetDpiForWindow(list).max(96) as i32 / 96;
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, pal.fg);
        for col in 0..columns {
            // Column 0's label leaves out the margin before it, as the
            // list draws its text.
            let mut rc = RECT {
                top: col,
                left: if col == 0 { LVIR_LABEL } else { LVIR_BOUNDS } as i32,
                ..RECT::default()
            };
            SendMessageW(list, LVM_GETSUBITEMRECT, row, (&raw mut rc) as isize);
            let mut text = [0u16; 512];
            let item = LVITEMW {
                iSubItem: col,
                pszText: text.as_mut_ptr(),
                cchTextMax: text.len() as i32,
                ..core::mem::zeroed()
            };
            let n = SendMessageW(list, LVM_GETITEMTEXTW, row, (&raw const item) as isize);
            rc.left += pad(if col == 0 { LABEL_INSET } else { CELL_INSET });
            rc.right -= pad(CELL_INSET);
            // A right-aligned column (a count) stays right-aligned.
            let mut column = LVCOLUMNW {
                mask: LVCF_FMT,
                ..core::mem::zeroed()
            };
            SendMessageW(
                list,
                LVM_GETCOLUMNW,
                col as usize,
                (&raw mut column) as isize,
            );
            let align = if column.fmt & LVCFMT_JUSTIFYMASK == LVCFMT_RIGHT {
                DT_RIGHT
            } else {
                0
            };
            DrawTextW(
                hdc,
                text.as_ptr(),
                n.clamp(0, text.len() as isize) as i32,
                &mut rc,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | align,
            );
        }
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
        // SAFETY: two brushes kept for the life of the thread.
        let made = unsafe { (CreateSolidBrush(pal.chrome), CreateSolidBrush(pal.bg)) };
        BRUSHES.set(Some(made));
        made
    });
    let (bg, brush) = if msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX {
        (pal.bg, field)
    } else {
        (pal.chrome, face)
    };
    // SAFETY: the DC the control is about to draw with.
    unsafe {
        SetTextColor(hdc, pal.fg);
        SetBkColor(hdc, bg);
    }
    Some(brush as isize)
}

/// What every themed dialog handles alike: its colours (`WM_CTLCOLOR*`),
/// and following the system's theme (`WM_SETTINGCHANGE`) when `look` says
/// so. `Some` is the dialog procedure's answer.
pub fn common(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM, look: &Cell<Look>) -> Option<isize> {
    match msg {
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN
        | WM_CTLCOLORLISTBOX => ctl_color(msg, w as HDC, look.get().dark),
        WM_SETTINGCHANGE => {
            if let Some(new) = look.get().after_setting_change(l) {
                look.set(new);
                apply_theme(dlg, new.dark);
            }
            Some(0)
        }
        _ => None,
    }
}

/// The state a dialog procedure keeps in its dialog's GWLP_USERDATA (put
/// there at WM_INITDIALOG); `None` for the messages that come before.
///
/// # Safety
/// The dialog's GWLP_USERDATA is null or a `T` that outlives the dialog.
pub unsafe fn state<'a, T>(dlg: HWND) -> Option<&'a T> {
    // SAFETY: as the caller promises.
    unsafe { (GetWindowLongPtrW(dlg, GWLP_USERDATA) as *const T).as_ref() }
}

/// Whether the foreground window is goz-gui's. None at all (the foreground
/// changing hands, the user's window just closed) is not: then the system
/// may hand goz-gui the foreground, which only the user should.
fn foreground_is_ours() -> bool {
    // SAFETY: reads the foreground window and its process id.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        pid == GetCurrentProcessId()
    }
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
pub struct Quiet(HHOOK);

thread_local! {
    /// How many `Quiet`s live on this thread.
    static QUIET: Cell<u32> = const { Cell::new(0) };
}

/// The subclass id of a window `Quiet` shows without activating.
const QUIET_SHOW: usize = 0x5157;

impl Quiet {
    /// `None` when goz-gui is the foreground app (the user opened the
    /// dialog): nothing to hold back.
    pub fn begin() -> Option<Quiet> {
        if foreground_is_ours() {
            return None;
        }
        // SAFETY: a CBT hook for this thread only, removed by Drop.
        let hook = unsafe {
            SetWindowsHookExW(
                WH_CBT,
                Some(quiet_hook),
                core::ptr::null_mut(),
                GetCurrentThreadId(),
            )
        };
        if hook.is_null() {
            return None;
        }
        QUIET.set(QUIET.get() + 1);
        Some(Quiet(hook))
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.set(QUIET.get().saturating_sub(1));
        // SAFETY: the hook `begin` set on this thread.
        unsafe { UnhookWindowsHookEx(self.0) };
    }
}

/// Shows a new top-level window without activating it, while a `Quiet`
/// lives and goz-gui is not the foreground app; refuses an activation that
/// neither a click nor the user's switching to goz-gui asked for.
unsafe extern "system" fn quiet_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 && l != 0 {
        // SAFETY: HCBT_CREATEWND's lParam is a CBT_CREATEWNDW whose
        // CREATESTRUCTW describes the window `w` being made on this thread.
        unsafe {
            let cs = &*(*(l as *const CBT_CREATEWNDW)).lpcs;
            if cs.style as u32 & WS_CHILD == 0 {
                SetWindowSubclass(w as HWND, Some(quiet_show_proc), QUIET_SHOW, 0);
            }
        }
    }
    if code == HCBT_ACTIVATE as i32 && l != 0 {
        // SAFETY: HCBT_ACTIVATE's lParam is a CBTACTIVATESTRUCT.
        let by_click = unsafe { (*(l as *const CBTACTIVATESTRUCT)).fMouse } != 0;
        // No foreground window at all is no permission either: it is how
        // the system hands goz-gui the foreground (a window of another
        // program closed, and goz-gui's is next), which only a click on
        // goz-gui's window should do.
        if !by_click && !foreground_is_ours() {
            return 1;
        }
    }
    // SAFETY: passes the event on unchanged.
    unsafe { CallNextHookEx(core::ptr::null_mut(), code, w, l) }
}

/// The subclass of a window made while a `Quiet` lived: while one still
/// lives and goz-gui is not the foreground app, showing it does not
/// activate it.
unsafe extern "system" fn quiet_show_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: `l` is read as each message defines it; the subclass goes
    // with the window.
    unsafe {
        match msg {
            WM_WINDOWPOSCHANGING if l != 0 => {
                let pos = &mut *(l as *mut WINDOWPOS);
                if pos.flags & SWP_SHOWWINDOW != 0 && QUIET.get() > 0 && !foreground_is_ours() {
                    pos.flags |= SWP_NOACTIVATE;
                }
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(quiet_show_proc), QUIET_SHOW);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
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

/// Places `controls` for the dialog's size: each at its template
/// rectangle, moved or grown (`MOVE_X`, `GROW_Y`, ...) by what the client
/// area has over the template's `size`. In dialog units, so it holds at
/// every DPI. A combo box's height is its list's, as in the template.
pub fn anchor(dlg: HWND, size: (i16, i16), controls: &[(u16, Units, u8)]) {
    let px = |(x, y, cx, cy): Units| {
        let mut r = RECT {
            left: x.into(),
            top: y.into(),
            right: i32::from(x) + i32::from(cx),
            bottom: i32::from(y) + i32::from(cy),
        };
        // SAFETY: converts within this thread's dialog.
        unsafe { MapDialogRect(dlg, &mut r) };
        r
    };
    let mut client = RECT::default();
    // SAFETY: reads this thread's dialog.
    unsafe { GetClientRect(dlg, &mut client) };
    let base = px((0, 0, size.0, size.1));
    let grew = (
        (client.right - base.right).max(0),
        (client.bottom - base.bottom).max(0),
    );
    for &(id, units, how) in controls {
        let r = anchored(px(units), grew, how);
        // SAFETY: moves a control of this thread's dialog.
        unsafe {
            let h = GetDlgItem(dlg, i32::from(id));
            if !h.is_null() {
                MoveWindow(h, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
            }
        }
    }
}

/// Centres a dialog on its owner, kept inside the work area of the owner's
/// monitor. An owner on no monitor at all (moved off the screens) leaves it
/// centred where the owner is.
pub fn center_on_owner(dlg: HWND) {
    // SAFETY: reads and moves this thread's windows; plain out-structs.
    unsafe {
        let owner = GetWindow(dlg, GW_OWNER);
        if owner.is_null() {
            return;
        }
        let (mut o, mut d) = (RECT::default(), RECT::default());
        GetWindowRect(owner, &mut o);
        GetWindowRect(dlg, &mut d);
        let (w, h) = (d.right - d.left, d.bottom - d.top);
        let mut x = o.left + (o.right - o.left - w) / 2;
        let mut y = o.top + (o.bottom - o.top - h) / 2;
        let monitor = MonitorFromRect(&o, MONITOR_DEFAULTTONULL);
        if !monitor.is_null() {
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..core::mem::zeroed()
            };
            if GetMonitorInfoW(monitor, &mut mi) != 0 {
                let work = mi.rcWork;
                x = x.min(work.right - w).max(work.left);
                y = y.min(work.bottom - h).max(work.top);
            }
        }
        SetWindowPos(
            dlg,
            core::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn template_layout() {
        let mut t = Template::new("T", 0, 100, 50);
        let header = t.words.len();
        t.control(Class::Edit, "ab", 7, 0, 0, (1, 2, 3, 4));
        t.control(Class::Named("X"), "", 8, 0, 0, (0, 0, 1, 1));
        assert_eq!(t.words[COUNT_AT], 2);
        // Size, then the title "T", after the menu and class words.
        assert_eq!(&t.words[7..12], &[100, 50, 0, 0, u16::from(b'T')]);
        // The first control starts 4-byte aligned, its id after its
        // styles and rectangle, then the Edit atom and the text.
        let at = header + header % 2;
        assert_eq!(t.words[at + 8], 7);
        assert_eq!(&t.words[at + 9..at + 11], &[0xffff, 0x0081]);
        assert_eq!(
            &t.words[at + 11..at + 14],
            &[u16::from(b'a'), u16::from(b'b'), 0]
        );
        let second = at + 15 + (at + 15) % 2;
        assert_eq!(t.words[second + 8], 8);
        assert_eq!(&t.words[second + 9..second + 11], &[u16::from(b'X'), 0]);
        assert_eq!(second % 2, 0);
    }
}
