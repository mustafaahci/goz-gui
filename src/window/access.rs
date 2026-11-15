//! Accessibility of what the main window and its panes paint themselves
//! (`crate::a11y`), from the window's side: the parts each has now, and
//! their actions.

use windows_sys::Win32::Foundation::{HWND, LRESULT, RECT, WPARAM};

use super::App;
use crate::a11y::{self, Action, Kind, Part, Server, Source};
use crate::searchbox::{self, Button};
use crate::strip::StripSlot;
use crate::win::{self, gdi};

/// The `App` behind a main window, for its object.
fn source_of_main(hwnd: HWND) -> Option<&'static dyn Source> {
    super::app_of(hwnd).map(|a| a as &dyn Source)
}

/// The `App` behind a pane window (a child of the main window).
fn source_of_pane(hwnd: HWND) -> Option<&'static dyn Source> {
    let parent = win::parent(hwnd);
    super::app_of(parent).map(|a| a as &dyn Source)
}

/// WM_GETOBJECT for a pane window's client area (`crate::pane`).
pub fn pane_object(hwnd: HWND, code: usize, w: WPARAM) -> Option<LRESULT> {
    Server::new(hwnd, Kind::Pane(code), source_of_pane).map(|s| s.answer(w))
}

impl App {
    /// WM_GETOBJECT for the main window's client area: its object, made
    /// once.
    pub(super) fn accessible_object(&self, w: WPARAM) -> Option<LRESULT> {
        if self.custom.accessible.borrow().is_none() {
            let server = Server::new(self.hwnd, Kind::Main, source_of_main)?;
            *self.custom.accessible.borrow_mut() = Some(server);
        }
        self.custom
            .accessible
            .borrow()
            .as_ref()
            .map(|s| s.answer(w))
    }

    /// The status bar's texts as last drawn (`status::paint`).
    pub fn set_status_parts(&self, parts: Vec<(String, RECT)>) {
        self.custom.set_status_parts(parts);
    }

    /// The status bar's texts as it would draw them now: drawn into a
    /// buffer of its size (a window off screen, or not repainted since its
    /// text changed, has drawn nothing).
    fn fresh_status_parts(&self) -> Vec<(String, RECT)> {
        let rc = self.status_rc.get();
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || h <= 0 {
            return Vec::new();
        }
        // A memory DC and bitmap of this thread's window, released here (the
        // guards drop in the reverse of their order); the status bar draws
        // into it at its own coordinates.
        {
            let dc = gdi::WindowDc::get(self.hwnd);
            let mem = gdi::MemDc::compatible(dc.hdc());
            let bmp = gdi::Owned::new(gdi::create_compatible_bitmap(dc.hdc(), w, h));
            let old = gdi::Selected::new(mem.hdc(), bmp.get());
            gdi::set_viewport_org(mem.hdc(), -rc.left, -rc.top);
            crate::status::paint(self, mem.hdc());
            drop(old);
            drop(bmp);
            drop(mem);
            drop(dc);
        }
        self.custom.status_parts()
    }

