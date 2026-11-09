//! The results' two views: Details (the rows and columns) or thumbnails at
//! one of Everything 1.5's three sizes, with the INI keys Everything keeps
//! them under, and the geometry of a thumbnail's cell.

use windows_sys::Win32::Foundation::RECT;

/// Everything's `medium_thumbnail_size`, `large_thumbnail_size` and
/// `extra_large_thumbnail_size`, at 96 DPI.
pub const MEDIUM: u32 = 64;
pub const LARGE: u32 = 128;
pub const EXTRA_LARGE: u32 = 256;
/// Everything's `min_thumbnail_size` and `max_thumbnail_size`.
const MIN_SIZE: u32 = 32;
const MAX_SIZE: u32 = 256;

/// The space round a thumbnail inside its cell (`thumbnail_padding_*`) and
/// between cells (`thumbnail_min_margin_wide`, `_high`), at 96 DPI.
pub const PAD: i32 = 4;
const MARGIN_WIDE: i32 = 16;
const MARGIN_HIGH: i32 = 12;
/// The gap between a thumbnail and its name, at 96 DPI.
const TEXT_GAP: i32 = 2;
/// The name under a thumbnail takes at most this many lines.
pub const TEXT_LINES: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    /// Thumbnails rather than Details (`view=1`).
    pub thumbnails: bool,
    /// The thumbnails' size at 96 DPI (`thumbnail_size`): the size shown,
    /// or in Details the one the status bar's Thumbnails button goes back
    /// to (`statusbar_view_thumbnail_size`).
    pub size: u32,
}

impl Default for View {
    fn default() -> View {
        View {
            thumbnails: false,
            size: LARGE,
        }
    }
}

impl View {
    pub fn details(self) -> View {
        View {
            thumbnails: false,
            ..self
        }
    }

    pub fn thumbnails(size: u32) -> View {
        View {
            thumbnails: true,
            size: clamp(size),
        }
    }

    /// The view `get` holds under Everything's keys; what is missing or
    /// unreadable is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> View {
        let number = |key: &str| get(key).and_then(|v| v.trim().parse::<u32>().ok());
        let thumbnails = number("view") == Some(1);
        // The size on screen while thumbnails show; otherwise the one the
        // status bar's button would show them at.
        let keys = if thumbnails {
            ["thumbnail_size", "statusbar_view_thumbnail_size"]
        } else {
            ["statusbar_view_thumbnail_size", "thumbnail_size"]
        };
        let size = keys.iter().find_map(|k| number(k)).map_or(LARGE, clamp);
        View { thumbnails, size }
    }

    pub fn to_keys(self) -> Vec<(String, String)> {
        vec![
            ("view".into(), u8::from(self.thumbnails).to_string()),
            ("thumbnail_size".into(), self.size.to_string()),
            (
                "statusbar_view_thumbnail_size".into(),
                self.size.to_string(),
            ),
        ]
    }
}

fn clamp(size: u32) -> u32 {
    size.clamp(MIN_SIZE, MAX_SIZE)
}

fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

/// A thumbnail's side in pixels at `dpi`.
pub fn side(size: u32, dpi: u32) -> i32 {
    scale(size as i32, dpi)
}

/// The list's icon spacing (`LVM_SETICONSPACING`): from one cell's left
/// edge to the next one's, and from one row's top to the next one's. A
/// cell is the thumbnail with its padding and up to [`TEXT_LINES`] of
/// name (`line` pixels each) under it, and the margins go between cells.
pub fn spacing(size: u32, dpi: u32, line: i32) -> (i32, i32) {
    let s = side(size, dpi);
    let pad = scale(PAD, dpi);
    (
        s + 2 * pad + scale(MARGIN_WIDE, dpi),
        s + 2 * pad + scale(TEXT_GAP, dpi) + TEXT_LINES * line + scale(MARGIN_HIGH, dpi),
    )
}

