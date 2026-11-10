//! The docks and their panes, and the strips, from the window's side: what
//! is registered and open, where it goes in a layout, the dividers dragged
//! with the mouse, the View menu's pane items, and the keyboard between the
//! frames (F6 / Shift+F6, Tab / Shift+Tab over the tab stops, Shift+Esc
//! closing the pane that has the keyboard). See `crate::dock` for the
//! arithmetic and the settings, `crate::pane` for a pane's window and
//! `crate::strip` for the strips. FRAMEWORK.md (in the wave's notes) says
//! how a feature uses it.

use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetFocus, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, DestroyWindow, EndDeferWindowPos, GetClassNameW,
    GetCursorPos, HMENU, IDC_SIZENS, IDC_SIZEWE, IsChild, IsWindow, LoadCursorW, MSG, SW_SHOWNA,
    SWP_NOACTIVATE, SWP_NOZORDER, SetCursor, ShowWindow,
};

use super::App;
use crate::dock::{self, DockSettings, PaneId, PaneLayout, Side};
use crate::layout::{self, Regions};
use crate::pane::{self, PaneContent, PaneEvent, PaneSpec};
use crate::strip::{Strip, StripSlot};
use crate::{columns, menu};

/// A registered pane.
struct Registered {
    id: PaneId,
    title: &'static str,
    min_high: i32,
    make: Rc<dyn Fn() -> Rc<dyn PaneContent>>,
    /// Its place now: for Everything's panes, kept in the settings.
    layout: PaneLayout,
    open: Option<Open>,
}

/// An open pane's window and content.
#[derive(Clone)]
struct Open {
    hwnd: HWND,
    content: Rc<dyn PaneContent>,
}

/// A divider the mouse can drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Divider {
    /// Between a dock and the results: the dock's width.
    Dock(Side),
    /// Under pane `n` of a dock (from the top): its height and the next's.
    Split(Side, usize),
}

/// A drag under way: where it started and what it started from.
struct Drag {
    divider: Divider,
    start: POINT,
    /// The dock's width (pixels), or its panes' heights as shown.
    width: i32,
    heights: Vec<i32>,
}

/// Where the panes went in the last layout, for the mouse.
#[derive(Default)]
struct Placed {
    /// Each dock's open panes, top to bottom, with their rectangles.
    panes: [Vec<(PaneId, RECT)>; 2],
    /// The dividers between them.
    splits: [Vec<RECT>; 2],
}

/// The docks' state.
pub(super) struct Docks {
    /// The settings as read; Everything's panes' layouts and the widths
    /// are written back from here.
    settings: DockSettings,
    panes: Vec<Registered>,
    placed: Placed,
    drag: Option<Drag>,
}

impl Docks {
    pub(super) fn new(settings: DockSettings) -> Docks {
        Docks {
            settings,
            panes: Vec::new(),
            placed: Placed::default(),
            drag: None,
        }
    }

    fn get(&self, id: PaneId) -> Option<&Registered> {
        self.panes.iter().find(|p| p.id == id)
    }

    fn get_mut(&mut self, id: PaneId) -> Option<&mut Registered> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    /// The open panes of `side`, top to bottom.
    fn open_on(&self, side: Side) -> Vec<&Registered> {
        let mut v: Vec<&Registered> = self
            .panes
            .iter()
            .filter(|p| p.open.is_some() && p.layout.side == side)
            .collect();
        v.sort_by_key(|p| (p.layout.order, p.id.rank()));
        v
    }

    /// What goes back to the settings file.
    fn to_settings(&self) -> DockSettings {
        let mut s = self.settings;
        for p in &self.panes {
            s.set_pane(p.id, p.layout);
        }
        s
    }
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

fn slot_index(slot: StripSlot) -> usize {
    match slot {
        StripSlot::Tabs => 0,
        StripSlot::FindBar => 1,
    }
}

fn inside(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

/// A place the keyboard can be, which F6 steps through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Frame {
    Search,
    List,
    Pane(PaneId),
    Strip(StripSlot),
}

impl App {
    // ---- Registering, opening and closing panes ----

    /// The panes, strips and menu items of this build (`slots.rs`), and the
    /// test hook's: called once as the window starts.
    pub(super) fn register_panes(&self) {
        pane::register_class();
        self.start_features();
        let wanted = crate::testpane::wanted();
        for spec in crate::testpane::specs(&wanted) {
            self.register_pane(spec);
        }
        for (want, slot) in [
            (wanted.tabs, StripSlot::Tabs),
            (wanted.find, StripSlot::FindBar),
        ] {
            if want {
                self.set_strip(slot, Some(Rc::new(crate::testpane::TestStrip::new(slot))));
            }
        }
    }

