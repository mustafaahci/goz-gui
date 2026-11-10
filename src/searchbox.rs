//! The search box's two buttons, drawn by the main window inside the search
//! frame right of the text: Clear (×; a magnifier while the box is empty)
//! and Search History (▾), which opens the history dropdown. With the nav
//! bar on (`toolbar_nav_bar_visible=1`), Back and Forward (← →) sit left of
//! the frame, greyed while there is nowhere to go.
//!
//! The edit control stops short of them, so the mouse over them reaches the
//! main window, which hands it here. A press captures the mouse and a button
//! acts only when released over itself, so dragging off cancels. Glyphs are
//! drawn pixel by pixel from their geometry, antialiased, so they are crisp
//! and the same shape at every scale.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{HDC, InvalidateRect, PtInRect, SetPixelV};
use windows_sys::Win32::UI::Controls::{
    NMTTDISPINFOW, TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW,
    TTM_SETMAXTIPWIDTH, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, GetWindowTextLengthW, SendMessageW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::paint::fill;
use crate::theme::{self, Palette};
use crate::window::App;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Clear,
    History,
    /// The nav bar's Back and Forward.
    Back,
    Forward,
}

impl Button {
    const ALL: [Button; 4] = [
        Button::Clear,
        Button::History,
        Button::Back,
        Button::Forward,
    ];

    /// The button's tooltip id.
    fn tool(self) -> usize {
        match self {
            Button::Clear => 1,
            Button::History => 2,
            Button::Back => 3,
            Button::Forward => 4,
        }
    }
}

/// Where the parts of the search frame go.
#[derive(Clone, Copy)]
pub struct Frame {
    pub clear: RECT,
    pub history: RECT,
    /// The edit control's right edge, short of the buttons.
    pub edit_right: i32,
}

/// The buttons inside the 1px border of `search`, at the right, each as
/// tall as the inside and `width` wide (Everything's are 17 px at 100 %).
/// Without `history` (the search history is off) there is no History
/// button: its rectangle is empty and Clear takes its place.
pub fn frame(search: RECT, width: i32, gap: i32, history: bool) -> Frame {
    let inner = RECT {
        left: search.left + 1,
        top: search.top + 1,
        right: search.right - 1,
        bottom: search.bottom - 1,
    };
    let history = RECT {
        left: if history {
            (inner.right - width).max(inner.left)
        } else {
            inner.right
        },
        ..inner
    };
    let clear = RECT {
        left: (history.left - width).max(inner.left),
        right: history.left,
        ..inner
    };
    Frame {
        clear,
        history,
        edit_right: (clear.left - gap).max(inner.left),
    }
}

/// The search frame's layout at the window's DPI.
pub fn frame_of(app: &App) -> Frame {
    frame(
        app.search_rc.get(),
        app.scale(17),
        app.scale(2),
        app.search_history_enabled(),
    )
}

/// The nav bar's Back and Forward buttons left of `search`: squares as
/// tall as the frame, side by side, then `gap` before the frame.
pub fn nav_frame(search: RECT, gap: i32) -> (RECT, RECT) {
    let side = search.bottom - search.top;
    let forward = RECT {
        left: search.left - gap - side,
        right: search.left - gap,
        ..search
    };
    let back = RECT {
        left: forward.left - side,
        right: forward.left,
        ..search
    };
    (back, forward)
}

/// The room the nav bar takes left of a search frame `height` tall.
pub fn nav_room(height: i32, gap: i32) -> i32 {
    2 * height + gap
}

/// The nav bar's gap before the search frame, at 100 %.
pub const NAV_GAP: i32 = 3;

/// Back's and Forward's rectangles, while the nav bar shows.
pub fn nav_of(app: &App) -> Option<(RECT, RECT)> {
    app.nav_bar_visible()
        .then(|| nav_frame(app.search_rc.get(), app.scale(NAV_GAP)))
}

/// Whether a button can be used now: Back and Forward only when there is
/// somewhere to go.
fn enabled(app: &App, button: Button) -> bool {
    match button {
        Button::Back => app.can_navigate(true),
        Button::Forward => app.can_navigate(false),
        Button::Clear | Button::History => true,
    }
}

