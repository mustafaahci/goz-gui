//! goz-gui's message box: what `MessageBoxW` asks and answers, in goz-gui's
//! theme. A dialog made from [`crate::dialog`]'s template, so it is dark or
//! light as the window is (its title bar too), and follows the system's
//! theme while it is open when goz-gui does.
//!
//! As Windows' own: the system's information, warning, question and error
//! icons at the box's DPI, with their sounds; OK, OK and Cancel, or Yes and
//! No (with their access keys), right-aligned in a strip below the message;
//! the default button `MB_DEFBUTTON2` names; Enter, Esc and the close box
//! answering as Windows' do (a Yes / No box has no Esc and no close box);
//! the text word-wrapped to at most about two thirds of the work area;
//! modal to its owner; Ctrl+C copies it. A box asked for from the
//! background (a command posted to the window) shows without taking the
//! foreground (`dialog::Quiet`).
//!
//! [`show`] takes `MessageBoxW`'s arguments and gives its answers; a style
//! it does not draw (other buttons) is left to `MessageBoxW`.

use std::cell::Cell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_3DFACE, COLOR_WINDOW, COLOR_WINDOWTEXT, DC_BRUSH, DT_CALCRECT, DT_EDITCONTROL,
    DT_EXPANDTABS, DT_NOPREFIX, DT_WORDBREAK, DrawTextW, GetDC, GetMonitorInfoW, GetStockObject,
    GetSysColor, HDC, HFONT, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ReleaseDC,
    ScreenToClient, SelectObject, SetBkColor, SetDCBrushColor, SetTextColor,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::SystemServices::{
    SS_EDITCONTROL, SS_LEFT, SS_NOPREFIX, SS_OWNERDRAW,
};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, LoadIconWithScaleDown, ODT_STATIC};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_INSERT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DI_NORMAL, DM_SETDEFID, DWLP_MSGRESULT, DestroyIcon,
    DrawIconEx, EnableMenuItem, EndDialog, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GWLP_USERDATA,
    GetClientRect, GetDlgItem, GetParent, GetSystemMenu, GetWindow, GetWindowLongPtrW,
    GetWindowRect, HICON, IDCANCEL, IDI_ERROR, IDI_INFORMATION, IDI_QUESTION, IDI_WARNING, IDNO,
    IDOK, IDYES, MB_DEFBUTTON2, MB_DEFBUTTON3, MB_DEFBUTTON4, MB_DEFMASK, MB_ICONERROR,
    MB_ICONINFORMATION, MB_ICONMASK, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_OKCANCEL,
    MB_TYPEMASK, MB_YESNO, MF_BYCOMMAND, MF_GRAYED, MapDialogRect, MessageBoxW, MoveWindow,
    SC_CLOSE, SM_CXICON, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW,
    SetWindowPos, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLORDLG,
    WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_ERASEBKGND, WM_GETFONT, WM_INITDIALOG, WM_KEYDOWN,
    WM_NCDESTROY, WM_NEXTDLGCTL, WM_SETTINGCHANGE, WS_GROUP, WS_TABSTOP,
};

use crate::clipboard;
use crate::dialog::{self, Class, Look, Quiet, Template, apply_theme, center_on_owner};
use crate::paint::fill;
use crate::placement::Rect;
use crate::theme::Palette;
use crate::wide::wz;

/// The icon's and the text's controls (Windows' own box uses these ids).
const ID_ICON: u16 = 20;
const ID_TEXT: u16 = 0xffff;
/// The buttons' subclass id (Ctrl+C).
const COPY_SUBCLASS: usize = 0x4d42;
/// Sent by a button for Ctrl+C or Ctrl+Insert: the box copies itself.
const WM_COPY_BOX: u32 = WM_APP + 0x4d;
/// Round the buttons in their strip, in pixels at 96 DPI.
const STRIP_PAD: i32 = 11;

/// The buttons a style asks for: each one's answer and label, the one that
/// is the default, and what Esc and the close box answer (`None`: they do
/// nothing, and the close box is greyed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Buttons {
    set: &'static [(i32, &'static str)],
    default: usize,
    cancel: Option<i32>,
}