/// Where a cell's parts go.
#[derive(Clone, Copy)]
pub struct Cell {
    /// The whole cell, which a selection fills.
    pub cell: RECT,
    /// The square the thumbnail is fitted into.
    pub thumb: RECT,
    /// The name's lines.
    pub text: RECT,
}

/// The cell round the icon rectangle the list gives an item (`icon`,
/// which the list centres in its slot): the thumbnail's square centred on
/// it, the name under it, and the cell nearly as wide as the slot
/// (`spacing_x`), so the name has room.
pub fn cell(icon: &RECT, size: u32, dpi: u32, line: i32, spacing_x: i32) -> Cell {
    let s = side(size, dpi);
    let pad = scale(PAD, dpi);
    let mid = (icon.left + icon.right) / 2;
    let top = icon.top + ((icon.bottom - icon.top) - s).max(0) / 2;
    let thumb = RECT {
        left: mid - s / 2,
        top,
        right: mid - s / 2 + s,
        bottom: top + s,
    };
    let width = (spacing_x - scale(MARGIN_WIDE, dpi) / 2).max(s + 2 * pad);
    let left = mid - width / 2;
    let text_top = thumb.bottom + pad + scale(TEXT_GAP, dpi);
    let text = RECT {
        left: left + pad,
        top: text_top,
        right: left + width - pad,
        bottom: text_top + TEXT_LINES * line,
    };
    Cell {
        cell: RECT {
            left,
            top: top - pad,
            right: left + width,
            bottom: text.bottom + pad,
        },
        thumb,
        text,
    }
}

/// An image of `w` × `h` fitted into a square of side `s`: shrunk to fit,
/// keeping its shape, never enlarged; centred in the square.
pub fn fit(w: i32, h: i32, square: &RECT) -> RECT {
    let s = (square.right - square.left).min(square.bottom - square.top);
    let (w, h) = (w.max(1), h.max(1));
    let (fw, fh) = if w <= s && h <= s {
        (w, h)
    } else if w >= h {
        (s, (h as i64 * s as i64 / w as i64).max(1) as i32)
    } else {
        ((w as i64 * s as i64 / h as i64).max(1) as i32, s)
    };
    let left = square.left + (square.right - square.left - fw) / 2;
    let top = square.top + (square.bottom - square.top - fh) / 2;
    RECT {
        left,
        top,
        right: left + fw,
        bottom: top + fh,
    }
}

