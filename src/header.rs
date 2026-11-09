//! The results list's header, drawn by goz-gui in both themes so it
//! matches the rows: chrome background, the sorted column's item shaded
//! with an edge, 1px dividers and lines, and the sort direction as a small
//! triangle centred on the sorted item's top edge.
//!
//! The header tells the list, its parent, about each paint through
//! `NM_CUSTOMDRAW`; the list's subclass hands that here. The header reports
//! a pressed item in that notification but not the one under the mouse, so
//! its own subclass here tracks that.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DrawTextW, HDC,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDIS_SELECTED, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, HDF_JUSTIFYMASK, HDF_RIGHT,
    HDHITTESTINFO, HDI_FORMAT, HDI_TEXT, HDITEMW, HDLAYOUT, HDM_GETITEMCOUNT, HDM_GETITEMRECT,
    HDM_GETITEMW, HDM_HITTEST, HDM_LAYOUT, NMCUSTOMDRAW, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::DefSubclassProc;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, SendMessageW, WM_CONTEXTMENU, WM_MBUTTONUP, WM_MOUSEMOVE,
};

use crate::paint::fill;
use crate::window::App;

/// The item at `pt` (the header's client coordinates).
fn item_at(app: &App, pt: POINT) -> Option<usize> {
    let mut hit = HDHITTESTINFO {
        pt,
        flags: 0,
        iItem: -1,
    };
    let item = send(HDM_HITTEST, 0, (&raw mut hit) as isize, app);
    usize::try_from(item).ok()
}

fn client_point(l: LPARAM) -> POINT {
    POINT {
        x: (l & 0xffff) as i16 as i32,
        y: ((l >> 16) & 0xffff) as i16 as i32,
    }
}

/// The header's subclass: notes the item under the mouse, keeps the
/// header at Everything's height, and has the column menu (right click)
/// and removal (middle click).
pub unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the leaked `App` passed to SetWindowSubclass.
    let app = unsafe { &*(data as *const App) };
    match msg {
        // The list asks where its header goes, and puts the rows under it.
        HDM_LAYOUT => {
            // SAFETY: passes the message on, then adjusts the HDLAYOUT the
            // list passed, whose pointers it owns across the call.
            unsafe {
                let r = DefSubclassProc(hwnd, msg, w, l);
                let layout = &*(l as *const HDLAYOUT);
                if !layout.prc.is_null() && !layout.pwpos.is_null() {
                    let height = app.header_height();
                    let top = (*layout.pwpos).y;
                    (*layout.pwpos).cy = height;
                    (*layout.prc).top = top + height;
                }
                return r;
            }
        }
        WM_CONTEXTMENU => {
            let pt = client_point(l);
            // From the keyboard (-1, -1) there is no item: the menu goes at
            // the header's corner.
            let pt = if l == -1 {
                let mut rc = RECT::default();
                // SAFETY: reads this thread's header's place.
                unsafe { GetWindowRect(hwnd, &mut rc) };
                POINT {
                    x: rc.left,
                    y: rc.bottom,
                }
            } else {
                pt
            };
            app.header_menu(pt);
            return 0;
        }
        WM_MBUTTONUP => {
            if let Some(item) = item_at(app, client_point(l)) {
                app.remove_column(item);
            }
            return 0;
        }
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: asks for WM_MOUSELEAVE on this thread's header.
            unsafe { TrackMouseEvent(&mut tme) };
            set_hot(app, item_at(app, client_point(l)));
        }
        WM_MOUSELEAVE => set_hot(app, None),
        _ => {}
    }
    // SAFETY: passes the message on to the header.
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}

fn set_hot(app: &App, item: Option<usize>) {
    if app.hot_header.replace(item) != item {
        // SAFETY: repaints this thread's header.
        unsafe { InvalidateRect(app.header, core::ptr::null(), 0) };
    }
}

/// Answers the header's custom-draw notification: every item is drawn
/// here, and the strip right of the last one after the header's own paint.
pub fn custom_draw(app: &App, cd: &NMCUSTOMDRAW) -> LRESULT {
    match cd.dwDrawStage {
        CDDS_PREPAINT => (CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT) as LRESULT,
        CDDS_ITEMPREPAINT => {
            item(app, cd);
            CDRF_SKIPDEFAULT as LRESULT
        }
        CDDS_POSTPAINT => {
            rest(app, cd.hdc);
            CDRF_DODEFAULT as LRESULT
        }
        _ => CDRF_DODEFAULT as LRESULT,
    }
}

fn send(msg: u32, w: usize, l: isize, app: &App) -> isize {
    // SAFETY: a query to the header of this thread's list; any pointer in
    // `l` is the caller's and lives across the synchronous call.
    unsafe { SendMessageW(app.header, msg, w, l) }
}