/// The buttons of `style`, as `MessageBoxW` has them; `None` for a set
/// goz-gui does not draw.
fn buttons(style: u32) -> Option<Buttons> {
    let set: &'static [(i32, &'static str)] = match style & MB_TYPEMASK {
        MB_OK => &[(IDOK, "OK")],
        MB_OKCANCEL => &[(IDOK, "OK"), (IDCANCEL, "Cancel")],
        MB_YESNO => &[(IDYES, "&Yes"), (IDNO, "&No")],
        _ => return None,
    };
    let default = match style & MB_DEFMASK {
        MB_DEFBUTTON2 => 1,
        MB_DEFBUTTON3 => 2,
        MB_DEFBUTTON4 => 3,
        _ => 0,
    };
    // A default past the last button is the first, as in Windows.
    let default = if default < set.len() { default } else { 0 };
    // Esc answers Cancel when there is one, OK when OK is all there is,
    // and nothing in a Yes / No box.
    let cancel = if set.iter().any(|&(id, _)| id == IDCANCEL) {
        Some(IDCANCEL)
    } else if set.len() == 1 {
        Some(set[0].0)
    } else {
        None
    };
    Some(Buttons {
        set,
        default,
        cancel,
    })
}

/// The icon `style` shows: the system icon's resource number, and the
/// sound (`MB_ICON*`) that goes with it.
fn icon_of(style: u32) -> Option<(usize, u32)> {
    let id = match style & MB_ICONMASK {
        MB_ICONERROR => IDI_ERROR,
        MB_ICONQUESTION => IDI_QUESTION,
        MB_ICONWARNING => IDI_WARNING,
        MB_ICONINFORMATION => IDI_INFORMATION,
        _ => return None,
    };
    Some((id as usize, style & MB_ICONMASK))
}

/// What Ctrl+C puts on the clipboard, as Windows' message box writes it.
fn copy_text(title: &str, text: &str, labels: &[&str]) -> String {
    const RULE: &str = "---------------------------\r\n";
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    let buttons: String = labels
        .iter()
        .map(|l| format!("{}   ", l.replace('&', "")))
        .collect();
    format!("{RULE}{title}\r\n{RULE}{text}\r\n{RULE}{buttons}\r\n{RULE}")
}

/// Sizes in pixels at the box's DPI.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    /// Round the message.
    pad: i32,
    /// The icon's side; 0 without one.
    icon: i32,
    /// Between the icon and the text.
    icon_gap: i32,
    button: (i32, i32),
    button_gap: i32,
    /// Round the buttons in their strip.
    strip_pad: i32,
}

impl Metrics {
    /// Where the text starts.
    fn text_x(&self) -> i32 {
        self.pad
            + if self.icon > 0 {
                self.icon + self.icon_gap
            } else {
                0
            }
    }
}

