//! Accessibility (MSAA, which UI Automation and Narrator read through
//! their proxy) for what goz-gui paints itself: the search box's buttons,
//! the filter bar, the tab strip, the status bar's parts, and each pane's
//! title row with its close button.
//!
//! A window that has such parts answers WM_GETOBJECT for its client area
//! with an `IAccessible` of its own that wraps the system's standard one
//! (`win::a11y::AccessibleObject`): the window itself and its child
//! windows (the search box, the results list and its header, a pane's
//! tree or list, which have accessibility of their own) are the standard
//! object's, and the painted parts are added after them as simple
//! elements (child ids past the standard children), with a name, a role, a
//! state, a place on screen, a value and a default action.
//!
//! The object runs on the UI thread (COM calls reach the window's
//! apartment through its message loop) and asks the window for its parts
//! each time, so it never holds anything of the window between calls.

use windows_sys::Win32::Foundation::RECT;

pub use crate::win::a11y::wants_client;
use crate::win::a11y::{AccessibleObject, Element, Elements};

/// MSAA roles (oleacc.h).
pub const ROLE_PANE: u32 = 16;
pub const ROLE_STATUSBAR: u32 = 23;
pub const ROLE_PAGETAB: u32 = 37;
pub const ROLE_STATICTEXT: u32 = 41;
pub const ROLE_PUSHBUTTON: u32 = 43;
pub const ROLE_COMBOBOX: u32 = 46;
pub const ROLE_PAGETABLIST: u32 = 60;
/// MSAA states.
pub const STATE_UNAVAILABLE: u32 = 0x1;
pub const STATE_SELECTED: u32 = 0x2;
pub const STATE_READONLY: u32 = 0x40;
pub const STATE_HOTTRACKED: u32 = 0x80;
pub const STATE_SELECTABLE: u32 = 0x20_0000;

/// What doing a part's default action does; the window runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A command of the window (`App::command`).
    Command(u16),
    /// A search box button.
    SearchButton(crate::searchbox::Button),
    /// Show tab `n`.
    Tab(usize),
    /// Close the pane whose window this is.
    ClosePane,
    /// The status bar's view buttons: thumbnails or details.
    Thumbnails(bool),
}

/// One painted part of a window, as accessibility tools see it.
#[derive(Clone)]
pub struct Part {
    pub name: String,
    pub role: u32,
    pub state: u32,
    /// In the window's client coordinates.
    pub rect: RECT,
    pub value: Option<String>,
    pub description: Option<String>,
    /// Its default action's name ("Press", "Switch") and what it does.
    pub action: Option<(&'static str, Action)>,
}

impl Part {
    pub fn new(name: impl Into<String>, role: u32, rect: RECT) -> Part {
        Part {
            name: name.into(),
            role,
            state: 0,
            rect,
            value: None,
            description: None,
            action: None,
        }
    }
}

/// Which window an object speaks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Main,
    /// A pane (`dock::PaneId::code`).
    Pane(usize),
}

/// What a window's object asks of the window.
pub trait Source {
    /// Its painted parts now.
    fn parts(&self, kind: Kind, hwnd: windows_sys::Win32::Foundation::HWND) -> Vec<Part>;
    /// The window's own name and role, instead of the standard object's.
    fn own(&self, kind: Kind) -> Option<(String, u32)>;
    /// Runs a part's default action.
    fn act(&self, kind: Kind, hwnd: windows_sys::Win32::Foundation::HWND, action: Action);
}

/// The window's object, kept by the window while it lives so every
/// WM_GETOBJECT hands out the same one.
pub struct Server {
    object: AccessibleObject,
}

/// What the object asks of its window, answered by the window's
/// [`Source`], found again on every call.
struct Window {
    hwnd: windows_sys::Win32::Foundation::HWND,
    kind: Kind,
    source: fn(windows_sys::Win32::Foundation::HWND) -> Option<&'static dyn Source>,
}

impl Window {
    fn source(&self) -> Option<&'static dyn Source> {
        (self.source)(self.hwnd)
    }

    fn parts(&self) -> Vec<Part> {
        self.source()
            .map(|s| s.parts(self.kind, self.hwnd))
            .unwrap_or_default()
    }
}

impl Elements for Window {
    fn elements(&self) -> Vec<Element> {
        self.parts()
            .into_iter()
            .map(|p| Element {
                name: p.name,
                role: p.role,
                state: p.state,
                rect: p.rect,
                value: p.value,
                description: p.description,
                action: p.action.map(|(name, _)| name),
            })
            .collect()
    }

    fn own(&self) -> Option<(String, u32)> {
        self.source().and_then(|s| s.own(self.kind))
    }

    fn act(&self, index: usize) {
        let Some((_, action)) = self.parts().into_iter().nth(index).and_then(|p| p.action) else {
            return;
        };
        if let Some(s) = self.source() {
            s.act(self.kind, self.hwnd, action);
        }
    }
}

impl Server {
    /// The object for window `hwnd` of `kind`; `source` finds who answers
    /// for it (the window's `App`) each time it is asked.
    pub fn new(
        hwnd: windows_sys::Win32::Foundation::HWND,
        kind: Kind,
        source: fn(windows_sys::Win32::Foundation::HWND) -> Option<&'static dyn Source>,
    ) -> Option<Server> {
        let object = AccessibleObject::new(hwnd, Window { hwnd, kind, source })?;
        Some(Server { object })
    }

    /// WM_GETOBJECT's answer for the client area, given its wParam.
    pub fn answer(&self, w: usize) -> isize {
        self.object.answer_w(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_starts_plain() {
        let p = Part::new("Clear", ROLE_PUSHBUTTON, RECT::default());
        assert_eq!(p.state, 0);
        assert!(wants_client(-4));
        assert!(!wants_client(-3));
    }
}
