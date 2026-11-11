//! The status bar: the result count and the selection on the left; from the
//! right edge, the total size, the search options in force, and gozd's own
//! state. Double-clicking an option's label turns that option off.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DC_BRUSH, DC_PEN, DT_CALCRECT, DT_MODIFYSTRING, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_SINGLELINE,
    DrawTextW, ETO_CLIPPED, ExtTextOutW, GetStockObject, HDC, PtInRect, RoundRect, SelectObject,
    SetBkMode, SetDCBrushColor, SetDCPenColor, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::LVM_GETSELECTEDCOUNT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

use crate::format;
use crate::paint::{fill, width};
use crate::window::App;

/// The view buttons at the status bar's right end, as in Everything 1.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewButton {
    Details,
    Thumbnails,
}

/// Where the view buttons go in the status bar `rc` (its top line
/// included): squares of `side`, `gap` apart, the last `margin` from the
/// right edge, centred below the line. None when the bar is hidden.
pub fn view_buttons(rc: &RECT, side: i32, gap: i32, margin: i32) -> [(ViewButton, RECT); 2] {
    let top = rc.top + 1 + (rc.bottom - rc.top - 1 - side) / 2;
    let square = |right: i32| RECT {
        left: right - side,
        top,
        right,
        bottom: top + side,
    };
    let thumbs = square(rc.right - margin);
    let details = square(thumbs.left - gap);
    [
        (ViewButton::Details, details),
        (ViewButton::Thumbnails, thumbs),
    ]
}

/// The view buttons' places in the window's status bar now.
fn buttons_of(app: &App) -> Option<[(ViewButton, RECT); 2]> {
    let rc = app.status_rc.get();
    // The button fills the bar but for a little room above and below.
    let side = (rc.bottom - rc.top - 1 - app.scale(4)).min(app.scale(20));
    (side > 0).then(|| view_buttons(&rc, side, app.scale(2), app.scale(4)))
}

/// The button of `buttons` at `pt`.
pub fn button_at(buttons: &[(ViewButton, RECT)], pt: POINT) -> Option<ViewButton> {
    buttons
        .iter()
        // SAFETY: reads two plain structs.
        .find(|(_, rc)| unsafe { PtInRect(rc, pt) } != 0)
        .map(|&(b, _)| b)
}

/// The view button at `pt`, in the main window's client coordinates.
pub fn view_button_at(app: &App, pt: POINT) -> Option<ViewButton> {
    button_at(&buttons_of(app)?, pt)
}

/// The mouse moved to `pt` (None: it left the window): the button under
/// it lights up.
pub fn view_hover(app: &App, pt: Option<POINT>) {
    let hot = pt.and_then(|pt| view_button_at(app, pt));
    if hot.is_some() {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: app.hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: asks for WM_MOUSELEAVE on this thread's window.
        unsafe { TrackMouseEvent(&mut tme) };
    }
    if app.view_hot.replace(hot) != hot {
        app.invalidate_status();
    }
}

/// Draws the view buttons: the view shown pressed (a lighter rounded
/// square), the one under the mouse lit; Details' glyph four lines,
/// Thumbnails' four squares.
fn paint_view_buttons(app: &App, hdc: HDC, buttons: &[(ViewButton, RECT)]) {
    let pal = app.pal.get();
    let thumbnails = app.view().thumbnails;
    let hot = app.view_hot.get();
    let px = app.scale(1).max(1);
    for &(button, rc) in buttons {
        let active = (button == ViewButton::Thumbnails) == thumbnails;
        let bg = if active {
            pal.glyph_hot_bg
        } else if hot == Some(button) {
            pal.header_hot
        } else {
            pal.chrome
        };
        if bg != pal.chrome {
            round_fill(hdc, &rc, bg, app.scale(4));
        }
        let fg = if active || hot == Some(button) {
            pal.glyph_hot
        } else {
            pal.glyph
        };
        // A 12 px glyph (at 96 DPI) in the middle.
        let g = app.scale(12);
        let left = rc.left + (rc.right - rc.left - g) / 2;
        let top = rc.top + (rc.bottom - rc.top - g) / 2;
        let bar = |l: i32, t: i32, r: i32, b: i32| {
            fill(
                hdc,
                &RECT {
                    left: l,
                    top: t,
                    right: r,
                    bottom: b,
                },
                fg,
            );
        };
        match button {
            ViewButton::Details => {
                for i in 0..4 {
                    let t = top + i * g / 4 + px;
                    bar(left, t, left + g, t + 2 * px);
                }
            }
            ViewButton::Thumbnails => {
                let half = (g - px) / 2;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let l = left + dx * (g - half);
                    let t = top + dy * (g - half);
                    bar(l, t, l + half, t + half);
                }
            }
        }
    }
}

