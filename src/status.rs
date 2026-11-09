//! The status bar: the result count and the selection on the left; from the
//! right edge, the total size, the search options in force, and gozd's own
//! state. Double-clicking an option's label turns that option off.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    ETO_CLIPPED, ExtTextOutW, HDC, PtInRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::LVM_GETSELECTEDCOUNT;
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

use crate::format;
use crate::paint::{fill, width};
use crate::window::App;

/// A search option the status bar shows while it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indicator {
    Case,
    Path,
    Filter,
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
    let sel_count = send(app, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
    let first_selected = if sel_count == 1 {
        app.selected(1).first().copied()
    } else {
        None
    };
    let total = app.model.borrow().total;
    let held = if sel_count > 0 && sel_count < total {
        app.selected_held()
    } else {
        Vec::new()
    };

    let note = app.status_note();
    let mut left: Vec<String> = Vec::new();
    let mut right: Vec<Part> = Vec::new();
    {
        let m = app.model.borrow();
        let state = if m.incomplete {
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
                (0, _) | (1, None) => Selection::None,
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
            if let Some(size) = total_size(m.total, whole, sel_count, sizes) {
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
        // Right side first, so the left side can stop short of it.
        let mut x_right = rc.right - pad;
        for part in right.iter().rev() {
            let t: Vec<u16> = part.text.encode_utf16().collect();
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