    /// Adds a pane to the window: its View menu item (Everything's panes),
    /// and the pane itself if the settings say it was open (a test pane
    /// opens at once). Registering an id twice keeps the first.
    pub fn register_pane(&self, spec: PaneSpec) {
        let open = {
            let mut d = self.docks.borrow_mut();
            if d.get(spec.id).is_some() {
                return;
            }
            let layout = match spec.id.key() {
                Some(_) => d.settings.pane(spec.id),
                None => PaneLayout {
                    visible: true,
                    side: spec.side,
                    order: spec.order,
                    ..spec.id.default_layout()
                },
            };
            d.panes.push(Registered {
                id: spec.id,
                title: spec.title,
                min_high: spec.min_high.max(0),
                make: Rc::from(spec.make),
                layout,
                open: None,
            });
            layout.visible
        };
        self.update_pane_menu();
        if open {
            self.open_pane(spec.id);
        }
    }

    /// Whether a pane of `id` is registered (this build has it).
    pub fn pane_registered(&self, id: PaneId) -> bool {
        self.docks.borrow().get(id).is_some()
    }

    /// Whether pane `id` is open.
    pub fn pane_visible(&self, id: PaneId) -> bool {
        self.docks
            .borrow()
            .get(id)
            .is_some_and(|p| p.open.is_some())
    }

    /// Opens (`show`) or closes pane `id`; the settings keep which. An id
    /// not registered does nothing.
    pub fn show_pane(&self, id: PaneId, show: bool) {
        if show {
            self.open_pane(id);
        } else {
            self.close_pane(id, true);
        }
    }

    /// Opens pane `id` if it is closed, closes it if it is open.
    pub fn toggle_pane(&self, id: PaneId) {
        self.show_pane(id, !self.pane_visible(id));
    }

    /// Pane `id`'s window, while it is open.
    pub fn pane_hwnd(&self, id: PaneId) -> Option<HWND> {
        self.docks
            .borrow()
            .get(id)
            .and_then(|p| p.open.as_ref().map(|o| o.hwnd))
    }

    /// Where pane `id`'s content goes (`PaneContent::place`'s rectangle),
    /// in the pane window's coordinates, while it is open.
    pub fn pane_client_rect(&self, id: PaneId) -> Option<RECT> {
        self.pane_hwnd(id).map(|h| pane::content_rect(self, h))
    }