pub fn hit(app: &App, pt: POINT) -> Option<Button> {
    let f = frame_of(app);
    // SAFETY: reads two plain structs.
    let inside = |rc: &RECT| unsafe { PtInRect(rc, pt) } != 0;
    if let Some((back, forward)) = nav_of(app) {
        if inside(&back) {
            return Some(Button::Back);
        }
        if inside(&forward) {
            return Some(Button::Forward);
        }
    }
    if inside(&f.history) {
        Some(Button::History)
    } else if inside(&f.clear) {
        Some(Button::Clear)
    } else {
        None
    }
}

/// Repaints the buttons, which show the box's text, the mouse and the
/// dropdown.
pub fn invalidate(app: &App) {
    let f = frame_of(app);
    let rc = RECT {
        left: f.clear.left,
        right: f.history.right,
        ..f.clear
    };
    // SAFETY: invalidates part of this thread's window.
    unsafe { InvalidateRect(app.hwnd, &rc, 0) };
    invalidate_nav(app);
}

/// Repaints Back and Forward (whether each can be used may have changed).
pub fn invalidate_nav(app: &App) {
    if let Some((back, forward)) = nav_of(app) {
        let rc = RECT {
            right: forward.right,
            ..back
        };
        // SAFETY: invalidates part of this thread's window.
        unsafe { InvalidateRect(app.hwnd, &rc, 0) };
    }
}

fn set_hot(app: &App, hot: Option<Button>) {
    if app.search_hot.replace(hot) != hot {
        invalidate(app);
    }
}

/// The mouse moved over the main window, at `pt` in its client area.
pub fn mouse_move(app: &App, pt: POINT) {
    // A greyed button does not light up (its tooltip still shows).
    let over = hit(app, pt).filter(|&b| enabled(app, b));
    let hot = match app.search_pressed.get() {
        // Held down: lit only while over the button pressed.
        Some(pressed) => (over == Some(pressed)).then_some(pressed),
        None => {
            if over.is_some() {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: app.hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: asks for WM_MOUSELEAVE on this thread's window.
                unsafe { TrackMouseEvent(&mut tme) };
            }
            over
        }
    };
    set_hot(app, hot);
}

pub fn mouse_leave(app: &App) {
    if app.search_pressed.get().is_none() {
        set_hot(app, None);
    }
}

/// The left button went down at `pt`; true when on a button, which then
/// holds the mouse until it is released.
pub fn button_down(app: &App, pt: POINT) -> bool {
    let Some(button) = hit(app, pt) else {
        return false;
    };
    if !enabled(app, button) {
        // On a greyed button: nothing happens, and it is not the frame.
        return true;
    }
    app.search_pressed.set(Some(button));
    // SAFETY: captures the mouse for this thread's window.
    unsafe { SetCapture(app.hwnd) };
    set_hot(app, Some(button));
    true
}

/// The left button came up at `pt`: the button clicked, if it went down on
/// that same button.
pub fn button_up(app: &App, pt: POINT) -> Option<Button> {
    let pressed = app.search_pressed.take()?;
    // SAFETY: releases the capture `button_down` took, if it is still ours.
    unsafe {
        if GetCapture() == app.hwnd {
            ReleaseCapture();
        }
    }
    // Hovering again: lit while the mouse stays on a button, which needs
    // its leaving tracked anew.
    mouse_move(app, pt);
    (hit(app, pt) == Some(pressed)).then_some(pressed)
}

/// Something else took the mouse: the press is off.
pub fn capture_lost(app: &App) {
    if app.search_pressed.take().is_some() {
        set_hot(app, None);
    }
}

/// Draws both buttons into the main window's paint buffer.
pub fn paint(app: &App, hdc: HDC) {
    let pal = app.pal.get();
    let f = frame_of(app);
    // SAFETY: reads the length of this thread's search box.
    let empty = unsafe { GetWindowTextLengthW(app.edit) } == 0;
    let scale = app.dpi.get() as f32 / 96.0;
    let hot = app.search_hot.get();
    let open = app.dropdown.is_open();
    for (button, rc) in [(Button::Clear, f.clear), (Button::History, f.history)] {
        if rc.right <= rc.left {
            continue;
        }
        let clear = button == Button::Clear;
        // While a button is held, `hot` is that button or nothing. The
        // magnifier only says what the box is for.
        let lit = if clear {
            !empty && hot == Some(button)
        } else {
            open || hot == Some(button)
        };
        let (fg, bg) = colours(&pal, lit);
        fill(hdc, &rc, bg);
        let cx = (rc.left + rc.right) / 2;
        let cy = (rc.top + rc.bottom) / 2;
        match (clear, empty) {
            (true, true) => {
                // Dimmed, as Everything's: half way to the background.
                draw(hdc, cx, cy, &magnifier(scale), blend(bg, fg, 0.5), bg);
            }
            (true, false) => draw(hdc, cx, cy, &cross(scale), fg, bg),
            (false, _) if pal.dark => triangle(hdc, cx, cy, scale, fg),
            (false, _) => draw(hdc, cx, cy, &chevron(scale), fg, bg),
        }
    }
}

