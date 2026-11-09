//! View > Filter Bar, View > Zoom, and Search > Next and Previous Filter
//! (Ctrl+] and Ctrl+[).

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, InvalidateRect, MapWindowPoints};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyMenu, GetWindowRect, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER,
    WHEEL_DELTA, WINDOWPOS, WM_SETFONT, WM_WINDOWPOSCHANGED,
};

use super::{App, Fonts, send};
use crate::filter::Filter;
use crate::{menu, zoom};

/// Everything's `filters_wide`: the filter bar's width at 100 %.
pub(super) const FILTER_BAR_WIDE: i32 = 128;

impl App {
    pub(super) fn filter_bar_visible(&self) -> bool {
        self.filter_bar.get()
    }

    /// Shows or hides the filter bar; the search frame gives it room.
    pub(super) fn set_filter_bar_visible(&self, visible: bool) {
        self.filter_bar.set(visible);
        self.layout();
        self.settings_changed();
    }

    /// The filter bar, while it shows: where it is, the filter's name, and
    /// whether its list is open.
    pub fn filter_bar(&self) -> Option<(RECT, &'static str, bool)> {
        let rc = self.filter_rc.get();
        (rc.right > rc.left).then(|| (rc, self.opts.get().filter.title(), self.filter_open.get()))
    }

    pub(super) fn invalidate_filter_bar(&self) {
        let rc = self.filter_rc.get();
        if rc.right > rc.left {
            // SAFETY: repaints part of this thread's window.
            unsafe { InvalidateRect(self.hwnd, &rc, 0) };
        }
    }

    /// A click on the filter bar: its list opens under it, and the filter
    /// chosen applies as the Search menu's does.
    pub(super) fn filter_bar_click(&self) {
        let rc = self.filter_rc.get();
        let popup = menu::filter_menu(self.opts.get().filter);
        let mut pt = POINT {
            x: rc.left,
            y: rc.bottom,
        };
        // SAFETY: converts a point of this thread's window.
        unsafe { ClientToScreen(self.hwnd, &mut pt) };
        self.filter_open.set(true);
        self.invalidate_filter_bar();
        let cmd = crate::popup::track(popup, self.hwnd, pt, "filter bar", &|_| None);
        // SAFETY: the menu this function made.
        unsafe { DestroyMenu(popup) };
        self.filter_open.set(false);
        self.invalidate_filter_bar();
        if let Some(filter) = u16::try_from(cmd).ok().and_then(Filter::from_menu_id) {
            self.set_filter(filter);
        }
    }

    /// Searches with `filter`: the Search menu, the filter bar, and Ctrl+]
    /// and Ctrl+[ all come here.
    pub(super) fn set_filter(&self, filter: Filter) {
        let mut o = self.opts.get();
        o.filter = filter;
        self.opts.set(o);
        self.start_search(false);
        self.invalidate_status();
    }

    /// View > Zoom > Zoom In (`Some(true)`), Zoom Out, or Reset (`None`).
    pub(super) fn zoom_step(&self, up: Option<bool>) {
        let z = match up {
            Some(up) => zoom::step(self.zoom.get(), up),
            None => zoom::DEFAULT,
        };
        self.set_zoom(z);
    }

    /// Ctrl+wheel over the list: a zoom step per notch (a fine wheel's
    /// small turns add up to one).
    pub(super) fn zoom_wheel(&self, delta: i32) {
        let turned = self.wheel.get() + delta;
        let notch = WHEEL_DELTA as i32;
        let steps = turned / notch;
        self.wheel.set(turned % notch);
        let mut z = self.zoom.get();
        for _ in 0..steps.abs() {
            z = zoom::step(z, steps > 0);
        }
        self.set_zoom(z);
    }

    /// Sizes the result list's fonts, rows, header and status bar at `z` %.
    fn set_zoom(&self, z: u32) {
        if z != self.zoom.get() {
            self.zoom.set(z);
            let old = self.fonts.replace(Fonts::new(self.dpi.get(), z));
            let font = self.fonts.borrow().normal;
            send(self.list, WM_SETFONT, font as usize, 0);
            // The list no longer draws with the old fonts.
            drop(old);
            self.remeasure_rows();
            self.layout();
            self.settings_changed();
        }
        self.show_note(format!("Zoom: {z}%"));
    }

    /// Has the list ask its rows' height again (WM_MEASUREITEM), which an
    /// owner-drawn list does when told it moved, and lay out its header.
    fn remeasure_rows(&self) {
        let mut rc = RECT::default();
        // SAFETY: reads the list's place in this thread's window; the
        // WINDOWPOS lives across the synchronous send.
        unsafe {
            GetWindowRect(self.list, &mut rc);
            MapWindowPoints(
                core::ptr::null_mut(),
                self.hwnd,
                (&raw mut rc).cast::<POINT>(),
                2,
            );
            let wp = WINDOWPOS {
                hwnd: self.list,
                hwndInsertAfter: core::ptr::null_mut(),
                x: rc.left,
                y: rc.top,
                cx: rc.right - rc.left,
                cy: rc.bottom - rc.top,
                flags: SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOOWNERZORDER | SWP_NOZORDER,
            };
            send(self.list, WM_WINDOWPOSCHANGED, 0, (&raw const wp) as isize);
            InvalidateRect(self.list, core::ptr::null(), 1);
            InvalidateRect(self.header, core::ptr::null(), 1);
        }
    }
}