/// Where everything goes, in client pixels.
#[derive(Debug, PartialEq, Eq)]
struct Layout {
    client: (i32, i32),
    icon: Rect,
    text: Rect,
    buttons: Vec<Rect>,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

/// Lays the box out for a text of `text` pixels and `n` buttons: the icon
/// at the top left, the text beside it (a text shorter than the icon
/// centred on it), the buttons at the right of a strip below.
fn layout(m: &Metrics, text: (i32, i32), n: usize) -> Layout {
    let n = n as i32;
    let x = m.text_x();
    let area = text.1.max(m.icon);
    let (bw, bh) = m.button;
    let row = n * bw + (n - 1).max(0) * m.button_gap;
    let width = (x + text.0 + m.pad).max(row + 2 * m.strip_pad);
    let strip_top = m.pad + area + m.pad;
    let height = strip_top + 2 * m.strip_pad + bh;
    let mut bx = width - m.strip_pad - row;
    let by = strip_top + m.strip_pad;
    let buttons = (0..n)
        .map(|_| {
            let r = rect(bx, by, bw, bh);
            bx += bw + m.button_gap;
            r
        })
        .collect();
    Layout {
        client: (width, height),
        icon: rect(m.pad, m.pad, m.icon, m.icon),
        text: rect(x, m.pad + (area - text.1) / 2, text.0, text.1),
        buttons,
    }
}

/// The box's state, in its GWLP_USERDATA while it is open.
struct MsgBox {
    /// The text, NUL-terminated.
    text: Vec<u16>,
    title: String,
    buttons: Buttons,
    icon: Option<(usize, u32)>,
    look: Cell<Look>,
    /// The box has been active: its default button had the keyboard then.
    activated: Cell<bool>,
}

/// Shows a message box over `owner` as `MessageBoxW(owner, text, title,
/// style)` does, in goz-gui's theme; its answer (`IDOK`, `IDCANCEL`,
/// `IDYES`, `IDNO`).
pub fn show(owner: HWND, text: &str, title: &str, style: u32) -> i32 {
    let Some(buttons) = buttons(style) else {
        // SAFETY: NUL-terminated strings for a modal box owned by `owner`.
        return unsafe { MessageBoxW(owner, wz(text).as_ptr(), wz(title).as_ptr(), style) };
    };
    let state = MsgBox {
        text: wz(text),
        title: title.to_owned(),
        buttons,
        icon: icon_of(style),
        look: Cell::new(dialog::app_look()),
        activated: Cell::new(false),
    };
    let mut t = Template::new(title, 0, 0, 0);
    if state.icon.is_some() {
        t.control(Class::Static, "", ID_ICON, SS_OWNERDRAW, 0, (0, 0, 0, 0));
    }
    t.control(
        Class::Static,
        text,
        ID_TEXT,
        SS_LEFT | SS_NOPREFIX | SS_EDITCONTROL,
        0,
        (0, 0, 0, 0),
    );
    for (i, &(id, label)) in buttons.set.iter().enumerate() {
        let mut style = WS_TABSTOP;
        style |= if i == buttons.default {
            BS_DEFPUSHBUTTON as u32
        } else {
            BS_PUSHBUTTON as u32
        };
        // One group: the arrow keys go from button to button.
        if i == 0 {
            style |= WS_GROUP;
        }
        t.control(Class::Button, label, id as u16, style, 0, (0, 0, 0, 0));
    }
    // Asked for by a command posted while goz-gui is in the background:
    // shown, but not brought to the front.
    let _quiet = Quiet::begin();
    match t.run(owner, Some(proc), (&raw const state) as LPARAM) {
        // The dialog could not be made: Windows' own box, unthemed.
        // SAFETY: NUL-terminated strings for a modal box owned by `owner`.
        -1 => unsafe { MessageBoxW(owner, wz(text).as_ptr(), wz(title).as_ptr(), style) },
        // 0 when the owner was not a window (as `MessageBoxW` fails) or
        // the box went with it.
        answer => answer as i32,
    }
}

/// The message area's and the button strip's colours, and the text's: in
/// the light theme the system's, as Windows' box has them.
fn colours(dark: bool) -> (u32, u32, u32) {
    if dark {
        let pal = Palette::new(true);
        (pal.popup_bg, pal.chrome, pal.fg)
    } else {
        // SAFETY: reads system colours.
        unsafe {
            (
                GetSysColor(COLOR_WINDOW),
                GetSysColor(COLOR_3DFACE),
                GetSysColor(COLOR_WINDOWTEXT),
            )
        }
    }
}

fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: looks up a child of this thread's dialog.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

/// `v` pixels at 96 DPI at `dpi`.
fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

impl MsgBox {
    /// Sizes and places the box and its controls, for its text in its font
    /// at its DPI, centred on its owner.
    fn init(&self, dlg: HWND) {
        let (m, text, frame) = self.measure(dlg);
        let l = layout(&m, text, self.buttons.set.len());
        let place = |id: u16, r: &Rect| {
            let h = item(dlg, id);
            if !h.is_null() {
                // SAFETY: moves a control of this thread's dialog.
                unsafe { MoveWindow(h, r.x, r.y, r.w, r.h, 0) };
            }
        };
        place(ID_ICON, &l.icon);
        place(ID_TEXT, &l.text);
        for (&(id, _), r) in self.buttons.set.iter().zip(&l.buttons) {
            place(id as u16, r);
        }
        // SAFETY: sizes this thread's dialog, which is not shown yet.
        unsafe {
            SetWindowPos(
                dlg,
                core::ptr::null_mut(),
                0,
                0,
                l.client.0 + frame.0,
                l.client.1 + frame.1,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        if self.buttons.cancel.is_none() {
            // SAFETY: greys the close box of this thread's dialog.
            unsafe { EnableMenuItem(GetSystemMenu(dlg, 0), SC_CLOSE, MF_BYCOMMAND | MF_GRAYED) };
        }
        for &(id, _) in self.buttons.set {
            // SAFETY: subclasses a button of this thread's dialog; the
            // subclass removes itself when the button goes.
            unsafe {
                SetWindowSubclass(item(dlg, id as u16), Some(button_proc), COPY_SUBCLASS, 0);
            }
        }
        self.focus_default(dlg);
        // SAFETY: plays the sound of the box's icon (the default sound
        // without one), as Windows' box does.
        unsafe { MessageBeep(self.icon.map_or(MB_OK, |(_, sound)| sound)) };
    }

    /// The box's sizes at its DPI, its text's size wrapped to at most about
    /// two thirds of its owner's work area, and what its frame adds to its
    /// client area.
    fn measure(&self, dlg: HWND) -> (Metrics, (i32, i32), (i32, i32)) {
        // SAFETY: reads this thread's dialog: its DPI, font and styles, a
        // DC for measuring (released), and its owner's monitor's work area;
        // every out-struct outlives its call.
        unsafe {
            let dpi = GetDpiForWindow(dlg).max(96);
            let mut du = rect(0, 0, 50, 14).to_win();
            MapDialogRect(dlg, &mut du);
            let mut m = Metrics {
                pad: scale(24, dpi),
                icon: if self.icon.is_some() {
                    GetSystemMetricsForDpi(SM_CXICON, dpi)
                } else {
                    0
                },
                icon_gap: scale(12, dpi),
                button: (du.right, du.bottom),
                button_gap: scale(8, dpi),
                strip_pad: scale(STRIP_PAD, dpi),
            };
            let style = GetWindowLongPtrW(dlg, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(dlg, GWL_EXSTYLE) as u32;
            let mut frame = RECT::default();
            AdjustWindowRectExForDpi(&mut frame, style, 0, ex, dpi);
            let frame = (frame.right - frame.left, frame.bottom - frame.top);
            let owner = GetWindow(dlg, GW_OWNER);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..core::mem::zeroed()
            };
            GetMonitorInfoW(
                MonitorFromWindow(
                    if owner.is_null() { dlg } else { owner },
                    MONITOR_DEFAULTTONEAREST,
                ),
                &mut mi,
            );
            let work = mi.rcWork.right - mi.rcWork.left;
            let most = (work * 2 / 3 - frame.0 - m.text_x() - m.pad).max(scale(160, dpi));
            let font = SendMessageW(dlg, WM_GETFONT, 0, 0) as HFONT;
            let hdc = GetDC(dlg);
            let old = SelectObject(hdc, font);
            let mut rc = rect(0, 0, most, 0).to_win();
            DrawTextW(
                hdc,
                self.text.as_ptr(),
                self.text.len().saturating_sub(1) as i32,
                &mut rc,
                DT_CALCRECT | DT_WORDBREAK | DT_EXPANDTABS | DT_NOPREFIX | DT_EDITCONTROL,
            );
            // Each button wide enough for its label.
            for &(_, label) in self.buttons.set {
                let label = wz(&label.replace('&', ""));
                let mut lr = RECT::default();
                DrawTextW(
                    hdc,
                    label.as_ptr(),
                    label.len() as i32 - 1,
                    &mut lr,
                    DT_CALCRECT | DT_NOPREFIX,
                );
                m.button.0 = m.button.0.max(lr.right + scale(20, dpi));
            }
            SelectObject(hdc, old);
            ReleaseDC(dlg, hdc);
            // A pixel wider than measured, so the static wraps as measured.
            (m, (rc.right + 1, rc.bottom.max(1)), frame)
        }
    }

    /// Gives the default button the keyboard (and Enter).
    fn focus_default(&self, dlg: HWND) {
        let (id, _) = self.buttons.set[self.buttons.default];
        // SAFETY: messages to this thread's dialog about its own button.
        unsafe {
            SendMessageW(dlg, DM_SETDEFID, id as usize, 0);
            SendMessageW(dlg, WM_NEXTDLGCTL, item(dlg, id as u16) as usize, 1);
        }
    }

    /// Paints the message area and the button strip below it.
    fn erase(&self, dlg: HWND, hdc: HDC) {
        let (area, strip, _) = colours(self.look.get().dark);
        let mut rc = RECT::default();
        let mut first = RECT::default();
        // SAFETY: reads this thread's dialog and its first button.
        let dpi = unsafe {
            GetClientRect(dlg, &mut rc);
            GetWindowRect(item(dlg, self.buttons.set[0].0 as u16), &mut first);
            GetDpiForWindow(dlg).max(96)
        };
        // The strip's top, from where its buttons are now (they move with
        // the DPI).
        let mut top = POINT {
            x: first.left,
            y: first.top,
        };
        // SAFETY: converts a point to this thread's dialog's client area.
        unsafe { ScreenToClient(dlg, &mut top) };
        let strip_top = (top.y - scale(STRIP_PAD, dpi)).clamp(0, rc.bottom);
        fill(
            hdc,
            &RECT {
                bottom: strip_top,
                ..rc
            },
            area,
        );
        fill(
            hdc,
            &RECT {
                top: strip_top,
                ..rc
            },
            strip,
        );
    }

    /// Draws the icon in its control, at the control's size.
    fn draw_icon(&self, dis: &DRAWITEMSTRUCT) {
        let (area, _, _) = colours(self.look.get().dark);
        fill(dis.hDC, &dis.rcItem, area);
        let Some((id, _)) = self.icon else { return };
        let r = dis.rcItem;
        let side = (r.right - r.left).min(r.bottom - r.top);
        if side <= 0 {
            return;
        }
        let mut icon: HICON = core::ptr::null_mut();
        // SAFETY: a system icon at the control's size, drawn on the DC the
        // control gave, then destroyed (it is this call's own copy).
        unsafe {
            if LoadIconWithScaleDown(core::ptr::null_mut(), id as _, side, side, &mut icon) >= 0
                && !icon.is_null()
            {
                DrawIconEx(
                    dis.hDC,
                    r.left,
                    r.top,
                    icon,
                    side,
                    side,
                    0,
                    core::ptr::null_mut(),
                    DI_NORMAL,
                );
                DestroyIcon(icon);
            }
        }
    }

    /// Ctrl+C: the box as text on the clipboard.
    fn copy(&self, dlg: HWND) {
        let labels: Vec<&str> = self.buttons.set.iter().map(|&(_, l)| l).collect();
        let text = String::from_utf16_lossy(&self.text[..self.text.len().saturating_sub(1)]);
        let all: Vec<u16> = copy_text(&self.title, &text, &labels)
            .encode_utf16()
            .collect();
        clipboard::put_text(dlg, &all);
    }
}

/// The colours of the box's controls: the brush for the dialog procedure
/// to answer `msg` with.
fn ctl_colour(msg: u32, hdc: HDC, dark: bool) -> isize {
    let (area, strip, text) = colours(dark);
    let bg = if msg == WM_CTLCOLORSTATIC {
        area
    } else {
        strip
    };
    // SAFETY: the DC the control is about to draw with; the DC brush takes
    // its colour from it.
    unsafe {
        SetTextColor(hdc, text);
        SetBkColor(hdc, bg);
        SetDCBrushColor(hdc, bg);
        GetStockObject(DC_BRUSH) as isize
    }
}

unsafe extern "system" fn proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `MsgBox` that `show` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `MsgBox` from `show`, or null before.
    let Some(s) = (unsafe { dialog::state::<MsgBox>(dlg) }) else {
        return 0;
    };
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The keyboard is on the default button already.
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORDLG => {
            ctl_colour(msg, w as HDC, s.look.get().dark)
        }
        WM_ERASEBKGND => {
            s.erase(dlg, w as HDC);
            // SAFETY: the answer of this dialog's WM_ERASEBKGND.
            unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, 1) };
            1
        }
        WM_DRAWITEM => {
            // SAFETY: WM_DRAWITEM's lParam is a DRAWITEMSTRUCT.
            let dis = unsafe { &*(l as *const DRAWITEMSTRUCT) };
            if dis.CtlType != ODT_STATIC || dis.CtlID != u32::from(ID_ICON) {
                return 0;
            }
            s.draw_icon(dis);
            1
        }
        // The first activation puts the keyboard on the default button, not
        // the dialog's first: a box shown in the background (`Quiet`) is
        // first active when it is clicked.
        WM_ACTIVATE if (w & 0xffff) as u32 != WA_INACTIVE && !s.activated.replace(true) => {
            s.focus_default(dlg);
            1
        }
        WM_SETTINGCHANGE => {
            if let Some(new) = s.look.get().after_setting_change(l) {
                s.look.set(new);
                apply_theme(dlg, new.dark);
            }
            0
        }
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            // Esc and the close box come as IDCANCEL.
            let answer = if id == IDCANCEL {
                s.buttons.cancel
            } else {
                s.buttons.set.iter().find(|b| b.0 == id).map(|b| b.0)
            };
            if let Some(answer) = answer {
                // SAFETY: ends this modal dialog.
                unsafe { EndDialog(dlg, answer as isize) };
            }
            1
        }
        WM_COPY_BOX => {
            s.copy(dlg);
            1
        }
        _ => 0,
    }
}