/// Draws Back and Forward into the main window's paint buffer: flat on the
/// window's background, lit under the mouse, darker while held, greyed
/// while there is nowhere to go.
pub fn paint_nav(app: &App, hdc: HDC) {
    let Some((back, forward)) = nav_of(app) else {
        return;
    };
    let pal = app.pal.get();
    let scale = app.dpi.get() as f32 / 96.0;
    let hot = app.search_hot.get();
    let pressed = app.search_pressed.get();
    for (button, rc) in [(Button::Back, back), (Button::Forward, forward)] {
        let (fg, bg) = nav_colours(
            &pal,
            enabled(app, button),
            hot == Some(button),
            pressed == Some(button),
        );
        fill(hdc, &rc, bg);
        let cx = (rc.left + rc.right) / 2;
        let cy = (rc.top + rc.bottom) / 2;
        draw(hdc, cx, cy, &arrow(scale, button == Button::Back), fg, bg);
    }
}

/// A nav button's glyph and background colours.
fn nav_colours(pal: &Palette, enabled: bool, hot: bool, pressed: bool) -> (u32, u32) {
    match (enabled, hot, pressed) {
        (false, _, _) => (blend(pal.chrome, pal.glyph, 0.4), pal.chrome),
        (true, true, true) => (pal.glyph_hot, pal.header_pressed),
        (true, true, false) => (pal.glyph_hot, pal.glyph_hot_bg),
        (true, false, _) => (pal.glyph, pal.chrome),
    }
}

/// One stroke of a glyph, in pixels from the glyph's centre (the centre of
/// the pixel at the button's middle).
#[derive(Clone, Copy, Debug)]
enum Stroke {
    /// A line from `a` to `b`, `half` its width on each side.
    Line {
        a: (f32, f32),
        b: (f32, f32),
        half: f32,
    },
    /// A circle round `c` of radius `r` (to the middle of its line).
    Ring { c: (f32, f32), r: f32, half: f32 },
}

