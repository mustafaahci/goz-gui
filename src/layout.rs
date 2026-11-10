//! The main window's regions, top to bottom: the menu bar (Windows draws it
//! above the client area), the tab strip, the search row (the nav bar, the
//! search frame, the filter bar), the main area, and the status bar. The
//! main area is the left dock and its divider, the centre column (the
//! results, with the find bar under them, as in Everything 1.5), and the
//! right dock's divider and the right dock. A region not shown takes no
//! room. Plain arithmetic, so it is tested without a window.

use windows_sys::Win32::Foundation::RECT;

/// What the regions need, in pixels at the window's DPI.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    /// The client area's size.
    pub width: i32,
    pub height: i32,
    /// The tab strip's height (0: none).
    pub tabs: i32,
    /// The search row: the room above the search frame, the frame's
    /// height, and the room below it.
    pub search: (i32, i32, i32),
    /// The status bar's height (0: hidden).
    pub status: i32,
    /// The find bar's height under the results (0: none).
    pub find: i32,
    /// The docks' widths as wanted (0: no dock on that side).
    pub left: i32,
    pub right: i32,
    /// A dock's divider, a dock's least width (`pane_min_wide`), and the
    /// least the results keep while the docks are as wide as wanted.
    pub divider: i32,
    pub min_dock: i32,
    pub min_center: i32,
}

/// Where each region goes, in client coordinates. A region not shown is
/// empty (no width or no height).
#[derive(Clone, Copy, Default)]
pub struct Regions {
    pub tabs: RECT,
    /// The search row, edge to edge.
    pub search: RECT,
    /// The docks, the results and the find bar.
    pub main: RECT,
    pub left: RECT,
    pub left_divider: RECT,
    /// The results, and the find bar under them.
    pub list: RECT,
    pub find: RECT,
    pub right_divider: RECT,
    pub right: RECT,
    pub status: RECT,
}

/// The docks' widths in a window `width` wide: as wanted while the results
/// keep `min_center`; else both shrink in proportion, but not below
/// `min_dock`; in a window too narrow even for that, the right dock and
/// then the left give way so nothing reaches past the window. A dock not
/// wanted (0) stays 0, and so does its divider.
pub fn fit_docks(
    width: i32,
    (left, right): (i32, i32),
    divider: i32,
    min_dock: i32,
    min_center: i32,
) -> (i32, i32) {
    let (left, right) = (left.max(0), right.max(0));
    let dl = if left > 0 { divider } else { 0 };
    let dr = if right > 0 { divider } else { 0 };
    let room = (width - min_center - dl - dr).max(0);
    let want = left + right;
    let (mut l, mut r) = if want <= room {
        (left, right)
    } else {
        let l = (i64::from(left) * i64::from(room) / i64::from(want.max(1))) as i32;
        (l, if right > 0 { room - l } else { 0 })
    };
    if left > 0 {
        l = l.max(min_dock);
    }
    if right > 0 {
        r = r.max(min_dock);
    }
    let over = l + r + dl + dr - width.max(0);
    if over > 0 {
        let take = over.min(r);
        r -= take;
        l -= (over - take).min(l);
    }
    (l, r)
}

/// The regions for `i`.
pub fn regions(i: &Inputs) -> Regions {
    let (w, h) = (i.width.max(0), i.height.max(0));
    let row = |top: i32, bottom: i32| RECT {
        left: 0,
        top,
        right: w,
        bottom,
    };
    let tabs = row(0, i.tabs.clamp(0, h));
    let (above, frame, below) = i.search;
    let search = row(tabs.bottom, (tabs.bottom + above + frame + below).min(h));
    let status = row((h - i.status.max(0)).max(0), h);
    let main = row(search.bottom, status.top.max(search.bottom));
    let (l, r) = fit_docks(w, (i.left, i.right), i.divider, i.min_dock, i.min_center);
    let dl = if l > 0 { i.divider } else { 0 };
    let dr = if r > 0 { i.divider } else { 0 };
    let col = |left: i32, right: i32| RECT {
        left,
        top: main.top,
        right: right.max(left),
        bottom: main.bottom,
    };
    let left = col(0, l);
    let left_divider = col(l, l + dl);
    let right = col(w - r, w);
    let right_divider = col(w - r - dr, w - r);
    let center = col(left_divider.right, right_divider.left);
    let find_h = i.find.clamp(0, center.bottom - center.top);
    let find = RECT {
        top: center.bottom - find_h,
        ..center
    };
    let list = RECT {
        bottom: find.top,
        ..center
    };
    Regions {
        tabs,
        search,
        main,
        left,
        left_divider,
        list,
        find,
        right_divider,
        right,
        status,
    }
}