/// A button's subclass: Ctrl+C (and Ctrl+Insert) copies the box.
unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> isize {
    // SAFETY: `w`/`l` are read as each message defines them; the subclass
    // goes with the button.
    unsafe {
        match msg {
            WM_KEYDOWN
                if (w == usize::from(b'C') || w == usize::from(VK_INSERT))
                    && GetKeyState(i32::from(VK_CONTROL)) < 0 =>
            {
                SendMessageW(GetParent(hwnd), WM_COPY_BOX, 0, 0);
                return 0;
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(button_proc), COPY_SUBCLASS);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_answer_as_windows_box() {
        let ok = buttons(MB_OK | MB_ICONWARNING).unwrap();
        assert_eq!(ok.set, &[(IDOK, "OK")]);
        assert_eq!((ok.default, ok.cancel), (0, Some(IDOK)));
        let oc = buttons(MB_OKCANCEL).unwrap();
        assert_eq!(oc.set.len(), 2);
        assert_eq!((oc.default, oc.cancel), (0, Some(IDCANCEL)));
        // Yes / No: No the default, no Esc.
        let yn = buttons(MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2).unwrap();
        assert_eq!(yn.set, &[(IDYES, "&Yes"), (IDNO, "&No")]);
        assert_eq!((yn.default, yn.cancel), (1, None));
        // A default past the last button is the first.
        assert_eq!(buttons(MB_OK | MB_DEFBUTTON3).unwrap().default, 0);
        // Others (Yes / No / Cancel here) are Windows' own box's.
        assert_eq!(buttons(3), None);
    }

    #[test]
    fn each_icon_is_the_systems_with_its_sound() {
        assert_eq!(icon_of(MB_OK), None);
        assert_eq!(
            icon_of(MB_ICONWARNING | MB_YESNO | MB_DEFBUTTON2),
            Some((IDI_WARNING as usize, MB_ICONWARNING))
        );
        assert_eq!(
            icon_of(MB_ICONQUESTION | MB_YESNO),
            Some((IDI_QUESTION as usize, MB_ICONQUESTION))
        );
        assert_eq!(
            icon_of(MB_ICONINFORMATION),
            Some((IDI_INFORMATION as usize, MB_ICONINFORMATION))
        );
        assert_eq!(
            icon_of(MB_ICONERROR),
            Some((IDI_ERROR as usize, MB_ICONERROR))
        );
    }

    #[test]
    fn ctrl_c_copies_as_windows_box_does() {
        assert_eq!(
            copy_text("goz", "Open 3 items?\nSure.", &["&Yes", "&No"]),
            "---------------------------\r\ngoz\r\n---------------------------\r\n\
             Open 3 items?\r\nSure.\r\n---------------------------\r\nYes   No   \r\n\
             ---------------------------\r\n"
        );
    }

    #[test]
    fn the_layout_puts_the_buttons_right_below_the_message() {
        let m = Metrics {
            pad: 24,
            icon: 32,
            icon_gap: 12,
            button: (88, 26),
            button_gap: 8,
            strip_pad: 11,
        };
        // One line, shorter than the icon: centred on it.
        let l = layout(&m, (200, 15), 2);
        assert_eq!(l.icon, rect(24, 24, 32, 32));
        assert_eq!(l.text, rect(68, 24 + 8, 200, 15));
        let width = 68 + 200 + 24;
        let strip = 24 + 32 + 24;
        assert_eq!(l.client, (width, strip + 22 + 26));
        assert_eq!(l.buttons[1], rect(width - 11 - 88, strip + 11, 88, 26));
        assert_eq!(l.buttons[0].x + 88 + 8, l.buttons[1].x);
        // A short text is no narrower than its buttons; a tall one sets the
        // height.
        let l = layout(&m, (10, 100), 2);
        assert_eq!(l.client.0, 2 * 88 + 8 + 22);
        assert_eq!(l.text.y, 24);
        assert_eq!(l.client.1, 24 + 100 + 24 + 22 + 26);
        // Without an icon the text starts at the margin.
        let l = layout(&Metrics { icon: 0, ..m }, (300, 40), 1);
        assert_eq!(l.text.x, 24);
    }
}