impl Stroke {
    fn covers(self, x: f32, y: f32) -> bool {
        match self {
            Stroke::Line { a, b, half } => {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 {
                    (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (px, py) = (a.0 + t * dx - x, a.1 + t * dy - y);
                px * px + py * py <= half * half
            }
            Stroke::Ring { c, r, half } => {
                let d = ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt();
                (d - r).abs() <= half
            }
        }
    }
}

/// The ×: two diagonals across 7 px at 100 %, ending on whole pixels.
fn cross(scale: f32) -> Vec<Stroke> {
    let arm = (3.0 * scale).round().max(2.0);
    let half = 0.6 * scale.max(1.0);
    vec![
        Stroke::Line {
            a: (-arm, -arm),
            b: (arm, arm),
            half,
        },
        Stroke::Line {
            a: (-arm, arm),
            b: (arm, -arm),
            half,
        },
    ]
}

/// A × whose arms reach `arm` px from its centre at 100 % (the search
/// box's reach 3), centred on the pixel (`cx`, `cy`) over `bg`: the panes'
/// close buttons.
pub fn paint_cross(hdc: HDC, cx: i32, cy: i32, arm: f32, scale: f32, fg: u32, bg: u32) {
    let arm = (arm * scale).round().max(2.0);
    let half = 0.6 * scale.max(1.0);
    let strokes = [
        Stroke::Line {
            a: (-arm, -arm),
            b: (arm, arm),
            half,
        },
        Stroke::Line {
            a: (-arm, arm),
            b: (arm, -arm),
            half,
        },
    ];
    draw(hdc, cx, cy, &strokes, fg, bg);
}

/// A chevron pointing up (`up`) or down, 8 px wide at 100 %, centred on
/// the pixel (`cx`, `cy`) over `bg`: the find bar's Previous and Next.
pub fn paint_chevron(hdc: HDC, cx: i32, cy: i32, up: bool, scale: f32, fg: u32, bg: u32) {
    let s = scale;
    let half = 0.65 * s.max(1.0);
    let dir = if up { -1.0 } else { 1.0 };
    let strokes = [
        Stroke::Line {
            a: (-4.0 * s, -2.0 * s * dir),
            b: (0.0, 2.0 * s * dir),
            half,
        },
        Stroke::Line {
            a: (0.0, 2.0 * s * dir),
            b: (4.0 * s, -2.0 * s * dir),
            half,
        },
    ];
    draw(hdc, cx, cy, &strokes, fg, bg);
}

/// Back's ← (Forward's → when not `back`): a shaft 9 px long at 100 % and
/// a head 4 px back along each side.
fn arrow(scale: f32, back: bool) -> Vec<Stroke> {
    let s = scale;
    let half = 0.6 * s.max(1.0);
    let reach = (4.0 * s).round().max(3.0);
    let dir = if back { 1.0 } else { -1.0 };
    let tip = (-reach * dir, 0.0);
    vec![
        Stroke::Line {
            a: tip,
            b: (reach * dir, 0.0),
            half,
        },
        Stroke::Line {
            a: tip,
            b: (0.0, -reach),
            half,
        },
        Stroke::Line {
            a: tip,
            b: (0.0, reach),
            half,
        },
    ]
}

/// The magnifier: a lens up and to the right, its handle down to the left.
fn magnifier(scale: f32) -> Vec<Stroke> {
    let s = scale;
    vec![
        Stroke::Ring {
            c: (1.5 * s, -1.5 * s),
            r: 3.4 * s,
            half: 0.75 * s,
        },
        Stroke::Line {
            a: (-0.9 * s, 0.9 * s),
            b: (-4.3 * s, 4.3 * s),
            half: 0.8 * s,
        },
    ]
}

/// The light theme's ▾, a chevron, as Windows' own drop-down buttons have.
fn chevron(scale: f32) -> Vec<Stroke> {
    let s = scale;
    let half = 0.65 * s.max(1.0);
    vec![
        Stroke::Line {
            a: (-4.0 * s, -2.0 * s),
            b: (0.0, 2.0 * s),
            half,
        },
        Stroke::Line {
            a: (0.0, 2.0 * s),
            b: (4.0 * s, -2.0 * s),
            half,
        },
    ]
}

/// How much of the pixel whose top left corner is at (`x`, `y`) the strokes
/// cover, from 16 samples.
fn coverage(strokes: &[Stroke], x: f32, y: f32) -> f32 {
    const N: usize = 4;
    let mut hits = 0;
    for i in 0..N {
        for j in 0..N {
            let sx = x + (i as f32 + 0.5) / N as f32;
            let sy = y + (j as f32 + 0.5) / N as f32;
            if strokes.iter().any(|s| s.covers(sx, sy)) {
                hits += 1;
            }
        }
    }
    hits as f32 / (N * N) as f32
}

/// `from` moved `t` of the way to `to`, channel by channel.
fn blend(from: u32, to: u32, t: f32) -> u32 {
    let ch = |c: u32, shift: u32| ((c >> shift) & 0xff) as f32;
    let mut out = 0;
    for shift in [0, 8, 16] {
        let v = ch(from, shift) + (ch(to, shift) - ch(from, shift)) * t;
        out |= (v.round().clamp(0.0, 255.0) as u32) << shift;
    }
    out
}

/// Paints `strokes` centred on the pixel (`cx`, `cy`) over `bg`.
fn draw(hdc: HDC, cx: i32, cy: i32, strokes: &[Stroke], fg: u32, bg: u32) {
    // Far enough out for every glyph here at its scale.
    let reach = strokes
        .iter()
        .map(|s| match *s {
            Stroke::Line { a, b, half } => {
                a.0.abs().max(a.1.abs()).max(b.0.abs()).max(b.1.abs()) + half
            }
            Stroke::Ring { c, r, half } => c.0.abs().max(c.1.abs()) + r + half,
        })
        .fold(0.0f32, f32::max)
        .ceil() as i32
        + 1;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            // The glyph's centre is the middle of pixel (cx, cy).
            let c = coverage(strokes, dx as f32 - 0.5, dy as f32 - 0.5);
            if c > 0.0 {
                // SAFETY: sets one pixel of the paint buffer.
                unsafe { SetPixelV(hdc, cx + dx, cy + dy, blend(bg, fg, c)) };
            }
        }
    }
}

