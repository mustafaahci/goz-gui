//! Where the window opens: the rectangle it had when it last closed, on
//! the monitor it was on, resized when that monitor's DPI is not the one it
//! was saved at, and moved into the nearest work area when that monitor is
//! gone or the title bar would be out of reach.

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromRect,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
};
use windows_sys::core::BOOL;

use crate::settings::WindowPos;

/// A rectangle as left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w
    }

    fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// The area this rectangle shares with `other`.
    fn overlap(&self, other: &Rect) -> i64 {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0);
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0);
        i64::from(w) * i64::from(h)
    }

    /// The square of the distance between the two rectangles (0 if they
    /// touch or overlap).
    fn distance(&self, other: &Rect) -> i64 {
        let dx = i64::from((other.x - self.right()).max(self.x - other.right()).max(0));
        let dy = i64::from(
            (other.y - self.bottom())
                .max(self.y - other.bottom())
                .max(0),
        );
        dx * dx + dy * dy
    }

    pub fn from_win(rc: &RECT) -> Rect {
        Rect {
            x: rc.left,
            y: rc.top,
            w: rc.right - rc.left,
            h: rc.bottom - rc.top,
        }
    }

    pub fn to_win(self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.right(),
            bottom: self.bottom(),
        }
    }
}

/// A monitor: its whole area, its work area (without the taskbar), and
/// its DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub area: Rect,
    pub work: Rect,
    pub dpi: u32,
}

/// The monitor `rect` covers most, as Windows picks one for a window;
/// `None` when it is on none.
pub fn monitor_of(rect: &Rect, monitors: &[Monitor]) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| (rect.overlap(&m.area), i))
        .filter(|&(area, _)| area > 0)
        .max_by_key(|&(area, i)| (area, core::cmp::Reverse(i)))
        .map(|(_, i)| i)
}

/// Where a window saved as `saved` opens among `monitors`, in screen
/// pixels. `None` when there is no monitor at all.
///
/// The monitor is the one the saved rectangle covers most, else the
/// nearest. Its size is scaled from the DPI it was saved at to that
/// monitor's, keeping the top-left corner. It is then moved (and shrunk if
/// need be) into the work area when the monitor it was on is gone (less
/// than half of it is on the monitors there are now: most of it was on one
/// unplugged since), or when less than `grip` pixels of its top edge (the
/// title bar) are inside it.
pub fn fit(saved: &WindowPos, monitors: &[Monitor]) -> Option<Rect> {
    let rect = Rect {
        x: saved.x,
        y: saved.y,
        w: saved.width,
        h: saved.height,
    };
    let covered = monitor_of(&rect, monitors).map(|i| &monitors[i]);
    let on_screen: i64 = monitors.iter().map(|m| rect.overlap(&m.area)).sum();
    let gone = covered.is_none() || on_screen * 2 < i64::from(rect.w) * i64::from(rect.h);
    let mon = covered.or_else(|| monitors.iter().min_by_key(|m| rect.distance(&m.area)))?;
    let mut r = rect;
    if let Some(dpi) = saved.dpi.filter(|&d| d != mon.dpi) {
        let scale = |v: i32| (i64::from(v) * i64::from(mon.dpi) / i64::from(dpi.max(1))) as i32;
        r.w = scale(r.w).max(1);
        r.h = scale(r.h).max(1);
    }
    let work = mon.work;
    let grip = (48 * mon.dpi / 96) as i32;
    let caption = Rect {
        h: (32 * mon.dpi / 96) as i32,
        ..r
    };
    let reachable =
        r.y >= work.y && caption.overlap(&work) >= i64::from(grip.min(r.w)) * i64::from(caption.h);
    if gone || !reachable {
        r.w = r.w.min(work.w);
        r.h = r.h.min(work.h);
        r.x = r.x.clamp(work.x, work.right() - r.w);
        r.y = r.y.clamp(work.y, work.bottom() - r.h);
    }
    Some(r)
}

/// The monitors as Windows has them now.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec passed to EnumDisplayMonitors below,
        // which outlives the enumeration.
        let out = unsafe { &mut *(data as *mut Vec<Monitor>) };
        if let Some(m) = monitor(mon) {
            out.push(m);
        }
        1
    }
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only pushes to `out`, alive across the call.
    unsafe {
        EnumDisplayMonitors(
            core::ptr::null_mut(),
            core::ptr::null(),
            Some(each),
            (&raw mut out) as LPARAM,
        );
    }
    out
}

fn monitor(mon: HMONITOR) -> Option<Monitor> {
    // SAFETY: plain out-structs for a monitor handle Windows gave.
    unsafe {
        let mut info: MONITORINFO = core::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut info) == 0 {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) < 0 {
            dx = 96;
        }
        Some(Monitor {
            area: Rect::from_win(&info.rcMonitor),
            work: Rect::from_win(&info.rcWork),
            dpi: dx,
        })
    }
}