/// Where a name too long for one line of `fit` characters breaks: after
/// the last space, dash, underscore or dot in the second half of the
/// line, so words stay whole where they can; otherwise after `fit`.
pub fn line_break(name: &[u16], fit: usize) -> usize {
    let fit = fit.clamp(1, name.len().max(1));
    if fit >= name.len() {
        return name.len();
    }
    let soft = |c: u16| matches!(c, 0x20 | 0x2d | 0x5f | 0x2e);
    (fit / 2..fit)
        .rev()
        .find(|&i| i > 0 && soft(name[i - 1]))
        .unwrap_or(fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn sides(r: RECT) -> (i32, i32, i32, i32) {
        (r.left, r.top, r.right, r.bottom)
    }

    fn keys(view: View) -> Vec<(String, String)> {
        view.to_keys()
    }

    #[test]
    fn view_keys_read_back_the_same() {
        for view in [
            View::default(),
            View::thumbnails(MEDIUM),
            View::thumbnails(EXTRA_LARGE),
            View::thumbnails(MEDIUM).details(),
        ] {
            let pairs = keys(view);
            let get = |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
            };
            assert_eq!(View::from_keys(get), view);
        }
    }

    #[test]
    fn everythings_keys_and_defaults() {
        // Everything's defaults: Details, the button going to Large.
        let ini = [
            ("view", "0"),
            ("thumbnail_size", "128"),
            ("statusbar_view_thumbnail_size", "128"),
        ];
        let get = |k: &str| ini.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        assert_eq!(View::from_keys(get), View::default());
        // Nothing in the file: the same.
        assert_eq!(View::from_keys(|_| None), View::default());
        // Thumbnails on: their size is `thumbnail_size`.
        let ini = [
            ("view", "1"),
            ("thumbnail_size", "256"),
            ("statusbar_view_thumbnail_size", "64"),
        ];
        let get = |k: &str| ini.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        assert_eq!(View::from_keys(get), View::thumbnails(EXTRA_LARGE));
        // Out of range or unreadable: clamped, or the default.
        let ini = [("view", "1"), ("thumbnail_size", "9999")];
        let get = |k: &str| ini.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        assert_eq!(View::from_keys(get).size, 256);
        let ini = [("view", "x"), ("thumbnail_size", "big")];
        let get = |k: &str| ini.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        assert_eq!(View::from_keys(get), View::default());
    }

    #[test]
    fn spacing_holds_the_cell_and_scales() {
        // Large at 100 %, 15 px lines: 128 + 8 + 16 wide; 128 + 8 + 2 +
        // 30 + 12 high.
        assert_eq!(spacing(LARGE, 96, 15), (152, 180));
        assert_eq!(spacing(MEDIUM, 96, 15), (88, 116));
        assert_eq!(spacing(EXTRA_LARGE, 96, 15), (280, 308));
        // 150 %: everything half as big again, the text as measured.
        assert_eq!(spacing(LARGE, 144, 23), (228, 6 + 192 + 3 + 46 + 18 + 6));
    }

    #[test]
    fn the_cell_sits_round_the_icon() {
        let (sx, _) = spacing(LARGE, 96, 15);
        // The list's icon rectangle: the thumbnail's size and a margin.
        let icon = rect(100, 50, 232, 182);
        let c = cell(&icon, LARGE, 96, 15, sx);
        assert_eq!(sides(c.thumb), (102, 52, 230, 180));
        // 152 - 8 wide, centred on the icon.
        assert_eq!(c.cell.right - c.cell.left, 144);
        assert_eq!((c.cell.left + c.cell.right) / 2, 166);
        assert_eq!(c.cell.top, 48);
        assert_eq!(c.text.top, 180 + 4 + 2);
        assert_eq!(c.text.bottom - c.text.top, 30);
        assert_eq!(c.cell.bottom, c.text.bottom + 4);
        assert!(c.text.left >= c.cell.left && c.text.right <= c.cell.right);
        // The next row's cell starts below this one's.
        let (_, sy) = spacing(LARGE, 96, 15);
        assert!(c.cell.bottom - c.cell.top <= sy);
    }

    #[test]
    fn images_fit_without_growing() {
        let sq = rect(0, 0, 128, 128);
        // Small: as it is, centred.
        assert_eq!(sides(fit(32, 32, &sq)), (48, 48, 80, 80));
        // Wide: as wide as the square, its shape kept.
        assert_eq!(sides(fit(256, 128, &sq)), (0, 32, 128, 96));
        // Tall.
        assert_eq!(sides(fit(100, 400, &sq)), (48, 0, 80, 128));
        // Degenerate sizes stay at least a pixel.
        assert_eq!(fit(1000, 1, &sq).bottom - fit(1000, 1, &sq).top, 1);
    }

    #[test]
    fn names_break_between_words() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        // Fits: one line.
        assert_eq!(line_break(&w("short.txt"), 20), 9);
        // After the last dot or dash in the line's second half.
        assert_eq!(line_break(&w("holiday-photos.jpeg"), 16), 15);
        assert_eq!(line_break(&w("img0_1920x1200.jpg"), 8), 5);
        // No break in the second half: as many as fit.
        assert_eq!(line_break(&w("abcdefghijklmnop"), 10), 10);
        assert_eq!(line_break(&w("a.bcdefghijklmnop"), 10), 10);
        // At least one character a line.
        assert_eq!(line_break(&w("abc"), 0), 1);
    }
}
