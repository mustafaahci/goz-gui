//! The results list's header, drawn by goz-gui in both themes so it
//! matches the rows: chrome background, the sorted column's item shaded
//! with an edge, 1px dividers and lines, and the sort direction as a small
//! triangle centred on the sorted item's top edge.
//!
//! The header tells the list, its parent, about each paint through
//! `NM_CUSTOMDRAW`; the list's subclass hands that here. The header reports
//! a pressed item in that notification but not the one under the mouse, so
//! its own subclass here tracks that.

use windows_sys::Win32::Foundation::{LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HDC, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDIS_SELECTED, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, HDF_JUSTIFYMASK, HDF_RIGHT,
    HDI_FORMAT, HDI_TEXT, HDM_LAYOUT, NMCUSTOMDRAW, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_CONTEXTMENU, WM_MBUTTONUP, WM_MOUSEMOVE};

use crate::colors::{Item, State};
use crate::paint::fill;
use crate::win::ext::b1;
use crate::win::{self, Msg, gdi, subclass};
use crate::window::App;

/// The space before and after an item's text, at 96 DPI.
pub const TEXT_PAD: i32 = 6;

/// The header's subclass ([`subclass_proc`]).
pub static SUBCLASS: subclass::Key = subclass::Key::new("results header");

/// The item at `pt` (the header's client coordinates).
fn item_at(app: &App, pt: POINT) -> Option<usize> {
    usize::try_from(b1::header_item_at(app.header, pt)).ok()
}

/// The item at `pt` (the screen).
pub fn item_at_screen(app: &App, pt: POINT) -> Option<usize> {
    // Maps a point into this thread's header.
    let at = win::screen_to_client(app.header, pt);
    item_at(app, at)
}

/// The header's subclass: notes the item under the mouse, keeps the
/// header at Everything's height, and has the column menu (right click)
/// and removal (middle click).
pub fn subclass_proc(app: &App, m: &Msg) -> LRESULT {
    let hwnd = m.hwnd;
    match m.id {
        // The list asks where its header goes, and puts the rows under it.
        HDM_LAYOUT => {
            // Passes the message on, then adjusts the HDLAYOUT the list
            // passed.
            let r = m.def();
            b1::header_layout(m, |pos, rest| {
                let height = app.header_height();
                let top = pos.y;
                pos.cy = height;
                rest.top = top + height;
            });
            return r;
        }
        WM_CONTEXTMENU => {
            // The point is on the screen.
            let pt = m.point();
            // From the keyboard (-1, -1) there is no item: the menu goes at
            // the header's corner.
            let (pt, item) = if m.l == -1 {
                // This thread's header's place.
                let rc = win::window_rect(hwnd);
                let corner = POINT {
                    x: rc.left,
                    y: rc.bottom,
                };
                (corner, None)
            } else {
                (pt, item_at_screen(app, pt))
            };
            app.header_menu(pt, item);
            return 0;
        }
        WM_MBUTTONUP => {
            if let Some(item) = item_at(app, m.point()) {
                app.remove_column(item);
            }
            return 0;
        }
        WM_MOUSEMOVE => {
            // Asks for WM_MOUSELEAVE on this thread's header.
            win::track_leave(hwnd);
            set_hot(app, item_at(app, m.point()));
        }
        WM_MOUSELEAVE => set_hot(app, None),
        _ => {}
    }
    // Passes the message on to the header.
    m.def()
}

fn set_hot(app: &App, item: Option<usize>) {
    if app.hot_header.replace(item) != item {
        // Repaints this thread's header.
        win::invalidate(app.header, None, false);
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

fn item(app: &App, cd: &NMCUSTOMDRAW) {
    let index = cd.dwItemSpec;
    let (hdi, text) =
        win::header::get_item(app.header, index as i32, HDI_TEXT | HDI_FORMAT).unwrap_or_default();
    let text: Vec<u16> = text.unwrap_or_default().encode_utf16().collect();

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

    let pad = app.scale(TEXT_PAD);
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
    // Options > Fonts and Colors' Header: Normal, or Current Sort.
    let state = if sorted {
        State::CurrentSort
    } else {
        State::Normal
    };
    // Fonts and Colors' Header font.
    let font = app.part_font(Item::Header, pal.is_bold(Item::Header, state));
    // Draws into the header's DC; the old font is put back.
    let old = gdi::Selected::new(hdc, font);
    gdi::set_bk_mode(hdc, TRANSPARENT as i32);
    gdi::set_text_color(hdc, pal.ink(Item::Header, state).fg);
    gdi::draw_text(
        hdc,
        &text,
        &mut text_rc,
        align | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    drop(old);
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
    let count = win::header::item_count(app.header);
    let mut right = 0;
    for i in 0..count {
        let rc = win::header::item_rect(app.header, i as i32).unwrap_or_default();
        right = right.max(rc.right);
    }
    // The size of this thread's header.
    let client = win::client_rect(app.header);
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