/// `r` (screen pixels) in workspace coordinates, which WINDOWPLACEMENT
/// holds: relative to the work area of the monitor the window is on, so a
/// taskbar at the left or top shifts them. As it is when on no monitor.
pub fn screen_to_workspace(r: Rect, monitors: &[Monitor]) -> Rect {
    match monitor_of(&r, monitors) {
        Some(i) => {
            let m = &monitors[i];
            Rect {
                x: r.x - (m.work.x - m.area.x),
                y: r.y - (m.work.y - m.area.y),
                ..r
            }
        }
        None => r,
    }
}

/// The DPI of the monitor `rc` (screen pixels) is mostly on.
pub fn dpi_at(rc: &RECT) -> u32 {
    // SAFETY: MonitorFromRect never fails with DEFAULTTONEAREST.
    let mon = unsafe { MonitorFromRect(rc, MONITOR_DEFAULTTONEAREST) };
    monitor(mon).map_or(96, |m| m.dpi)
}

/// `ws` in workspace coordinates (GetWindowPlacement's: relative to the
/// work area of the monitor the window is on) as screen coordinates. The
/// monitor is the one whose offset puts the window mostly on itself.
pub fn workspace_to_screen(ws: Rect, monitors: &[Monitor]) -> Rect {
    for (i, m) in monitors.iter().enumerate() {
        let screen = Rect {
            x: ws.x + m.work.x - m.area.x,
            y: ws.y + m.work.y - m.area.y,
            ..ws
        };
        if monitor_of(&screen, monitors) == Some(i) {
            return screen;
        }
    }
    ws
}