/// Fills `rc` with `colour`, its corners rounded by `radius`.
fn round_fill(hdc: HDC, rc: &RECT, colour: u32, radius: i32) {
    // SAFETY: the stock DC brush and pen paint in the colours set here; the
    // DC's own are put back.
    unsafe {
        SetDCBrushColor(hdc, colour);
        SetDCPenColor(hdc, colour);
        let old_brush = SelectObject(hdc, GetStockObject(DC_BRUSH));
        let old_pen = SelectObject(hdc, GetStockObject(DC_PEN));
        RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
    }
}

/// A search option the status bar shows while it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indicator {
    Case,
    Path,
    Filter,
    /// The Folders pane's folder, which keeps the search inside it.
    Folder,
    /// The open file list (wave 4), searched instead of gozd's index.
    FileList,
}

/// `text` (a path) cut to `max` px wide in `hdc`'s font with an ellipsis in
/// its middle, as Explorer shortens paths (`C:\Users\…\Projects`); as it
/// is when it fits.
fn elide_path(hdc: HDC, text: &[u16], max: i32) -> Vec<u16> {
    if width(hdc, text) <= max {
        return text.to_vec();
    }
    // DT_MODIFYSTRING writes the shortened text back, with room for the
    // ellipsis and its NUL.
    let mut buf = text.to_vec();
    buf.extend_from_slice(&[0; 5]);
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: max.max(1),
        bottom: 1,
    };
    // SAFETY: the buffer holds the text and five more units, as
    // DT_MODIFYSTRING needs.
    unsafe {
        DrawTextW(
            hdc,
            buf.as_mut_ptr(),
            text.len() as i32,
            &mut rc,
            DT_SINGLELINE | DT_NOPREFIX | DT_PATH_ELLIPSIS | DT_MODIFYSTRING | DT_CALCRECT,
        );
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    buf.truncate(end);
    buf
}

/// What is selected, as the left side describes it.
#[derive(Debug, PartialEq)]
pub enum Selection {
    None,
    One {
        /// `None` for a folder, or a size not read yet.
        size: Option<u64>,
        date: Option<String>,
        parent: String,
    },
    Many(u64),
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", format::grouped(n), if n == 1 { one } else { many })
}

/// The left side's parts: `683 items | Size: 87.2 KB | Date Modified: … |
/// Path: …`, or `683 items | 12 items selected`.
pub fn left_parts(total: u64, selection: Selection) -> Vec<String> {
    let mut parts = vec![count(total, "item", "items")];
    match selection {
        Selection::None => {}
        Selection::One { size, date, parent } => {
            if let Some(size) = size {
                parts.push(format!("Size: {}", format::size_auto(size)));
            }
            if let Some(date) = date {
                parts.push(format!("Date Modified: {date}"));
            }
            parts.push(format!("Path: {parent}"));
        }
        Selection::Many(n) => parts.push(count(n, "item selected", "items selected")),
    }
    parts
}

/// The size the right edge shows: the selection's when there is one, else
/// the whole result's. A selection of every row is the whole result; any
/// other is summed from the rows held, which may be only part of it.
pub fn total_size(
    results: u64,
    results_size: Option<u64>,
    selected: u64,
    selected_sizes: impl IntoIterator<Item = Option<u64>>,
) -> Option<u64> {
    if results == 0 {
        return None;
    }
    if selected == 0 || selected >= results {
        return results_size;
    }
    selected_sizes
        .into_iter()
        .flatten()
        .reduce(|a, b| a.saturating_add(b))
}

fn send(app: &App, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a query to this thread's list.
    unsafe { SendMessageW(app.list, msg, w, l) }
}

struct Part {
    text: String,
    colour: u32,
    indicator: Option<Indicator>,
}