fn item(app: &App, cd: &NMCUSTOMDRAW) {
    let index = cd.dwItemSpec;
    let mut text = [0u16; 128];
    let mut hdi = HDITEMW {
        mask: HDI_TEXT | HDI_FORMAT,
        pszText: text.as_mut_ptr(),
        cchTextMax: text.len() as i32,
        // SAFETY: the rest of HDITEMW is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    send(HDM_GETITEMW, index, (&raw mut hdi) as isize, app);
    let len = text.iter().position(|&c| c == 0).unwrap_or(text.len());

    let pal = app.pal.get();
    let hdc = cd.hdc;
    let rc = cd.rc;
    let sorted = app.column_id(index) == Some(app.sorted_column());
    let hot = app.hot_header.get() == Some(index);
    let pressed = cd.uItemState & CDIS_SELECTED != 0;
    let bg = match (pressed, hot, sorted) {
        (true, _, _) => pal.header_pressed,
        (false, true, true) => pal.header_sort_hot,
        (false, true, false) => pal.header_hot,
        (false, false, true) => pal.header_sort,
        (false, false, false) => pal.header,
    };
    fill(hdc, &rc, bg);
    lines(app, hdc, &rc);
    let inner = RECT {
        top: rc.top + 1,
        bottom: rc.bottom - 1,
        ..rc
    };
    if sorted {
        // An edge round the sorted item, over the divider and bottom line.
        let edge = pal.header_sort_edge;
        fill(
            hdc,
            &RECT {
                right: rc.left + 1,
                ..inner
            },
            edge,
        );
        fill(
            hdc,
            &RECT {
                left: rc.right - 1,
                ..inner
            },
            edge,
        );
        fill(
            hdc,
            &RECT {
                top: rc.bottom - 1,
                ..rc
            },
            edge,
        );
    } else {
        fill(
            hdc,
            &RECT {
                left: rc.right - 1,
                ..inner
            },
            pal.divider,
        );
    }

    let pad = app.scale(6);
    let mut text_rc = RECT {
        left: rc.left + pad,
        right: rc.right - pad,
        ..inner
    };
    let align = if hdi.fmt & HDF_JUSTIFYMASK == HDF_RIGHT {
        DT_RIGHT
    } else {
        DT_LEFT
    };
    let font = app.fonts.borrow().normal;
    // SAFETY: draws into the header's DC; the old font is put back.
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, pal.fg);
        DrawTextW(
            hdc,
            text.as_mut_ptr(),
            len as i32,
            &mut text_rc,
            align | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
    if sorted {
        arrow(app, hdc, &rc);
    }
}

/// The top line and bottom line every item and the empty strip share.
fn lines(app: &App, hdc: HDC, rc: &RECT) {
    let line = app.pal.get().line;
    fill(
        hdc,
        &RECT {
            bottom: rc.top + 1,
            ..*rc
        },
        line,
    );
    fill(
        hdc,
        &RECT {
            top: rc.bottom - 1,
            ..*rc
        },
        line,
    );
}

/// The sort direction: a filled triangle centred at the item's top edge,
/// pointing up for ascending and down for descending, drawn row by row so
/// it stays crisp at every scale.
fn arrow(app: &App, hdc: HDC, rc: &RECT) {
    let half = app.scale(2).max(2);
    let cx = (rc.left + rc.right) / 2;
    let top = rc.top + 1 + app.scale(2);
    let down = !app.sort_ascending();
    let colour = app.pal.get().arrow;
    for i in 0..=half {
        let reach = if down { half - i } else { i };
        let row = RECT {
            left: cx - reach,
            right: cx + reach + 1,
            top: top + i,
            bottom: top + i + 1,
        };
        fill(hdc, &row, colour);
    }
}

/// The header right of its last item, which the header itself would draw in
/// its theme's light colours.
fn rest(app: &App, hdc: HDC) {
    let count = send(HDM_GETITEMCOUNT, 0, 0, app).max(0) as usize;
    let mut right = 0;
    for i in 0..count {
        let mut rc = RECT::default();
        send(HDM_GETITEMRECT, i, (&raw mut rc) as isize, app);
        right = right.max(rc.right);
    }
    let mut client = RECT::default();
    // SAFETY: reads the size of this thread's header.
    unsafe { GetClientRect(app.header, &mut client) };
    if right >= client.right {
        return;
    }
    let strip = RECT {
        left: right,
        ..client
    };
    fill(hdc, &strip, app.pal.get().header);
    lines(app, hdc, &strip);
}