/// A top-level window's normal (not minimized or maximized) rectangle in
/// screen pixels, with the DPI there, and whether it is maximized (or
/// minimized from maximized). `None` when it has no placement or no size.
pub fn normal_place(hwnd: HWND) -> Option<(WindowPos, bool)> {
    let mut wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        // SAFETY: the rest of WINDOWPLACEMENT is plain data.
        ..unsafe { core::mem::zeroed() }
    };
    // SAFETY: reads a window's placement into `wp`.
    if unsafe { GetWindowPlacement(hwnd, &mut wp) } == 0 {
        return None;
    }
    let r = workspace_to_screen(Rect::from_win(&wp.rcNormalPosition), &monitors());
    if r.w <= 0 || r.h <= 0 {
        return None;
    }
    let show = wp.showCmd as i32;
    let maximized = show == SW_SHOWMAXIMIZED
        || (show == SW_SHOWMINIMIZED && wp.flags & WPF_RESTORETOMAXIMIZED != 0);
    let pos = WindowPos {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
        dpi: Some(dpi_at(&r.to_win())),
    };
    Some((pos, maximized))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn pos(x: i32, y: i32, w: i32, h: i32, dpi: Option<u32>) -> WindowPos {
        WindowPos {
            x,
            y,
            width: w,
            height: h,
            dpi,
        }
    }

    #[test]
    fn screen_to_workspace_coordinates() {
        // A taskbar on the left of the primary, 60 px wide; another at the
        // top of a second monitor, 40 px tall.
        let monitors = [
            Monitor {
                area: r(0, 0, 1920, 1080),
                work: r(60, 0, 1860, 1080),
                dpi: 96,
            },
            Monitor {
                area: r(1920, 0, 1920, 1080),
                work: r(1920, 40, 1920, 1040),
                dpi: 96,
            },
        ];
        assert_eq!(
            screen_to_workspace(r(160, 50, 800, 600), &monitors),
            r(100, 50, 800, 600)
        );
        assert_eq!(
            screen_to_workspace(r(2000, 90, 800, 600), &monitors),
            r(2000, 50, 800, 600)
        );
        assert_eq!(
            screen_to_workspace(r(9000, 50, 800, 600), &monitors),
            r(9000, 50, 800, 600)
        );
        assert_eq!(Rect::from_win(&r(-5, 7, 30, 40).to_win()), r(-5, 7, 30, 40));
    }

    #[test]
    fn workspace_coordinates_to_screen() {
        // A taskbar on the left of the primary monitor, 60 px wide; a
        // second monitor to the right with none.
        let monitors = [
            Monitor {
                area: r(0, 0, 1920, 1080),
                work: r(60, 0, 1860, 1080),
                dpi: 96,
            },
            Monitor {
                area: r(1920, 0, 1920, 1080),
                work: r(1920, 0, 1920, 1080),
                dpi: 96,
            },
        ];
        assert_eq!(
            workspace_to_screen(r(100, 50, 800, 600), &monitors),
            r(160, 50, 800, 600)
        );
        assert_eq!(
            workspace_to_screen(r(2000, 50, 800, 600), &monitors),
            r(2000, 50, 800, 600)
        );
        // Nowhere: as it is.
        assert_eq!(
            workspace_to_screen(r(9000, 50, 800, 600), &monitors),
            r(9000, 50, 800, 600)
        );
    }

    /// A 1920×1080 primary at 100 % with a taskbar at the bottom, and a
    /// 2560×1440 monitor at 150 % to its left.
    fn two() -> Vec<Monitor> {
        vec![
            Monitor {
                area: r(0, 0, 1920, 1080),
                work: r(0, 0, 1920, 1032),
                dpi: 96,
            },
            Monitor {
                area: r(-2560, 0, 2560, 1440),
                work: r(-2560, 0, 2560, 1380),
                dpi: 144,
            },
        ]
    }

    #[test]
    fn a_window_comes_back_where_it_was() {
        let m = two();
        assert_eq!(
            fit(&pos(200, 150, 1400, 850, Some(96)), &m),
            Some(r(200, 150, 1400, 850))
        );
        assert_eq!(
            fit(&pos(-2000, 100, 1500, 900, Some(144)), &m),
            Some(r(-2000, 100, 1500, 900))
        );
        // Partly off the right edge on purpose (more than half on screen):
        // left there.
        assert_eq!(
            fit(&pos(1400, 100, 1000, 600, Some(96)), &m),
            Some(r(1400, 100, 1000, 600))
        );
        // No DPI saved (Everything's file): the size as it is.
        assert_eq!(
            fit(&pos(-2000, 100, 1000, 600, None), &m),
            Some(r(-2000, 100, 1000, 600))
        );
    }

    #[test]
    fn a_dpi_change_rescales_the_size() {
        let m = two();
        // Saved at 96 on what is now the 150 % monitor.
        assert_eq!(
            fit(&pos(-2400, 50, 1000, 600, Some(96)), &m),
            Some(r(-2400, 50, 1500, 900))
        );
        // Saved at 150 % on what is now 100 %.
        assert_eq!(
            fit(&pos(100, 100, 1500, 900, Some(144)), &m),
            Some(r(100, 100, 1000, 600))
        );
    }

    #[test]
    fn a_window_on_a_monitor_now_gone_moves_to_the_nearest_work_area() {
        let one = vec![two()[0]];
        // It was on the left monitor, which is unplugged.
        assert_eq!(
            fit(&pos(-2000, 100, 1500, 900, Some(144)), &one),
            Some(r(0, 100, 1000, 600))
        );
        // Bigger than the work area: shrunk to it.
        assert_eq!(
            fit(&pos(3000, 2000, 4000, 3000, Some(96)), &one),
            Some(r(0, 0, 1920, 1032))
        );
    }

    #[test]
    fn a_window_mostly_on_a_monitor_now_gone_comes_back_whole() {
        // The user's: a 2560 primary (taskbar 48 px) and a 1920 to its
        // left; the window was on a monitor to the right, now unplugged.
        let m = vec![
            Monitor {
                area: r(0, 0, 2560, 1440),
                work: r(0, 0, 2560, 1392),
                dpi: 96,
            },
            Monitor {
                area: r(-1920, 0, 1920, 1080),
                work: r(-1920, 0, 1920, 1032),
                dpi: 96,
            },
        ];
        // 60 of its 1400 px on the primary: moved into its work area.
        assert_eq!(
            fit(&pos(2500, 300, 1400, 900, Some(96)), &m),
            Some(r(1160, 300, 1400, 900))
        );
        assert_eq!(
            fit(&pos(2400, 200, 1200, 800, Some(96)), &m),
            Some(r(1360, 200, 1200, 800))
        );
        // Below the left monitor (which is 1080 tall), 200 px of its width
        // on the primary: moved up onto the primary, which it covers most.
        assert_eq!(
            fit(&pos(-1000, 1100, 1200, 500, Some(96)), &m),
            Some(r(0, 892, 1200, 500))
        );
        // Over both monitors, all of it on screen: left there.
        assert_eq!(
            fit(&pos(-600, 100, 1400, 900, Some(96)), &m),
            Some(r(-600, 100, 1400, 900))
        );
    }

    #[test]
    fn a_title_bar_out_of_reach_is_brought_back() {
        let m = two();
        // Above the top of the screen.
        assert_eq!(
            fit(&pos(100, -500, 800, 600, Some(96)), &m),
            Some(r(100, 0, 800, 600))
        );
        // Only 20 px of it on the right edge.
        assert_eq!(
            fit(&pos(1900, 100, 800, 600, Some(96)), &m),
            Some(r(1120, 100, 800, 600))
        );
        // Under the taskbar.
        assert_eq!(
            fit(&pos(100, 1040, 800, 600, Some(96)), &m),
            Some(r(100, 432, 800, 600))
        );
        assert_eq!(fit(&pos(0, 0, 10, 10, None), &[]), None);
    }
}