/// Whether `rc` has no area.
pub fn is_empty(rc: &RECT) -> bool {
    rc.right <= rc.left || rc.bottom <= rc.top
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sides(r: RECT) -> (i32, i32, i32, i32) {
        (r.left, r.top, r.right, r.bottom)
    }

    /// Everything 1.5's window at 100 %: 1384 × 811 client, a 23 px status
    /// bar, goz-gui's search row (5 + 23 + 3).
    fn window() -> Inputs {
        Inputs {
            width: 1384,
            height: 811,
            tabs: 0,
            search: (5, 23, 3),
            status: 23,
            find: 0,
            left: 0,
            right: 0,
            divider: 6,
            min_dock: 32,
            min_center: 64,
        }
    }

    #[test]
    fn without_docks_the_list_takes_the_main_area() {
        let r = regions(&window());
        assert_eq!(sides(r.search), (0, 0, 1384, 31));
        assert_eq!(sides(r.main), (0, 31, 1384, 788));
        assert_eq!(sides(r.list), (0, 31, 1384, 788));
        assert_eq!(sides(r.status), (0, 788, 1384, 811));
        for empty in [
            r.tabs,
            r.left,
            r.left_divider,
            r.right,
            r.right_divider,
            r.find,
        ] {
            assert!(is_empty(&empty), "{:?}", sides(empty));
        }
    }

    #[test]
    fn docks_dividers_and_the_find_bar_as_everything_places_them() {
        // Everything's window with its tab strip (29 px), both docks (256
        // px, 6 px dividers) and the find bar (30 px) under the results.
        let r = regions(&Inputs {
            tabs: 29,
            left: 256,
            right: 256,
            find: 30,
            ..window()
        });
        assert_eq!(sides(r.tabs), (0, 0, 1384, 29));
        assert_eq!(sides(r.search), (0, 29, 1384, 60));
        assert_eq!(sides(r.left), (0, 60, 256, 788));
        assert_eq!(sides(r.left_divider), (256, 60, 262, 788));
        assert_eq!(sides(r.list), (262, 60, 1122, 758));
        assert_eq!(sides(r.find), (262, 758, 1122, 788));
        assert_eq!(sides(r.right_divider), (1122, 60, 1128, 788));
        assert_eq!(sides(r.right), (1128, 60, 1384, 788));
        // The status bar spans the window, under the docks.
        assert_eq!(sides(r.status), (0, 788, 1384, 811));
    }

    #[test]
    fn hidden_regions_take_no_room() {
        let r = regions(&Inputs {
            status: 0,
            left: 200,
            ..window()
        });
        assert_eq!(sides(r.status), (0, 811, 1384, 811));
        assert_eq!(r.main.bottom, 811);
        assert!(is_empty(&r.right_divider) && is_empty(&r.right));
        assert_eq!(r.list.right, 1384);
        assert_eq!(r.list.left, 206);
    }

    #[test]
    fn docks_shrink_in_proportion_when_the_window_is_narrow() {
        // Room for both: as wanted.
        assert_eq!(fit_docks(1000, (256, 256), 6, 32, 64), (256, 256));
        // 600 px: 600 - 64 - 12 = 524 for 512 wanted: still as wanted.
        assert_eq!(fit_docks(600, (256, 256), 6, 32, 64), (256, 256));
        // 400 px: 324 shared in proportion (300 : 100).
        assert_eq!(fit_docks(400, (300, 100), 6, 32, 64), (243, 81));
        // One dock: it alone gives way.
        assert_eq!(fit_docks(300, (256, 0), 6, 32, 64), (230, 0));
        assert_eq!(fit_docks(300, (0, 256), 6, 32, 64), (0, 230));
        // Never below the least width while the window has room for it.
        assert_eq!(fit_docks(150, (256, 256), 6, 32, 64), (37, 37));
        assert_eq!(fit_docks(120, (256, 256), 6, 32, 64), (32, 32));
        // Narrower than two docks at their least: the right gives way
        // first, nothing reaches past the window.
        assert_eq!(fit_docks(60, (256, 256), 6, 32, 64), (32, 16));
        assert_eq!(fit_docks(20, (256, 256), 6, 32, 64), (8, 0));
        assert_eq!(fit_docks(0, (256, 256), 6, 32, 64), (0, 0));
        // No docks: nothing.
        assert_eq!(fit_docks(800, (0, 0), 6, 32, 64), (0, 0));
    }

    #[test]
    fn a_short_window_never_turns_a_region_inside_out() {
        let r = regions(&Inputs {
            height: 20,
            tabs: 29,
            find: 30,
            left: 256,
            ..window()
        });
        for rc in [r.tabs, r.search, r.main, r.left, r.list, r.find, r.status] {
            assert!(
                rc.right >= rc.left && rc.bottom >= rc.top,
                "{:?}",
                sides(rc)
            );
            assert!(rc.top >= 0 && rc.bottom <= 20, "{:?}", sides(rc));
        }
    }
}
