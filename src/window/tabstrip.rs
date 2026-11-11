//! The tab strip (the framework's `StripSlot::Tabs` row): Everything 1.5's,
//! measured on its window. It shows with two or more tabs
//! (`tabs_always_show=0`), 31 px at 100 %: 5 px of margin, then the tabs,
//! 26 px tall, sharing the width equally between `tabs_min_wide` (76) and
//! `tabs_max_wide` (224) px, each with its title (the search, or "New Tab")
//! and a × (`tabs_close_button=1`); a "+" after the last. The shown tab is
//! lit and edged, a line runs along the strip's foot under the others.
//! When they do not all fit, as many as fit show (the shown one always
//! among them) and an overflow button at the right end lists them all.
//!
//! The mouse: a press on a tab shows it (and a drag along the strip moves
//! it), the × closes it, so does a middle click (and a double-click with
//! `tabs_double_click_action=1`), "+" opens a new tab, a right-click opens
//! the tab menu (`tabs.rs`). The strip never takes the keyboard: Ctrl+Tab,
//! Ctrl+T, Ctrl+W and the menus reach every command.

use std::any::Any;
use std::cell::Cell;

use windows_sys::Win32::Foundation::{POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, HDC,
    ScreenToClient, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, SM_CXDRAG, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

use super::App;
use crate::colors::{Item as ColorItem, State as ColorState};
use crate::paint::fill;
use crate::searchbox::paint_cross;
use crate::strip::{Strip, StripSlot};
use crate::tabs::{self, Hit, Metrics, Placed};
use crate::theme::{Palette, rgb};

/// The room above the tabs: Everything's `tabs_margin_top` (3 px) and the
/// 2 px its menu bar is taller than Windows'.
const TOP: i32 = 5;

/// A button held down on the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Down {
    /// What it went down on.
    on: Hit,
    /// The middle button (a click closes the tab), not the left.
    middle: bool,
    /// Where it went down (client x), and whether a drag has begun.
    x: i32,
    dragging: bool,
}

/// The strip's colours, from the theme's.
struct Colours {
    /// The strip, and the text of the tabs not shown (Fonts and Colors'
    /// Tabs: Normal).
    strip: u32,
    text: u32,
    line: u32,
    /// The shown tab and its text (Tabs: Selected).
    selected: u32,
    selected_text: u32,
    edge: u32,
    hot: u32,
    glyph: u32,
    glyph_hot: u32,
    /// A × or button under the mouse, and held down.
    button_hot: u32,
    button_pressed: u32,
}

fn colours(pal: &Palette) -> Colours {
    let normal = pal.ink(ColorItem::Tabs, ColorState::Normal);
    let shown = pal.ink(ColorItem::Tabs, ColorState::Selected);
    if pal.dark {
        // Sampled from Everything's dark strip: the chrome (0x20), a line
        // of 0x30 along the foot, the shown tab 0x39 edged with 0x52.
        Colours {
            strip: normal.bg,
            text: normal.fg,
            line: pal.line,
            selected: shown.bg,
            selected_text: shown.fg,
            edge: pal.header_sort_edge,
            hot: pal.header_hot,
            glyph: pal.arrow,
            glyph_hot: pal.glyph_hot,
            button_hot: rgb(0x4a, 0x4a, 0x4a),
            button_pressed: rgb(0x5c, 0x5c, 0x5c),
        }
    } else {
        // Light: the shown tab white on the chrome, edged as the search
        // frame is.
        Colours {
            strip: normal.bg,
            text: normal.fg,
            line: pal.border,
            selected: shown.bg,
            selected_text: shown.fg,
            edge: pal.border,
            hot: rgb(0xe3, 0xe3, 0xe3),
            glyph: pal.glyph,
            glyph_hot: rgb(0x00, 0x00, 0x00),
            button_hot: rgb(0xd6, 0xd6, 0xd6),
            button_pressed: rgb(0xc4, 0xc4, 0xc4),
        }
    }
}