/// The dark theme's ▾: a filled triangle, 7 px wide and 4 tall at 100 %,
/// drawn row by row so its edges stay sharp.
fn triangle(hdc: HDC, cx: i32, cy: i32, scale: f32, colour: u32) {
    let half = (3.0 * scale).round().max(3.0) as i32;
    let top = cy - half / 2;
    for i in 0..=half {
        let reach = half - i;
        let row = RECT {
            left: cx - reach,
            right: cx + reach + 1,
            top: top + i,
            bottom: top + i + 1,
        };
        fill(hdc, &row, colour);
    }
}

/// The tooltips of both buttons, as one tooltip control over the main
/// window; their texts are asked for as they show (`tooltip_text`).
pub fn create_tooltip(app: &App) -> HWND {
    // SAFETY: creates a tooltip control owned by this thread's window and
    // registers two tools with it; the structs live across the calls.
    unsafe {
        let tip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            core::ptr::null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            app.hwnd,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null(),
        );
        if tip.is_null() {
            return tip;
        }
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, app.scale(400) as isize);
        for button in Button::ALL {
            let info = tool(app, button);
            SendMessageW(tip, TTM_ADDTOOLW, 0, (&raw const info) as isize);
        }
        tip
    }
}

/// LPSTR_TEXTCALLBACKW: the tooltip asks for the text each time it shows.
const TEXT_CALLBACK: *mut u16 = -1isize as *mut u16;

fn tool(app: &App, button: Button) -> TTTOOLINFOW {
    let f = frame_of(app);
    TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_SUBCLASS,
        hwnd: app.hwnd,
        uId: button.tool(),
        // The nav bar's while it is hidden: empty, so they never show.
        rect: match button {
            Button::Clear => f.clear,
            Button::History => f.history,
            Button::Back => nav_of(app).map_or_else(RECT::default, |n| n.0),
            Button::Forward => nav_of(app).map_or_else(RECT::default, |n| n.1),
        },
        lpszText: TEXT_CALLBACK,
        // SAFETY: the rest of TTTOOLINFOW is plain data.
        ..unsafe { core::mem::zeroed() }
    }
}

/// Moves the tooltips' areas to where the buttons are now.
pub fn place_tooltip(app: &App) {
    let tip = app.tooltip.get();
    if tip.is_null() {
        return;
    }
    for button in Button::ALL {
        let info = tool(app, button);
        // SAFETY: the struct lives across the synchronous call.
        unsafe { SendMessageW(tip, TTM_NEWTOOLRECTW, 0, (&raw const info) as isize) };
    }
}

/// Dark or light tooltips with the window.
pub fn theme_tooltip(app: &App, dark: bool) {
    let tip = app.tooltip.get();
    if tip.is_null() {
        return;
    }
    theme::allow(tip, dark);
    theme::set_class(
        tip,
        Some(if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        }),
    );
}

/// The text of a button's tooltip about to show: none over the magnifier,
/// which has nothing to clear.
pub fn tooltip_text(app: &App, info: &mut NMTTDISPINFOW) {
    // SAFETY: reads the length of this thread's search box.
    let empty = unsafe { GetWindowTextLengthW(app.edit) } == 0;
    let text = match info.hdr.idFrom {
        1 if !empty => "Clear",
        2 => "Search History",
        3 => "Go back one search",
        4 => "Go forward one search",
        _ => "",
    };
    info.szText = [0; 80];
    for (dst, src) in info.szText.iter_mut().zip(text.encode_utf16()) {
        *dst = src;
    }
    // The tooltip shows what this points at; the buffer is in `info`, which
    // the tooltip reads before it goes.
    info.lpszText = info.szText.as_mut_ptr();
}