    fn main_parts(&self) -> Vec<Part> {
        let mut out = Vec::new();
        // The tab strip, while it shows.
        if self.tab_strip_shown()
            && let Some(strip) = self.strip(StripSlot::Tabs)
            && let Some(tabs) = strip.as_any().downcast_ref::<super::tabstrip::TabStrip>()
        {
            let (placed, plus) = tabs.tab_rects(self);
            let titles = self.tab_titles();
            let current = self.current_tab();
            let mut list = Part::new(
                "Tabs",
                a11y::ROLE_PAGETABLIST,
                self.strip_rect(StripSlot::Tabs),
            );
            list.state = a11y::STATE_READONLY;
            out.push(list);
            for (i, rc) in placed {
                let name = titles.get(i).cloned().unwrap_or_default();
                let mut p = Part::new(name, a11y::ROLE_PAGETAB, rc);
                p.state = a11y::STATE_SELECTABLE;
                if i == current {
                    p.state |= a11y::STATE_SELECTED;
                }
                p.action = Some(("Switch", Action::Tab(i)));
                out.push(p);
            }
            let mut p = Part::new("New Tab", a11y::ROLE_PUSHBUTTON, plus);
            p.action = Some(("Press", Action::Command(crate::tabs::NEW_TAB)));
            out.push(p);
        }
        // The search box's buttons, with their tooltips' names.
        let mut buttons: Vec<(Button, RECT, &str)> = Vec::new();
        if let Some((back, forward)) = searchbox::nav_of(self) {
            buttons.push((Button::Back, back, "Go back one search"));
            buttons.push((Button::Forward, forward, "Go forward one search"));
        }
        let f = searchbox::frame_of(self);
        if f.history.right > f.history.left {
            buttons.push((Button::History, f.history, "Search History"));
        }
        buttons.push((Button::Clear, f.clear, "Clear"));
        let empty = win::text_len(self.edit) == 0;
        for (b, rc, name) in buttons {
            let mut p = Part::new(name, a11y::ROLE_PUSHBUTTON, rc);
            if !searchbox::enabled(self, b) || (b == Button::Clear && empty) {
                p.state = a11y::STATE_UNAVAILABLE;
            }
            if self.search_hot.get() == Some(b) {
                p.state |= a11y::STATE_HOTTRACKED;
            }
            p.action = Some(("Press", Action::SearchButton(b)));
            out.push(p);
        }
        // The filter bar.
        if self.filter_bar.get() {
            let mut p = Part::new("Filter", a11y::ROLE_COMBOBOX, self.filter_rc.get());
            p.value = Some(self.opts.get().filter.title().to_owned());
            out.push(p);
        }
        // The status bar: the bar with all its text, each text, and the
        // view buttons.
        if self.status_bar_visible() {
            let texts = self.fresh_status_parts();
            let mut bar = Part::new("Status Bar", a11y::ROLE_STATUSBAR, self.status_rc.get());
            bar.value = Some(
                texts
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            bar.state = a11y::STATE_READONLY;
            out.push(bar);
            for (text, rc) in texts {
                let mut p = Part::new(text, a11y::ROLE_STATICTEXT, rc);
                p.state = a11y::STATE_READONLY;
                out.push(p);
            }
            let thumbs = self.view.get().thumbnails;
            for (b, rc) in crate::status::view_buttons_now(self).into_iter().flatten() {
                let t = b == crate::status::ViewButton::Thumbnails;
                let name = if t { "Thumbnails" } else { "Details" };
                let mut p = Part::new(name, a11y::ROLE_PUSHBUTTON, rc);
                if t == thumbs {
                    p.state = a11y::STATE_SELECTED;
                }
                p.action = Some(("Press", Action::Thumbnails(t)));
                out.push(p);
            }
        }
        out
    }
}

impl Source for App {
    fn parts(&self, kind: Kind, hwnd: HWND) -> Vec<Part> {
        match kind {
            Kind::Main => self.main_parts(),
            Kind::Pane(_) => crate::pane::accessible_parts(self, hwnd),
        }
    }

    fn own(&self, kind: Kind) -> Option<(String, u32)> {
        match kind {
            Kind::Main => None,
            Kind::Pane(code) => crate::dock::PaneId::from_code(code)
                .map(|id| (id.title().to_owned(), a11y::ROLE_PANE)),
        }
    }

    fn act(&self, kind: Kind, _hwnd: HWND, action: Action) {
        match action {
            Action::Command(cmd) => self.command(cmd),
            Action::SearchButton(b) => self.search_button(b),
            Action::Tab(i) => self.select_tab(i),
            Action::Thumbnails(t) => self.view_button(if t {
                crate::status::ViewButton::Thumbnails
            } else {
                crate::status::ViewButton::Details
            }),
            Action::ClosePane => {
                if let Kind::Pane(code) = kind {
                    // Posted to this thread's window: the pane goes from the
                    // window's own queue, not inside the call.
                    win::post_app(self.hwnd, super::WM_PANE_CLOSE, code, 0);
                }
            }
        }
    }
}