    /// Pane `id`'s content, while it is open: its feature reaches it with
    /// `as_any().downcast_ref::<T>()`.
    #[allow(
        dead_code,
        reason = "the features' way back to their panes (FRAMEWORK.md)"
    )]
    pub fn pane_content(&self, id: PaneId) -> Option<Rc<dyn PaneContent>> {
        self.docks
            .borrow()
            .get(id)
            .and_then(|p| p.open.as_ref().map(|o| Rc::clone(&o.content)))
    }

    /// Whether panes have their title rows (`pane_title`).
    pub fn pane_titles(&self) -> bool {
        self.docks.borrow().settings.title
    }

    fn open_pane(&self, id: PaneId) {
        let (title, make) = {
            let d = self.docks.borrow();
            let Some(p) = d.get(id) else { return };
            if p.open.is_some() {
                return;
            }
            (p.title, Rc::clone(&p.make))
        };
        let content = make();
        // The content makes its windows as the pane's is made
        // (`pane::create_window`, WM_CREATE), before any `place`.
        let hwnd = pane::create_window(self.hwnd, id, title, Rc::clone(&content));
        if hwnd.is_null() {
            return;
        }
        if let Some(p) = self.docks.borrow_mut().get_mut(id) {
            p.open = Some(Open { hwnd, content });
            p.layout.visible = true;
        }
        self.layout();
        // SAFETY: shows this thread's pane, now in its place, without
        // taking the keyboard or activating anything.
        unsafe { ShowWindow(hwnd, SW_SHOWNA) };
        self.update_pane_menu_checks();
        self.settings_changed();
    }

    /// Closes pane `id`: its content lets go of what it holds, then its
    /// window goes. `forget`: the user closed it (the settings keep it
    /// closed); else the window is ending and it reopens next time.
    fn close_pane(&self, id: PaneId, forget: bool) {
        let open = {
            let mut d = self.docks.borrow_mut();
            let Some(p) = d.get_mut(id) else { return };
            if forget {
                p.layout.visible = false;
            }
            p.open.take()
        };
        let Some(open) = open else { return };
        // SAFETY: reads and moves the focus within this thread's windows.
        unsafe {
            let focus = GetFocus();
            if focus == open.hwnd || IsChild(open.hwnd, focus) != 0 {
                // The keyboard goes to the results, as Everything's does.
                SetFocus(self.list);
            }
            let last = self.last_focus.get();
            if last == open.hwnd || IsChild(open.hwnd, last) != 0 {
                self.last_focus.set(self.list);
            }
        }
        open.content.destroy(self);
        // SAFETY: destroys this thread's pane window and its children.
        unsafe { DestroyWindow(open.hwnd) };
        drop(open);
        if forget {
            self.layout();
            self.update_pane_menu_checks();
            self.settings_changed();
        }
    }

    /// The window is ending: every pane's content lets go of what it holds
    /// (the panes reopen next time as they were).
    pub(super) fn close_all_panes(&self) {
        let ids: Vec<PaneId> = self
            .docks
            .borrow()
            .panes
            .iter()
            .filter(|p| p.open.is_some())
            .map(|p| p.id)
            .collect();
        for id in ids {
            self.close_pane(id, false);
        }
    }

    /// Gives pane `id` the keyboard (its content's choice of window).
    pub fn focus_pane(&self, id: PaneId) {
        let Some(open) = self.open_of(id) else { return };
        let target = open
            .content
            .focus()
            .filter(|&h| !h.is_null())
            .unwrap_or(open.hwnd);
        // SAFETY: focuses a window of this thread.
        unsafe { SetFocus(target) };
    }

    fn open_of(&self, id: PaneId) -> Option<Open> {
        self.docks.borrow().get(id).and_then(|p| p.open.clone())
    }

    /// The open panes' contents, for telling them something.
    fn open_contents(&self) -> Vec<Rc<dyn PaneContent>> {
        self.docks
            .borrow()
            .panes
            .iter()
            .filter_map(|p| p.open.as_ref().map(|o| Rc::clone(&o.content)))
            .collect()
    }

    /// Tells every open pane `event`.
    pub fn pane_event(&self, event: PaneEvent) {
        if self.docks.borrow().panes.iter().all(|p| p.open.is_none()) {
            return;
        }
        for content in self.open_contents() {
            content.event(self, event);
        }
    }

    /// Tells the open panes whether the window is on screen, when that
    /// changed.
    pub(super) fn panes_window_shown(&self, shown: bool) {
        if self.panes_shown.replace(shown) != shown {
            self.pane_event(PaneEvent::WindowShown(shown));
        }
    }

    /// The theme changed: the panes' and strips' contents follow (the
    /// window repaints them).
    pub(super) fn docks_theme(&self, dark: bool) {
        for content in self.open_contents() {
            content.theme(self, dark);
        }
        for strip in self.strips_now() {
            strip.theme(self, dark);
        }
    }

    /// The DPI changed: contents remake what depends on it; a layout
    /// follows, then every pane's content is placed again (its title row
    /// may have changed height without its size changing).
    pub(super) fn docks_dpi(&self, dpi: u32) {
        for content in self.open_contents() {
            content.dpi(self, dpi);
        }
        for strip in self.strips_now() {
            strip.dpi(self, dpi);
        }
    }

    pub(super) fn docks_place_all(&self) {
        let open: Vec<Open> = self
            .docks
            .borrow()
            .panes
            .iter()
            .filter_map(|p| p.open.clone())
            .collect();
        for o in open {
            o.content.place(self, pane::content_rect(self, o.hwnd));
            // SAFETY: repaints this thread's pane.
            unsafe { InvalidateRect(o.hwnd, core::ptr::null(), 0) };
        }
    }

    /// The docks as the settings file keeps them.
    pub(super) fn dock_settings(&self) -> DockSettings {
        self.docks.borrow().to_settings()
    }

    // ---- The strips ----

    /// Puts `strip` in `slot` (None empties it) and lays the window out.
    pub fn set_strip(&self, slot: StripSlot, strip: Option<Rc<dyn Strip>>) {
        self.strips.borrow_mut()[slot_index(slot)] = strip;
        if self.strip_capture.get() == Some(slot) {
            self.strip_capture.set(None);
        }
        self.layout();
    }

    /// The strip in `slot`.
    pub fn strip(&self, slot: StripSlot) -> Option<Rc<dyn Strip>> {
        self.strips.borrow()[slot_index(slot)].clone()
    }

    fn strips_now(&self) -> Vec<Rc<dyn Strip>> {
        self.strips.borrow().iter().flatten().cloned().collect()
    }

    /// Where `slot`'s row is (client coordinates); empty while it has none.
    pub fn strip_rect(&self, slot: StripSlot) -> RECT {
        let r = self.regions.get();
        match slot {
            StripSlot::Tabs => r.tabs,
            StripSlot::FindBar => r.find,
        }
    }

    /// Repaints `slot`'s row.
    pub fn invalidate_strip(&self, slot: StripSlot) {
        let rc = self.strip_rect(slot);
        if !layout::is_empty(&rc) {
            // SAFETY: repaints part of this thread's window.
            unsafe { InvalidateRect(self.hwnd, &rc, 0) };
        }
    }

    /// The strip in `slot` holds the mouse (it called `SetCapture` on the
    /// main window): every mouse message goes to it until `None`.
    pub fn capture_strip(&self, slot: Option<StripSlot>) {
        self.strip_capture.set(slot);
    }

    /// A message from a strip's own control (`WM_COMMAND`, `WM_NOTIFY`,
    /// `WM_CTLCOLOR*`, `WM_DRAWITEM`), for that strip: its answer, or
    /// `None` when no strip owns the control or the strip leaves it.
    pub(super) fn strip_message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, NMHDR};
        use windows_sys::Win32::UI::WindowsAndMessaging::{WM_DRAWITEM, WM_NOTIFY};
        if self.strips.borrow().iter().all(Option::is_none) || l == 0 {
            return None;
        }
        // SAFETY: each message's lParam as it defines it: a control's
        // window, an NMHDR from a control, a DRAWITEMSTRUCT.
        let from = unsafe {
            match msg {
                WM_NOTIFY => (*(l as *const NMHDR)).hwndFrom,
                WM_DRAWITEM => (*(l as *const DRAWITEMSTRUCT)).hwndItem,
                _ => l as HWND,
            }
        };
        [StripSlot::Tabs, StripSlot::FindBar]
            .into_iter()
            .filter_map(|slot| self.strip(slot))
            .find(|s| s.contains(from))
            .and_then(|s| s.message(self, msg, w, l))
    }

    /// Lays the window out again: a strip's height or a pane's content
    /// wanting other room.
    pub fn relayout(&self) {
        self.layout();
    }

    /// The strips' heights now, (tab strip, find bar).
    pub(super) fn strip_heights(&self) -> (i32, i32) {
        let [tabs, find] = self.strips.borrow().clone();
        (
            tabs.map_or(0, |s| s.height(self).max(0)),
            find.map_or(0, |s| s.height(self).max(0)),
        )
    }

    /// Paints the strips' rows that `dirty` touches into the main window's
    /// buffer.
    pub fn paint_strips(&self, hdc: windows_sys::Win32::Graphics::Gdi::HDC, dirty: &RECT) {
        for slot in [StripSlot::Tabs, StripSlot::FindBar] {
            let rc = self.strip_rect(slot);
            if layout::is_empty(&rc) || !overlaps(&rc, dirty) {
                continue;
            }
            if let Some(strip) = self.strip(slot) {
                strip.paint(self, hdc, &rc);
            }
        }
    }

    /// A mouse message of the main window for the strips: the one holding
    /// the mouse gets them all, else the one under `pt`; moves and leaves
    /// go to each. True when a strip took a button message.
    pub(super) fn strip_mouse(&self, msg: u32, w: usize, pt: POINT) -> bool {
        use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
        use windows_sys::Win32::UI::WindowsAndMessaging::{WM_CAPTURECHANGED, WM_MOUSEMOVE};
        if self.strips.borrow().iter().all(Option::is_none) {
            return false;
        }
        let captured = self.strip_capture.get();
        if msg == WM_CAPTURECHANGED {
            self.strip_capture.set(None);
        }
        let mut took = false;
        for slot in [StripSlot::Tabs, StripSlot::FindBar] {
            let Some(strip) = self.strip(slot) else {
                continue;
            };
            let over = inside(&self.strip_rect(slot), pt);
            let to_it = match captured {
                Some(c) => c == slot,
                None => over || matches!(msg, WM_MOUSELEAVE | WM_CAPTURECHANGED | WM_MOUSEMOVE),
            };
            if to_it && strip.mouse(self, msg, w, pt) {
                took = true;
            }
        }
        took && !matches!(msg, WM_MOUSEMOVE | WM_MOUSELEAVE | WM_CAPTURECHANGED)
    }

    // ---- Layout ----

    /// The docks' widths wanted now (pixels; 0 without an open pane).
    pub(super) fn dock_widths(&self) -> (i32, i32) {
        let d = self.docks.borrow();
        let dpi = self.dpi.get();
        let w = |side: Side, wide: i32| {
            if d.open_on(side).is_empty() {
                0
            } else {
                columns::scale(wide, dpi)
            }
        };
        (
            w(Side::Left, d.settings.left_wide),
            w(Side::Right, d.settings.right_wide),
        )
    }

    /// The docks' least width, and the results', in pixels.
    pub(super) fn dock_limits(&self) -> (i32, i32) {
        let min = self.docks.borrow().settings.min_wide;
        (
            columns::scale(min, self.dpi.get()),
            self.scale(dock::MIN_CENTER),
        )
    }

    /// Places the open panes in `r`'s docks (into `defer`), and notes where
    /// they and their dividers went.
    pub(super) fn place_panes(&self, r: &Regions, defer: &mut Defer) {
        let title = pane::title_height(self);
        let divider = self.scale(dock::DIVIDER);
        let dpi = self.dpi.get();
        let mut placed = Placed::default();
        let mut moves = Vec::new();
        {
            let d = self.docks.borrow();
            for (side, rc) in [(Side::Left, r.left), (Side::Right, r.right)] {
                let open = d.open_on(side);
                if open.is_empty() || layout::is_empty(&rc) {
                    for p in &open {
                        moves.push((p.open.as_ref().expect("open").hwnd, RECT::default()));
                    }
                    continue;
                }
                let wanted: Vec<i32> = open
                    .iter()
                    .map(|p| columns::scale(p.layout.high, dpi))
                    .collect();
                let mins: Vec<i32> = open
                    .iter()
                    .map(|p| title + self.scale(p.min_high))
                    .collect();
                let (panes, splits) = dock::stack(rc, &wanted, &mins, divider);
                for (p, prc) in open.iter().zip(&panes) {
                    moves.push((p.open.as_ref().expect("open").hwnd, *prc));
                    placed.panes[side_index(side)].push((p.id, *prc));
                }
                placed.splits[side_index(side)] = splits;
            }
        }
        self.docks.borrow_mut().placed = placed;
        for (hwnd, rc) in moves {
            defer.put(hwnd, rc);
        }
    }

    /// Repaints the dividers (the main window draws them).
    fn invalidate_main_area(&self) {
        let r = self.regions.get();
        // SAFETY: repaints part of this thread's window.
        unsafe { InvalidateRect(self.hwnd, &r.main, 0) };
    }

    /// Every divider's rectangle now: the docks' and the splits between
    /// panes.
    pub(super) fn dividers(&self) -> Vec<(Divider, RECT)> {
        let r = self.regions.get();
        let mut out = Vec::new();
        for (side, rc) in [(Side::Left, r.left_divider), (Side::Right, r.right_divider)] {
            if !layout::is_empty(&rc) {
                out.push((Divider::Dock(side), rc));
            }
        }
        let d = self.docks.borrow();
        for side in [Side::Left, Side::Right] {
            for (i, rc) in d.placed.splits[side_index(side)].iter().enumerate() {
                if !layout::is_empty(rc) {
                    out.push((Divider::Split(side, i), *rc));
                }
            }
        }
        out
    }

    /// Paints the dividers `dirty` touches into the main window's buffer:
    /// a line along each edge and the chrome between, as Everything's.
    pub fn paint_dividers(&self, hdc: windows_sys::Win32::Graphics::Gdi::HDC, dirty: &RECT) {
        let pal = self.pal.get();
        for (divider, rc) in self.dividers() {
            if !overlaps(&rc, dirty) {
                continue;
            }
            crate::paint::fill(hdc, &rc, pal.chrome);
            let (a, b) = match divider {
                Divider::Dock(_) => (
                    RECT {
                        right: rc.left + 1,
                        ..rc
                    },
                    RECT {
                        left: rc.right - 1,
                        ..rc
                    },
                ),
                Divider::Split(..) => (
                    RECT {
                        bottom: rc.top + 1,
                        ..rc
                    },
                    RECT {
                        top: rc.bottom - 1,
                        ..rc
                    },
                ),
            };
            crate::paint::fill(hdc, &a, pal.line);
            crate::paint::fill(hdc, &b, pal.line);
        }
    }

    /// The divider at `pt` (client coordinates).
    pub(super) fn divider_at(&self, pt: POINT) -> Option<Divider> {
        self.dividers()
            .into_iter()
            .find(|(_, rc)| inside(rc, pt))
            .map(|(d, _)| d)
    }

    /// The resize cursor over a divider (or while one is dragged); true if
    /// it was set.
    pub(super) fn divider_cursor(&self) -> bool {
        let dragging = self.docks.borrow().drag.as_ref().map(|d| d.divider);
        let divider = dragging.or_else(|| {
            let mut pt = POINT::default();
            // SAFETY: reads the cursor and maps it into this window.
            unsafe {
                GetCursorPos(&mut pt);
                ScreenToClient(self.hwnd, &mut pt);
            }
            self.divider_at(pt)
        });
        let Some(divider) = divider else {
            return false;
        };
        let shape = match divider {
            Divider::Dock(_) => IDC_SIZEWE,
            Divider::Split(..) => IDC_SIZENS,
        };
        // SAFETY: a system cursor, shared and never freed.
        unsafe { SetCursor(LoadCursorW(core::ptr::null_mut(), shape)) };
        true
    }

    /// The left button went down at `pt`: on a divider, its drag starts.
    pub(super) fn divider_down(&self, pt: POINT) -> bool {
        let Some(divider) = self.divider_at(pt) else {
            return false;
        };
        let r = self.regions.get();
        let heights: Vec<i32> = match divider {
            Divider::Split(side, _) => self.docks.borrow().placed.panes[side_index(side)]
                .iter()
                .map(|(_, rc)| rc.bottom - rc.top)
                .collect(),
            Divider::Dock(_) => Vec::new(),
        };
        let width = match divider {
            Divider::Dock(Side::Left) => r.left.right - r.left.left,
            Divider::Dock(Side::Right) => r.right.right - r.right.left,
            Divider::Split(..) => 0,
        };
        self.docks.borrow_mut().drag = Some(Drag {
            divider,
            start: pt,
            width,
            heights,
        });
        // SAFETY: the mouse stays with this window until the drag ends.
        unsafe { SetCapture(self.hwnd) };
        self.divider_cursor();
        true
    }

    /// The mouse moved to `pt` during a drag: the dock or panes follow.
    pub(super) fn divider_move(&self, pt: POINT) -> bool {
        if !self.dragging_divider() {
            return false;
        }
        // Measured before the docks are borrowed to change them: each of
        // these reads them too.
        let dpi = self.dpi.get();
        let r = self.regions.get();
        let (min_dock, min_center) = self.dock_limits();
        let divider_w = self.scale(dock::DIVIDER);
        let title = pane::title_height(self);
        let changed = {
            let mut d = self.docks.borrow_mut();
            let Some(drag) = d.drag.as_ref() else {
                return false;
            };
            match drag.divider {
                Divider::Dock(side) => {
                    // As wide as leaves the results their least, beside the
                    // other dock as it shows.
                    let other = match side {
                        Side::Left => r.right.right - r.right.left,
                        Side::Right => r.left.right - r.left.left,
                    };
                    let other_divider = if other > 0 { divider_w } else { 0 };
                    let max = (r.main.right - r.main.left)
                        - min_center
                        - divider_w
                        - other
                        - other_divider;
                    let w = dock::drag_width(
                        side,
                        drag.width,
                        pt.x - drag.start.x,
                        min_dock,
                        max.max(min_dock),
                    );
                    let wide = columns::unscale(w, dpi).max(d.settings.min_wide);
                    let slot = match side {
                        Side::Left => &mut d.settings.left_wide,
                        Side::Right => &mut d.settings.right_wide,
                    };
                    let changed = *slot != wide;
                    *slot = wide;
                    changed
                }
                Divider::Split(side, i) => {
                    let start = drag.start;
                    let heights = drag.heights.clone();
                    let open: Vec<(PaneId, i32)> = d
                        .open_on(side)
                        .iter()
                        .map(|p| (p.id, title + self.scale(p.min_high)))
                        .collect();
                    if open.len() != heights.len() {
                        // A pane came or went meanwhile: nothing to follow.
                        return true;
                    }
                    let mins: Vec<i32> = open.iter().map(|&(_, m)| m).collect();
                    let new = dock::drag_split(&heights, &mins, i, pt.y - start.y);
                    let mut changed = false;
                    // The first pane takes the rest: the others keep what
                    // they show now.
                    for (&(id, _), &h) in open.iter().zip(&new).skip(1) {
                        let high = columns::unscale(h, dpi);
                        if let Some(p) = d.get_mut(id)
                            && p.layout.high != high
                        {
                            p.layout.high = high;
                            changed = true;
                        }
                    }
                    changed
                }
            }
        };
        if changed {
            self.layout();
        }
        true
    }

    /// The drag ended (the button came up, or the mouse went elsewhere).
    pub(super) fn divider_up(&self) -> bool {
        let ended = self.docks.borrow_mut().drag.take().is_some();
        if ended {
            // SAFETY: lets go of the mouse this window took for the drag.
            unsafe {
                if GetCapture() == self.hwnd {
                    ReleaseCapture();
                }
            }
            self.invalidate_main_area();
            self.settings_changed();
        }
        ended
    }

    /// Whether a divider is being dragged.
    pub(super) fn dragging_divider(&self) -> bool {
        self.docks.borrow().drag.is_some()
    }

    // ---- The View menu ----

    /// Puts the registered panes' items in the View menu (Everything's,
    /// in its order, a group of their own after Status Bar).
    fn update_pane_menu(&self) {
        let mut items: Vec<(u32, u16, &'static str)> = self
            .docks
            .borrow()
            .panes
            .iter()
            .filter_map(|p| {
                p.id.menu_item()
                    .map(|(cmd, label)| (p.id.menu_rank(), cmd, label))
            })
            .collect();
        items.sort_by_key(|&(rank, ..)| rank);
        let items: Vec<(u16, &str)> = items.into_iter().map(|(_, c, l)| (c, l)).collect();
        menu::set_pane_items(self.menus.view, &items);
        self.update_pane_menu_checks();
    }

    /// Checks the View menu's pane items of the open panes.
    fn update_pane_menu_checks(&self) {
        self.check_pane_items(self.menus.view);
    }

    /// Checks pane items in `popup` (a menu about to open).
    pub(super) fn check_pane_items(&self, popup: HMENU) {
        let d = self.docks.borrow();
        for p in &d.panes {
            if let Some((cmd, _)) = p.id.menu_item() {
                menu::check_item(popup, cmd, p.open.is_some());
            }
        }
    }

    /// A View menu pane command: the pane opens or closes. False for any
    /// other command.
    pub(super) fn pane_command(&self, cmd: u16) -> bool {
        match cmd {
            menu::CLOSE_PANE => self.close_focused_pane(),
            menu::NEXT_FRAME => self.cycle_frame(true, false),
            menu::PREV_FRAME => self.cycle_frame(false, false),
            _ => {
                let Some(id) = PaneId::of_command(cmd) else {
                    return false;
                };
                if self.pane_registered(id) {
                    self.toggle_pane(id);
                }
            }
        }
        true
    }

    // ---- The keyboard between frames ----

    /// The frame `hwnd` (the keyboard's window) is in.
    pub(super) fn frame_of(&self, hwnd: HWND) -> Option<Frame> {
        if hwnd.is_null() {
            return None;
        }
        if hwnd == self.edit {
            return Some(Frame::Search);
        }
        if hwnd == self.list {
            return Some(Frame::List);
        }
        {
            let d = self.docks.borrow();
            for p in &d.panes {
                if let Some(o) = &p.open
                    // SAFETY: asks about this thread's windows.
                    && (o.hwnd == hwnd || unsafe { IsChild(o.hwnd, hwnd) } != 0)
                {
                    return Some(Frame::Pane(p.id));
                }
            }
        }
        for slot in [StripSlot::Tabs, StripSlot::FindBar] {
            if self.strip(slot).is_some_and(|s| s.contains(hwnd)) {
                return Some(Frame::Strip(slot));
            }
        }
        None
    }

    /// The frames in F6's order: the search box, the left dock's panes
    /// top to bottom, the results, the find bar, the right dock's panes.
    /// `tab`: only those Tab stops at (the search box, the results, and
    /// the panes whose `_tab_stop` is set).
    fn frames(&self, tab: bool) -> Vec<Frame> {
        let mut out = vec![Frame::Search];
        let d = self.docks.borrow();
        let panes = |side: Side, out: &mut Vec<Frame>| {
            for p in d.open_on(side) {
                if !tab || p.layout.tab_stop {
                    out.push(Frame::Pane(p.id));
                }
            }
        };
        panes(Side::Left, &mut out);
        out.push(Frame::List);
        if !tab
            && self
                .strip(StripSlot::FindBar)
                .is_some_and(|s| s.focus().is_some())
        {
            out.push(Frame::Strip(StripSlot::FindBar));
        }
        panes(Side::Right, &mut out);
        out
    }

    /// Moves the keyboard to the next (`forward`) or previous frame: all
    /// of them for F6, the tab stops for Tab (`tab`).
    pub(super) fn cycle_frame(&self, forward: bool, tab: bool) {
        // SAFETY: reads this thread's focus.
        let current = self.frame_of(unsafe { GetFocus() });
        let all = self.frames(false);
        let stops = if tab { self.frames(true) } else { all.clone() };
        let next = next_frame(&all, &stops, current, forward);
        if let Some(frame) = next {
            self.focus_frame(frame, !tab);
        }
    }

    /// Gives `frame` the keyboard. `select`: the search box's text is
    /// selected, as Alt+D does (F6).
    fn focus_frame(&self, frame: Frame, select: bool) {
        match frame {
            Frame::Search => {
                // SAFETY: focuses a child of this thread's window.
                unsafe { SetFocus(self.edit) };
                if select {
                    super::send(self.edit, EM_SETSEL, 0, -1);
                }
            }
            Frame::List => self.focus_list(),
            Frame::Pane(id) => self.focus_pane(id),
            Frame::Strip(slot) => {
                if let Some(h) = self.strip(slot).and_then(|s| s.focus()) {
                    // SAFETY: focuses a window of this thread.
                    unsafe { SetFocus(h) };
                }
            }
        }
    }

    /// Tab (`!back`) or Shift+Tab in `hwnd`: within a pane or strip that
    /// takes it, else to the next tab stop. False when `hwnd` is in no
    /// frame (another window of the thread, a rename box).
    pub(super) fn tab_key(&self, hwnd: HWND, back: bool) -> bool {
        let Some(frame) = self.frame_of(hwnd) else {
            return false;
        };
        let handled = match frame {
            Frame::Pane(id) => self
                .open_of(id)
                .is_some_and(|o| o.content.tab(self, hwnd, !back)),
            Frame::Strip(slot) => self.strip(slot).is_some_and(|s| s.tab(self, hwnd, !back)),
            Frame::Search | Frame::List => false,
        };
        if !handled {
            self.cycle_frame(!back, true);
        }
        true
    }

    /// A key message from the window's loop, for the pane or strip whose
    /// window it is for (`PaneContent::key`, `Strip::key`): true when it
    /// took it.
    pub(super) fn frame_key(&self, msg: &MSG) -> bool {
        match self.frame_of(msg.hwnd) {
            Some(Frame::Pane(id)) => self.open_of(id).is_some_and(|o| o.content.key(self, msg)),
            Some(Frame::Strip(slot)) => self.strip(slot).is_some_and(|s| s.key(self, msg)),
            _ => false,
        }
    }

    /// Whether `hwnd` is a text box of a pane or a strip (Edit > Cut, Copy
    /// and Paste then act on its text).
    pub(super) fn frame_text_box(&self, hwnd: HWND) -> bool {
        if !matches!(self.frame_of(hwnd), Some(Frame::Pane(_) | Frame::Strip(_))) {
            return false;
        }
        let mut class = [0u16; 16];
        // SAFETY: the buffer and its length.
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
        String::from_utf16_lossy(&class[..n.max(0) as usize]).eq_ignore_ascii_case("Edit")
    }

    /// Shift+Esc: closes the pane with the keyboard, which goes to the
    /// results.
    fn close_focused_pane(&self) {
        // SAFETY: reads this thread's focus.
        if let Some(Frame::Pane(id)) = self.frame_of(unsafe { GetFocus() }) {
            self.show_pane(id, false);
        }
    }

    /// The window to give the keyboard back to as it is activated again:
    /// `last` if it is still one of the window's own, else the search box.
    pub(super) fn focus_to_restore(&self, last: HWND) -> HWND {
        // SAFETY: asks about this thread's windows.
        let alive = unsafe { IsWindow(last) != 0 && IsChild(self.hwnd, last) != 0 };
        if alive { last } else { self.edit }
    }
}