/// The filter bar (View > Filter Bar) right of the search frame, `rc`: a
/// frame like the search box's holding the filter's name and a ▾ as wide
/// as the search box's buttons, lit while its list is `open`. Its list is
/// a menu (`App::filter_bar_click`), so it follows the theme as the
/// menus do.
pub fn paint_filter_bar(app: &App, hdc: HDC, rc: RECT, name: &str, open: bool) {
    use windows_sys::Win32::Graphics::Gdi::{
        DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT,
    };
    if rc.right - rc.left < 3 {
        return;
    }
    let pal = app.pal.get();
    fill(hdc, &rc, pal.border);
    let inner = RECT {
        left: rc.left + 1,
        top: rc.top + 1,
        right: rc.right - 1,
        bottom: rc.bottom - 1,
    };
    fill(hdc, &inner, pal.bg);
    let button = RECT {
        left: (inner.right - app.scale(17)).max(inner.left),
        ..inner
    };
    let (fg, bg) = colours(&pal, open);
    fill(hdc, &button, bg);
    let scale = app.dpi.get() as f32 / 96.0;
    let (cx, cy) = (
        (button.left + button.right) / 2,
        (button.top + button.bottom) / 2,
    );
    if pal.dark {
        triangle(hdc, cx, cy, scale, fg);
    } else {
        draw(hdc, cx, cy, &chevron(scale), fg, bg);
    }
    let mut text: Vec<u16> = name.encode_utf16().collect();
    let mut room = RECT {
        left: inner.left + app.scale(4),
        right: button.left - app.scale(2),
        ..inner
    };
    let font = app.ui_font();
    // SAFETY: draws into the paint buffer; the old font is put back.
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, pal.fg);
        DrawTextW(
            hdc,
            text.as_mut_ptr(),
            text.len() as i32,
            &mut room,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
}