pub(super) struct TabStrip {
    /// The first tab in view while they do not all fit.
    first: Cell<usize>,
    /// What is under the mouse.
    hot: Cell<Hit>,
    /// A button held down on it.
    down: Cell<Option<Down>>,
}

impl TabStrip {
    pub(super) fn new() -> TabStrip {
        TabStrip {
            first: Cell::new(0),
            hot: Cell::new(Hit::Empty),
            down: Cell::new(None),
        }
    }

    /// The strip's measures in the row `rc` at the window's DPI.
    fn metrics(app: &App, rc: &RECT) -> Metrics {
        let s = app.tab_settings();
        Metrics {
            left: rc.left + app.scale(3),
            right: rc.right - app.scale(3),
            top: (rc.top + app.scale(TOP)).min(rc.bottom),
            bottom: rc.bottom,
            min: app.scale(s.min_wide),
            max: app.scale(s.max_wide),
            button: (rc.bottom - rc.top - app.scale(TOP)).max(1),
            close: if s.close_button { app.scale(16) } else { 0 },
            close_inset: app.scale(13),
        }
    }

    /// Where the tabs and buttons are now, in the row `rc`.
    fn placed(&self, app: &App, rc: &RECT) -> (Metrics, Placed) {
        let m = Self::metrics(app, rc);
        let p = tabs::place(app.tab_count(), app.current_tab(), self.first.get(), &m);
        self.first.set(p.first);
        (m, p)
    }

    /// Where each tab in view and the "+" are now (accessibility tools,
    /// `access.rs`).
    pub(super) fn tab_rects(&self, app: &App) -> (Vec<(usize, RECT)>, RECT) {
        let rc = app.strip_rect(StripSlot::Tabs);
        let (m, p) = self.placed(app, &rc);
        let span = |l: i32, r: i32| RECT {
            left: l,
            top: m.top,
            right: r,
            bottom: m.bottom,
        };
        let tabs = p.tabs.iter().map(|&(i, l, r)| (i, span(l, r))).collect();
        (tabs, span(p.plus.0, p.plus.1))
    }

    fn hit_at(&self, app: &App, pt: POINT) -> Hit {
        let rc = app.strip_rect(StripSlot::Tabs);
        if !inside(&rc, pt) {
            return Hit::Empty;
        }
        let (m, p) = self.placed(app, &rc);
        tabs::hit(&p, &m, pt.x, pt.y)
    }

    fn set_hot(&self, app: &App, hot: Hit) {
        if self.hot.replace(hot) != hot {
            app.invalidate_strip(StripSlot::Tabs);
        }
    }

    /// What is under the mouse now: after a tab closed or moved, the ×
    /// that slid under it lights at once.
    fn rehover(&self, app: &App) {
        let mut pt = POINT::default();
        // SAFETY: reads the cursor and maps it to this thread's window.
        unsafe {
            GetCursorPos(&mut pt);
            ScreenToClient(app.hwnd, &mut pt);
        }
        self.set_hot(app, self.hit_at(app, pt));
    }

    /// Holds the mouse for a press on the strip.
    fn press(&self, app: &App, down: Down) {
        self.down.set(Some(down));
        // SAFETY: takes the mouse for this thread's window.
        unsafe { SetCapture(app.hwnd) };
        app.capture_strip(Some(StripSlot::Tabs));
        app.invalidate_strip(StripSlot::Tabs);
    }

    /// Lets go of the mouse; the press it held, if any.
    fn release(&self, app: &App) -> Option<Down> {
        let down = self.down.take();
        app.capture_strip(None);
        // SAFETY: lets go of this thread's capture if it is the window's.
        unsafe {
            if GetCapture() == app.hwnd {
                ReleaseCapture();
            }
        }
        app.invalidate_strip(StripSlot::Tabs);
        down
    }