/// Paints the status bar into `hdc` and notes where its indicators are.
pub fn paint(app: &App, hdc: HDC) {
    let rc = app.status_rc.get();
    app.indicators.borrow_mut().clear();
    if rc.bottom <= rc.top {
        return;
    }
    let pal = app.pal.get();
    // Options > View: one selected item's size, date and folder, and the
    // size at the right.
    let ui = app.ui();
    let sel_count = send(app, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
    let first_selected = if sel_count == 1 && ui.selected_in_status {
        app.selected(1).first().copied()
    } else {
        None
    };
    let total = app.model.borrow().total;
    let held = if ui.size_in_status && sel_count > 0 && sel_count < total {
        app.selected_held()
    } else {
        Vec::new()
    };

    let note = app.status_note();
    let mut left: Vec<String> = Vec::new();
    let mut right: Vec<Part> = Vec::new();
    {
        let m = app.model.borrow();
        let state = if m.incomplete && m.search.filelist.is_some() {
            Some("Loading file list…")
        } else if m.incomplete {
            Some("Indexing…")
        } else if m.metadata_pending {
            Some("Reading sizes and dates…")
        } else {
            None
        };
        if let Some(text) = state {
            right.push(Part {
                text: text.into(),
                colour: pal.dim,
                indicator: None,
            });
        }
        // The open file list's name (wave 4), as Everything shows it.
        if let Some(name) = app.filelist_indicator() {
            right.push(Part {
                text: name,
                colour: pal.fg,
                indicator: Some(Indicator::FileList),
            });
        }
        // The Folders pane's folder (F2), left of the options: Everything
        // shows its selection there.
        if let Some(folder) = app.folder_indicator() {
            right.push(Part {
                text: folder,
                colour: pal.fg,
                indicator: Some(Indicator::Folder),
            });
        }
        for (indicator, text) in app.indicators_on() {
            right.push(Part {
                text: text.into(),
                colour: pal.fg,
                indicator: Some(indicator),
            });
        }
        if let Some((_, text)) = &m.failure {
            left.push(text.clone());
        } else {
            let selection = match (sel_count, first_selected.and_then(|i| m.row(i))) {
                (0, _) => Selection::None,
                // Not shown (Options > View), or not loaded yet: counted.
                (1, None) if !ui.selected_in_status => Selection::Many(1),
                (1, None) => Selection::None,
                (1, Some(row)) => Selection::One {
                    size: row.size.filter(|_| !row.is_dir),
                    date: row.mtime.map(|ft| {
                        let mut d = Vec::new();
                        format::date(ft, &mut d);
                        String::from_utf16_lossy(&d)
                    }),
                    parent: String::from_utf16_lossy(row.parent()),
                },
                (n, _) => Selection::Many(n),
            };
            // A note (an action that took only part of the selection)
            // stands in for the counts while it lasts.
            left = match note {
                Some(note) => vec![note],
                None => left_parts(m.total, selection),
            };
            if let Some(why) = &m.partial_failure {
                left.push(format!("Some rows were not loaded: {why}"));
            }
            let sizes = held
                .iter()
                .filter_map(|&i| m.row(i))
                .map(|r| r.size.filter(|_| !r.is_dir));
            let whole = m.total_size.or_else(|| m.held_total_size());
            let size = if ui.size_in_status {
                total_size(m.total, whole, sel_count, sizes)
            } else {
                None
            };
            if let Some(size) = size {
                right.push(Part {
                    text: format::size_auto(size),
                    colour: pal.fg,
                    indicator: None,
                });
            }
        }
    }

    let fonts = app.fonts.borrow();
    let pad = app.scale(8);
    let mut found = Vec::new();
    fill(
        hdc,
        &RECT {
            bottom: rc.top + 1,
            ..rc
        },
        pal.line,
    );
    // SAFETY: draws into the paint buffer; the old font is put back.
    unsafe {
        let old = SelectObject(hdc, fonts.normal);
        SetBkMode(hdc, TRANSPARENT as i32);
        let y = rc.top + 1 + (rc.bottom - rc.top - 1 - fonts.height) / 2;
        let sep = |x: i32| {
            let r = RECT {
                left: x,
                right: x + 1,
                top: rc.top + app.scale(5),
                bottom: rc.bottom - app.scale(4),
            };
            fill(hdc, &r, pal.border);
        };
        let draw = |x: i32, clip: &RECT, text: &[u16], colour: u32| {
            SetTextColor(hdc, colour);
            ExtTextOutW(
                hdc,
                x,
                y,
                ETO_CLIPPED,
                clip,
                text.as_ptr(),
                text.len() as u32,
                core::ptr::null(),
            );
        };
        // Right side first, so the left side can stop short of it: the
        // view buttons at the end, then the parts right to left.
        let mut x_right = rc.right - pad;
        if let Some(buttons) = buttons_of(app) {
            paint_view_buttons(app, hdc, &buttons);
            x_right = buttons[0].1.left - pad;
        }
        for part in right.iter().rev() {
            let mut t: Vec<u16> = part.text.encode_utf16().collect();
            if matches!(
                part.indicator,
                Some(Indicator::Folder | Indicator::FileList)
            ) {
                // A long folder takes at most a third of the bar.
                t = elide_path(hdc, &t, (rc.right - rc.left) / 3);
            }
            let w = width(hdc, &t);
            x_right -= w;
            draw(x_right, &rc, &t, part.colour);
            if let Some(indicator) = part.indicator {
                let hit = RECT {
                    left: x_right - pad,
                    right: x_right + w + pad,
                    ..rc
                };
                found.push((indicator, hit));
            }
            x_right -= pad;
            sep(x_right);
            x_right -= pad;
        }
        let clip = RECT {
            right: x_right,
            ..rc
        };
        let mut x = rc.left + pad;
        for (i, text) in left.iter().enumerate() {
            if x >= clip.right {
                break;
            }
            if i > 0 {
                sep(x);
                x += pad;
            }
            let t: Vec<u16> = text.encode_utf16().collect();
            draw(x, &clip, &t, pal.fg);
            x += width(hdc, &t) + pad;
        }
        SelectObject(hdc, old);
    }
    *app.indicators.borrow_mut() = found;
}

/// The indicator at `pt`, in the main window's client coordinates.
pub fn indicator_at(app: &App, pt: POINT) -> Option<Indicator> {
    app.indicators
        .borrow()
        .iter()
        // SAFETY: reads two plain structs.
        .find(|(_, rc)| unsafe { PtInRect(rc, pt) } != 0)
        .map(|&(indicator, _)| indicator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_buttons_sit_at_the_right_end() {
        // A 23 px bar (its top line included) 1000 px wide, at 100 %.
        let bar = RECT {
            left: 0,
            top: 600,
            right: 1000,
            bottom: 623,
        };
        let [(d, details), (t, thumbs)] = view_buttons(&bar, 19, 2, 4);
        assert_eq!((d, t), (ViewButton::Details, ViewButton::Thumbnails));
        assert_eq!(
            (thumbs.left, thumbs.top, thumbs.right, thumbs.bottom),
            (977, 602, 996, 621)
        );
        assert_eq!((details.left, details.right), (956, 975));
        assert_eq!((details.top, details.bottom), (thumbs.top, thumbs.bottom));
        let buttons = view_buttons(&bar, 19, 2, 4);
        let at = |x, y| button_at(&buttons, POINT { x, y });
        assert_eq!(at(980, 610), Some(ViewButton::Thumbnails));
        assert_eq!(at(995, 620), Some(ViewButton::Thumbnails));
        assert_eq!(at(956, 602), Some(ViewButton::Details));
        // The gap between them, past the right edge, above and below.
        assert_eq!(at(976, 610), None);
        assert_eq!(at(996, 610), None);
        assert_eq!(at(960, 601), None);
        assert_eq!(at(960, 621), None);
        assert_eq!(at(500, 610), None);
    }

    #[test]
    fn counts_items() {
        assert_eq!(left_parts(1, Selection::None), ["1 item"]);
        assert_eq!(left_parts(0, Selection::None), ["0 items"]);
        assert_eq!(left_parts(683, Selection::None), ["683 items"]);
    }

    #[test]
    fn one_selected_file_shows_size_date_and_path() {
        let parts = left_parts(
            683,
            Selection::One {
                size: Some(89_293),
                date: Some("11/7/2026 4:00 PM".into()),
                parent: r"C:\Windows\System32".into(),
            },
        );
        assert_eq!(
            parts,
            [
                "683 items",
                "Size: 87.2 KB",
                "Date Modified: 11/7/2026 4:00 PM",
                r"Path: C:\Windows\System32",
            ]
        );
    }

    #[test]
    fn one_selected_folder_has_no_size() {
        let parts = left_parts(
            2,
            Selection::One {
                size: None,
                date: Some("d".into()),
                parent: r"C:\".into(),
            },
        );
        assert_eq!(parts, ["2 items", "Date Modified: d", r"Path: C:\"]);
    }

    #[test]
    fn several_selected() {
        assert_eq!(
            left_parts(683, Selection::Many(12)),
            ["683 items", "12 items selected"]
        );
    }

    #[test]
    fn total_size_follows_the_selection() {
        // Nothing selected: the whole result.
        assert_eq!(total_size(10, Some(5000), 0, []), Some(5000));
        // Everything selected: the whole result, not the rows held.
        assert_eq!(total_size(10, Some(5000), 10, [Some(1)]), Some(5000));
        // Some selected: their sizes, folders (None) adding nothing.
        assert_eq!(
            total_size(10, Some(5000), 3, [Some(100), None, Some(23)]),
            Some(123)
        );
        // Only folders selected: no size to show.
        assert_eq!(total_size(10, Some(5000), 2, [None, None]), None);
        // No results: nothing, even with a size from gozd.
        assert_eq!(total_size(0, Some(0), 0, []), None);
        // A daemon that does not sum: nothing for the whole result.
        assert_eq!(total_size(10, None, 0, []), None);
    }
}