/// The frame after (`forward`) or before `current` in `all` that is one of
/// `stops`, going round; from nowhere, the first (or last) stop.
pub(super) fn next_frame(
    all: &[Frame],
    stops: &[Frame],
    current: Option<Frame>,
    forward: bool,
) -> Option<Frame> {
    if stops.is_empty() {
        return None;
    }
    let Some(at) = current.and_then(|c| all.iter().position(|&f| f == c)) else {
        return Some(if forward {
            stops[0]
        } else {
            stops[stops.len() - 1]
        });
    };
    let n = all.len();
    (1..=n)
        .map(|k| {
            if forward {
                all[(at + k) % n]
            } else {
                all[(at + n - k % n) % n]
            }
        })
        .find(|f| stops.contains(f))
}

fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// Windows moved together, so they repaint once and in step.
pub(super) struct Defer {
    moves: Vec<(HWND, RECT)>,
}

impl Defer {
    pub(super) fn new() -> Defer {
        Defer { moves: Vec::new() }
    }

    pub(super) fn put(&mut self, hwnd: HWND, rc: RECT) {
        if !hwnd.is_null() {
            self.moves.push((hwnd, rc));
        }
    }

    /// Moves them all.
    pub(super) fn run(self) {
        if self.moves.is_empty() {
            return;
        }
        // SAFETY: moves this thread's child windows; a failed deferral
        // falls back to moving each on its own.
        unsafe {
            let mut hdwp = BeginDeferWindowPos(self.moves.len() as i32);
            for &(hwnd, rc) in &self.moves {
                if hdwp.is_null() {
                    break;
                }
                hdwp = DeferWindowPos(
                    hdwp,
                    hwnd,
                    core::ptr::null_mut(),
                    rc.left,
                    rc.top,
                    (rc.right - rc.left).max(0),
                    (rc.bottom - rc.top).max(0),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            if hdwp.is_null() || EndDeferWindowPos(hdwp) == 0 {
                use windows_sys::Win32::UI::WindowsAndMessaging::MoveWindow;
                for &(hwnd, rc) in &self.moves {
                    MoveWindow(
                        hwnd,
                        rc.left,
                        rc.top,
                        (rc.right - rc.left).max(0),
                        (rc.bottom - rc.top).max(0),
                        1,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f6_goes_round_every_frame_and_tab_round_the_stops() {
        let a = PaneId::Test(0);
        let b = PaneId::Test(1);
        let c = PaneId::Test(2);
        // Search, two left panes, the results, the find bar, a right pane.
        let all = [
            Frame::Search,
            Frame::Pane(a),
            Frame::Pane(b),
            Frame::List,
            Frame::Strip(StripSlot::FindBar),
            Frame::Pane(c),
        ];
        let f6 = |from, fwd| next_frame(&all, &all, from, fwd);
        assert_eq!(f6(Some(Frame::Search), true), Some(Frame::Pane(a)));
        assert_eq!(f6(Some(Frame::Pane(b)), true), Some(Frame::List));
        assert_eq!(f6(Some(Frame::Pane(c)), true), Some(Frame::Search));
        assert_eq!(f6(Some(Frame::Search), false), Some(Frame::Pane(c)));
        assert_eq!(f6(Some(Frame::List), false), Some(Frame::Pane(b)));
        // From nowhere (another window): the first, or the last.
        assert_eq!(f6(None, true), Some(Frame::Search));
        assert_eq!(f6(None, false), Some(Frame::Pane(c)));
        // Tab stops: the search box, pane b and the results.
        let stops = [Frame::Search, Frame::Pane(b), Frame::List];
        let tab = |from, fwd| next_frame(&all, &stops, from, fwd);
        assert_eq!(tab(Some(Frame::Search), true), Some(Frame::Pane(b)));
        assert_eq!(tab(Some(Frame::List), true), Some(Frame::Search));
        assert_eq!(tab(Some(Frame::Search), false), Some(Frame::List));
        // From a frame that is no stop: the next stop after it.
        assert_eq!(tab(Some(Frame::Pane(a)), true), Some(Frame::Pane(b)));
        assert_eq!(tab(Some(Frame::Pane(c)), true), Some(Frame::Search));
        assert_eq!(tab(Some(Frame::Pane(c)), false), Some(Frame::List));
        // No panes: Tab and F6 swap the search box and the results.
        let two = [Frame::Search, Frame::List];
        assert_eq!(
            next_frame(&two, &two, Some(Frame::Search), true),
            Some(Frame::List)
        );
        assert_eq!(
            next_frame(&two, &two, Some(Frame::List), true),
            Some(Frame::Search)
        );
        assert_eq!(
            next_frame(&two, &two, Some(Frame::List), false),
            Some(Frame::Search)
        );
    }
}