    /// A move while the left button holds tab `i`: past the system's drag
    /// distance the tab follows the mouse along the strip.
    fn drag(&self, app: &App, mut down: Down, pt: POINT) {
        let Hit::Tab(i) = down.on else {
            return;
        };
        if !down.dragging {
            // SAFETY: reads a system metric.
            let threshold = unsafe { GetSystemMetricsForDpi(SM_CXDRAG, app.dpi.get()) }.max(1);
            if (pt.x - down.x).abs() < threshold {
                return;
            }
            down.dragging = true;
        }
        let rc = app.strip_rect(StripSlot::Tabs);
        let (_, p) = self.placed(app, &rc);
        if let Some(to) = tabs::drop_slot(&p, pt.x)
            && to != i
            && i < app.tab_count()
        {
            app.move_tab(i, to);
            down.on = Hit::Tab(to);
        }
        self.down.set(Some(down));
    }

    /// Paints a tab: its face, title and ×.
    #[allow(clippy::too_many_arguments, reason = "one tab's whole state")]
    fn paint_tab(
        &self,
        app: &App,
        hdc: HDC,
        c: &Colours,
        m: &Metrics,
        (i, l, r): (usize, i32, i32),
        title: &str,
        selected: bool,
    ) {
        let tab = RECT {
            left: l,
            top: m.top,
            right: r,
            bottom: m.bottom,
        };
        let hot = matches!(self.hot.get(), Hit::Tab(h) | Hit::Close(h) if h == i);
        if selected {
            fill(hdc, &tab, c.selected);
            // Edged left, top and right; open at the foot, over the line.
            let edge = |rc: RECT| fill(hdc, &rc, c.edge);
            edge(RECT {
                right: l + 1,
                ..tab
            });
            edge(RECT { left: r - 1, ..tab });
            edge(RECT {
                bottom: m.top + 1,
                ..tab
            });
        } else if hot {
            fill(
                hdc,
                &RECT {
                    bottom: m.bottom - 1,
                    ..tab
                },
                c.hot,
            );
        }
        let (cl, ct, cr, cb) = tabs::close_box(r, m);
        let pad = app.scale(6);
        let mut text_rc = RECT {
            left: l + pad,
            top: m.top,
            right: if m.close > 0 {
                cl - app.scale(2)
            } else {
                r - pad
            },
            bottom: m.bottom,
        };
        if text_rc.right > text_rc.left {
            let mut text: Vec<u16> = title.encode_utf16().collect();
            let state = if selected {
                ColorState::Selected
            } else {
                ColorState::Normal
            };
            let font = app.part_font(
                ColorItem::Tabs,
                app.pal.get().is_bold(ColorItem::Tabs, state),
            );
            // SAFETY: draws into the paint buffer; the font is put back by
            // the caller.
            unsafe {
                SelectObject(hdc, font);
                SetTextColor(hdc, if selected { c.selected_text } else { c.text });
                DrawTextW(
                    hdc,
                    text.as_mut_ptr(),
                    text.len() as i32,
                    &mut text_rc,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
            }
        }
        if m.close <= 0 {
            return;
        }
        let close = RECT {
            left: cl,
            top: ct,
            right: cr,
            bottom: cb,
        };
        let over = self.hot.get() == Hit::Close(i);
        let held = self
            .down
            .get()
            .is_some_and(|d| !d.middle && d.on == Hit::Close(i));
        let face = if selected {
            c.selected
        } else if hot {
            c.hot
        } else {
            c.strip
        };
        let bg = match (over, held) {
            (true, true) => c.button_pressed,
            (true, false) | (false, true) => c.button_hot,
            _ => face,
        };
        if bg != face {
            fill(hdc, &close, bg);
        }
        let fg = if over { c.glyph_hot } else { c.glyph };
        let scale = app.dpi.get() as f32 / 96.0;
        paint_cross(hdc, (cl + cr) / 2, (ct + cb) / 2, 3.5, scale, fg, bg);
    }

    /// Paints a button (the "+" or the overflow list's) lit under the mouse
    /// or held; its glyph is drawn by `glyph` at the button's centre.
    fn paint_button(
        &self,
        app: &App,
        hdc: HDC,
        c: &Colours,
        rc: RECT,
        which: Hit,
        glyph: impl Fn(i32, i32, u32),
    ) {
        let over = self.hot.get() == which;
        let held = self.down.get().is_some_and(|d| d.on == which);
        let inset = app.scale(2);
        let face = RECT {
            left: rc.left + inset,
            top: rc.top + inset,
            right: rc.right - inset,
            bottom: rc.bottom - inset - 1,
        };
        match (over, held) {
            (true, true) => fill(hdc, &face, c.button_pressed),
            (true, false) | (false, true) => fill(hdc, &face, c.button_hot),
            _ => {}
        }
        let fg = if over { c.glyph_hot } else { c.glyph };
        glyph((rc.left + rc.right) / 2, (rc.top + rc.bottom - 1) / 2, fg);
    }
}

fn inside(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

/// A "+": two bars `arm` px out from its centre, `t` px thick.
fn plus(hdc: HDC, cx: i32, cy: i32, arm: i32, t: i32, colour: u32) {
    let h = t / 2;
    fill(
        hdc,
        &RECT {
            left: cx - arm,
            top: cy - h,
            right: cx + arm + 1,
            bottom: cy - h + t,
        },
        colour,
    );
    fill(
        hdc,
        &RECT {
            left: cx - h,
            top: cy - arm,
            right: cx - h + t,
            bottom: cy + arm + 1,
        },
        colour,
    );
}

/// A "▾", `half` px either side of its centre at the top, drawn row by row
/// so its edges stay sharp.
fn down_arrow(hdc: HDC, cx: i32, cy: i32, half: i32, colour: u32) {
    let top = cy - half / 2;
    for i in 0..=half {
        let reach = half - i;
        fill(
            hdc,
            &RECT {
                left: cx - reach,
                right: cx + reach + 1,
                top: top + i,
                bottom: top + i + 1,
            },
            colour,
        );
    }
}

impl Strip for TabStrip {
    /// 31 px at 100 % (its font's height (Fonts and Colors' Tabs) and 16
    /// px, so it follows the font as the header does), while it shows:
    /// Everything's 29 and the
    /// 2 px its own menu bar has over Windows' (the search row then keeps 3
    /// px above its frame instead of 5, so the tabs and the search frame sit
    /// exactly where Everything's do).
    fn height(&self, app: &App) -> i32 {
        if app.tab_strip_shown() {
            app.part_height(ColorItem::Tabs) + app.scale(TOP + 11)
        } else {
            0
        }
    }

    fn paint(&self, app: &App, hdc: HDC, rc: &RECT) {
        let pal = app.pal.get();
        let c = colours(&pal);
        fill(hdc, rc, c.strip);
        fill(
            hdc,
            &RECT {
                top: rc.bottom - 1,
                ..*rc
            },
            c.line,
        );
        let titles = app.tab_titles();
        let current = app.current_tab();
        let (m, p) = self.placed(app, rc);
        // SAFETY: draws into the paint buffer; the old font is put back.
        let old = unsafe {
            let old = SelectObject(hdc, app.part_font(ColorItem::Tabs, false));
            SetBkMode(hdc, TRANSPARENT as i32);
            old
        };
        for &(i, l, r) in &p.tabs {
            let title = titles.get(i).map_or("", String::as_str);
            self.paint_tab(app, hdc, &c, &m, (i, l, r), title, i == current);
        }
        // SAFETY: puts back the font selected above.
        unsafe { SelectObject(hdc, old) };
        let button = |(l, r): (i32, i32)| RECT {
            left: l,
            top: m.top,
            right: r,
            bottom: m.bottom,
        };
        let arm = app.scale(5);
        let t = app.scale(1).max(1);
        self.paint_button(app, hdc, &c, button(p.plus), Hit::Plus, |x, y, fg| {
            plus(hdc, x, y, arm, t, fg)
        });
        if let Some(o) = p.overflow {
            let half = app.scale(3).max(3);
            self.paint_button(app, hdc, &c, button(o), Hit::Overflow, |x, y, fg| {
                down_arrow(hdc, x, y, half, fg)
            });
        }
    }

    fn mouse(&self, app: &App, msg: u32, _w: WPARAM, pt: POINT) -> bool {
        let rc = app.strip_rect(StripSlot::Tabs);
        let over = inside(&rc, pt);
        match msg {
            WM_MOUSEMOVE => {
                if over {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: app.hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: asks for WM_MOUSELEAVE on this thread's window.
                    unsafe { TrackMouseEvent(&mut tme) };
                }
                if let Some(down) = self.down.get().filter(|d| !d.middle) {
                    self.drag(app, down, pt);
                }
                self.set_hot(app, self.hit_at(app, pt));
                false
            }
            WM_MOUSELEAVE => {
                if self.down.get().is_none() {
                    self.set_hot(app, Hit::Empty);
                }
                false
            }
            WM_CAPTURECHANGED => {
                if self.down.take().is_some() {
                    app.capture_strip(None);
                    app.invalidate_strip(StripSlot::Tabs);
                }
                false
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if over => {
                let hit = self.hit_at(app, pt);
                let down = Down {
                    on: hit,
                    middle: false,
                    x: pt.x,
                    dragging: false,
                };
                match hit {
                    Hit::Tab(i)
                        if msg == WM_LBUTTONDBLCLK && app.tab_settings().double_click_closes() =>
                    {
                        app.close_tab(i);
                        self.rehover(app);
                    }
                    Hit::Tab(i) => {
                        // The tab shows as the button goes down, as
                        // Everything's (and a drag can move it).
                        app.select_tab(i);
                        self.press(app, down);
                    }
                    Hit::Close(_) | Hit::Plus => self.press(app, down),
                    Hit::Overflow => {
                        let mut at = POINT {
                            x: overflow_left(self, app, &rc),
                            y: rc.bottom,
                        };
                        // SAFETY: maps a point of this thread's window to
                        // the screen.
                        unsafe { ClientToScreen(app.hwnd, &mut at) };
                        app.tab_overflow_menu(at);
                        self.rehover(app);
                    }
                    Hit::Empty => {}
                }
                true
            }
            WM_LBUTTONUP => {
                let Some(down) = self.down.get().filter(|d| !d.middle) else {
                    return over;
                };
                self.release(app);
                let hit = self.hit_at(app, pt);
                match down.on {
                    Hit::Close(i) if hit == Hit::Close(i) => app.close_tab(i),
                    Hit::Plus if hit == Hit::Plus => app.new_tab(),
                    _ => {}
                }
                self.rehover(app);
                true
            }
            WM_MBUTTONDOWN if over => {
                if let Hit::Tab(i) | Hit::Close(i) = self.hit_at(app, pt) {
                    self.press(
                        app,
                        Down {
                            on: Hit::Tab(i),
                            middle: true,
                            x: pt.x,
                            dragging: false,
                        },
                    );
                }
                true
            }
            WM_MBUTTONUP => {
                let Some(down) = self.down.get().filter(|d| d.middle) else {
                    return over;
                };
                self.release(app);
                if let (Hit::Tab(i), Hit::Tab(j) | Hit::Close(j)) = (down.on, self.hit_at(app, pt))
                    && i == j
                {
                    app.close_tab(i);
                }
                self.rehover(app);
                true
            }
            WM_RBUTTONDOWN => over,
            WM_RBUTTONUP if over => {
                let target = match self.hit_at(app, pt) {
                    Hit::Tab(i) | Hit::Close(i) => Some(i),
                    _ => None,
                };
                let mut at = pt;
                // SAFETY: maps a point of this thread's window to the
                // screen.
                unsafe { ClientToScreen(app.hwnd, &mut at) };
                app.tab_menu(target, at);
                self.rehover(app);
                true
            }
            _ => false,
        }
    }

    fn theme(&self, app: &App, _dark: bool) {
        app.invalidate_strip(StripSlot::Tabs);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The overflow button's left edge, where its list drops from.
fn overflow_left(strip: &TabStrip, app: &App, rc: &RECT) -> i32 {
    let (_, p) = strip.placed(app, rc);
    p.overflow.map_or(rc.right, |(l, _)| l)
}