/// A button's glyph and background colours, lit (under the mouse, held
/// down, or its dropdown open) or not.
fn colours(pal: &Palette, lit: bool) -> (u32, u32) {
    if lit {
        (pal.glyph_hot, pal.glyph_hot_bg)
    } else {
        (pal.glyph, pal.bg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::rgb;

    #[test]
    fn buttons_sit_inside_the_frame_right_of_the_text() {
        let search = RECT {
            left: 6,
            top: 6,
            right: 994,
            bottom: 34,
        };
        let f = frame(search, 17, 2, true);
        let h = f.history;
        assert_eq!((h.left, h.top, h.right, h.bottom), (976, 7, 993, 33));
        assert_eq!((f.clear.top, f.clear.bottom), (7, 33));
        assert_eq!(f.clear.right, f.history.left);
        assert_eq!(f.clear.right - f.clear.left, 17);
        assert_eq!(f.edit_right, f.clear.left - 2);
        // A frame too narrow for them squeezes them, never out of it.
        let tiny = frame(
            RECT {
                left: 0,
                top: 0,
                right: 20,
                bottom: 20,
            },
            17,
            2,
            true,
        );
        assert!(tiny.clear.left >= 1 && tiny.edit_right >= 1);
        assert!(tiny.clear.left <= tiny.clear.right);
        // With the search history off there is no History button: Clear
        // sits at the right, and the text gets the room.
        let off = frame(search, 17, 2, false);
        assert!(off.history.right <= off.history.left);
        assert_eq!((off.clear.left, off.clear.right), (976, 993));
        assert_eq!(off.edit_right, 974);
    }

    fn mask(strokes: &[Stroke], reach: i32) -> Vec<Vec<f32>> {
        (-reach..=reach)
            .map(|dy| {
                (-reach..=reach)
                    .map(|dx| coverage(strokes, dx as f32 - 0.5, dy as f32 - 0.5))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_cross_is_7_pixels_and_symmetric_at_100_percent() {
        let m = mask(&cross(1.0), 5);
        // Solid down both diagonals from -3 to 3, nothing beyond.
        for d in -3..=3 {
            assert!(m[(5 + d) as usize][(5 + d) as usize] > 0.5, "{d}");
            assert!(m[(5 + d) as usize][(5 - d) as usize] > 0.5, "{d}");
        }
        assert_eq!(m[0][0], 0.0);
        assert_eq!(m[1][1], 0.0);
        for (y, row) in m.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                assert!((c - m[x][y]).abs() < 1e-6, "mirror across the diagonal");
                assert!((c - row[10 - x]).abs() < 1e-6, "mirror left to right");
            }
        }
    }

    #[test]
    fn the_nav_bar_sits_left_of_the_frame() {
        // At 100 %: the frame 23 px tall from x 52 (3 + 2 × 23 + 3).
        let search = RECT {
            left: 3 + nav_room(23, 3),
            top: 5,
            right: 994,
            bottom: 28,
        };
        assert_eq!(search.left, 52);
        let (back, forward) = nav_frame(search, 3);
        assert_eq!((back.left, back.right), (3, 26));
        assert_eq!((forward.left, forward.right), (26, 49));
        assert_eq!(
            (back.top, back.bottom, forward.top, forward.bottom),
            (5, 28, 5, 28)
        );
        assert_eq!(forward.right + 3, search.left);
    }

    #[test]
    fn the_arrows_are_9_pixels_and_mirror_each_other() {
        let back = mask(&arrow(1.0, true), 6);
        let forward = mask(&arrow(1.0, false), 6);
        for (y, row) in back.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                assert!((c - forward[y][12 - x]).abs() < 1e-6, "left to right");
                assert!((c - back[12 - y][x]).abs() < 1e-6, "top to bottom");
            }
        }
        // The shaft runs from -4 to 4 on the middle row; the tip is left.
        let solid: Vec<usize> = (0..13).filter(|&x| back[6][x] > 0.5).collect();
        assert_eq!(solid, (2..=10).collect::<Vec<_>>());
        assert!(back[2][6] > 0.5 && back[10][6] > 0.5, "the head's ends");
        assert_eq!(back[2][9], 0.0);
    }

    #[test]
    fn nav_buttons_show_their_state() {
        for pal in [Palette::new(true), Palette::new(false)] {
            assert_eq!(
                nav_colours(&pal, true, false, false),
                (pal.glyph, pal.chrome)
            );
            assert_eq!(
                nav_colours(&pal, true, true, false),
                (pal.glyph_hot, pal.glyph_hot_bg)
            );
            assert_eq!(
                nav_colours(&pal, true, true, true),
                (pal.glyph_hot, pal.header_pressed)
            );
            // Greyed: the same flat background whatever the mouse does, and
            // a glyph closer to it than the usual one.
            let grey = nav_colours(&pal, false, true, true);
            assert_eq!(grey.1, pal.chrome);
            assert_eq!(grey.0, blend(pal.chrome, pal.glyph, 0.4));
            assert_ne!(grey.0, pal.glyph);
        }
    }

    fn back_arrow(scale: f32) -> Vec<Stroke> {
        arrow(scale, true)
    }

    #[test]
    fn glyphs_grow_with_the_scale() {
        let lit = |m: &Vec<Vec<f32>>| m.iter().flatten().filter(|&&c| c > 0.5).count();
        for shape in [cross, magnifier, chevron, back_arrow] {
            let small = lit(&mask(&shape(1.0), 12));
            let large = lit(&mask(&shape(2.0), 12));
            assert!(large > small * 3, "{small} {large}");
        }
    }

    #[test]
    fn blending_goes_from_one_colour_to_the_other() {
        let (a, b) = (rgb(0, 0, 0), rgb(0xb4, 0xb4, 0xb4));
        assert_eq!(blend(a, b, 0.0), a);
        assert_eq!(blend(a, b, 1.0), b);
        // Everything's dimmed magnifier on black.
        assert_eq!(blend(a, b, 0.5), rgb(0x5a, 0x5a, 0x5a));
        assert_eq!(
            blend(rgb(0xff, 0, 0x10), rgb(0, 0xff, 0x10), 0.5),
            rgb(0x80, 0x80, 0x10)
        );
    }

    #[test]
    fn lit_buttons_use_the_hot_colours() {
        let dark = Palette::new(true);
        assert_eq!(
            colours(&dark, true),
            (rgb(0xff, 0xff, 0xff), rgb(0x3a, 0x3a, 0x3a))
        );
        assert_eq!(colours(&dark, false), (rgb(0xb4, 0xb4, 0xb4), dark.bg));
        let light = Palette::new(false);
        assert_eq!(colours(&light, false), (rgb(0x60, 0x60, 0x60), light.bg));
    }
}
