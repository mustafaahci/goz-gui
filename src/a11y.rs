//! Accessibility (MSAA, which UI Automation and Narrator read through
//! their proxy) for what goz-gui paints itself: the search box's buttons,
//! the filter bar, the tab strip, the status bar's parts, and each pane's
//! title row with its close button.
//!
//! A window that has such parts answers WM_GETOBJECT for its client area
//! with an [`IAccessible`] of its own that wraps the system's standard one
//! ([`CreateStdAccessibleObject`]): the window itself and its child
//! windows (the search box, the results list and its header, a pane's
//! tree or list, which have accessibility of their own) are the standard
//! object's, and the painted parts are added after them as simple
//! elements (child ids past the standard children), with a name, a role, a
//! state, a place on screen, a value and a default action.
//!
//! The object runs on the UI thread (COM calls reach the window's
//! apartment through its message loop) and asks the window for its parts
//! each time, so it never holds anything of the window between calls.

use windows::Win32::Foundation::{E_INVALIDARG, HWND, LRESULT, S_FALSE, WPARAM};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, ITypeInfo,
};
use windows::Win32::System::Variant::{VARIANT, VT_EMPTY, VT_I4};
use windows::Win32::UI::Accessibility::{
    CreateStdAccessibleObject, IAccessible, IAccessible_Impl, LresultFromObject,
};
use windows::core::{BSTR, GUID, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;

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

const OBJID_CLIENT: i32 = -4;
const NAVDIR_NEXT: i32 = 5;
const NAVDIR_PREVIOUS: i32 = 6;

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
    object: IAccessible,
}

// Not agile: calls from other processes come through the UI thread's
// apartment, where the window and its `App` are.
#[implement(IAccessible, Agile = false)]
struct Accessible {
    hwnd: windows_sys::Win32::Foundation::HWND,
    kind: Kind,
    standard: IAccessible,
    source: fn(windows_sys::Win32::Foundation::HWND) -> Option<&'static dyn Source>,
}

/// Which child a VARIANT names.
enum Child {
    Me,
    Standard,
    Part(usize),
}

fn variant_i4(v: i32) -> VARIANT {
    let mut out = VARIANT::default();
    // SAFETY: writes the VT_I4 member of a fresh VARIANT.
    unsafe {
        let inner = &mut *out.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = v;
    }
    out
}

fn child_id(v: &VARIANT) -> Option<i32> {
    // SAFETY: reads the tag, then the member it says is there.
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        if inner.vt == VT_I4 {
            Some(inner.Anonymous.lVal)
        } else if inner.vt == VT_EMPTY {
            Some(0)
        } else {
            None
        }
    }
}

fn bstr(s: &str) -> BSTR {
    BSTR::from(s)
}

/// Windows' "not supported here" for a part without the property.
fn none<T>() -> windows::core::Result<T> {
    Err(windows::core::Error::from_hresult(S_FALSE))
}

impl Accessible {
    fn source(&self) -> Option<&'static dyn Source> {
        (self.source)(self.hwnd)
    }

    fn parts(&self) -> Vec<Part> {
        self.source()
            .map(|s| s.parts(self.kind, self.hwnd))
            .unwrap_or_default()
    }

    fn standard_count(&self) -> i32 {
        // SAFETY: a call on the standard object of this thread's window.
        unsafe { self.standard.accChildCount() }.unwrap_or(0)
    }

    fn which(&self, v: &VARIANT) -> windows::core::Result<Child> {
        let id = child_id(v).ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))?;
        if id == 0 {
            return Ok(Child::Me);
        }
        let n = self.standard_count();
        if id <= n {
            return Ok(Child::Standard);
        }
        let i = (id - n - 1) as usize;
        if i < self.parts().len() {
            Ok(Child::Part(i))
        } else {
            Err(windows::core::Error::from_hresult(E_INVALIDARG))
        }
    }

    fn part(&self, i: usize) -> windows::core::Result<Part> {
        self.parts()
            .into_iter()
            .nth(i)
            .ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))
    }

    fn screen(&self, rc: RECT) -> RECT {
        let mut tl = POINT {
            x: rc.left,
            y: rc.top,
        };
        // SAFETY: maps a point of this thread's window to the screen.
        unsafe { ClientToScreen(self.hwnd, &mut tl) };
        RECT {
            left: tl.x,
            top: tl.y,
            right: tl.x + (rc.right - rc.left),
            bottom: tl.y + (rc.bottom - rc.top),
        }
    }
}

impl IDispatch_Impl for Accessible_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _names: *const PCWSTR,
        _count: u32,
        _lcid: u32,
        _ids: *mut i32,
    ) -> windows::core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn Invoke(
        &self,
        _id: i32,
        _riid: *const GUID,
        _lcid: u32,
        _flags: DISPATCH_FLAGS,
        _params: *const DISPPARAMS,
        _result: *mut VARIANT,
        _excep: *mut EXCEPINFO,
        _arg: *mut u32,
    ) -> windows::core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }
}

// SAFETY (for every `unsafe` below): calls on the standard object of this
// thread's window, with VARIANTs that outlive them.
impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> windows::core::Result<IDispatch> {
        unsafe { self.standard.accParent() }
    }

    fn accChildCount(&self) -> windows::core::Result<i32> {
        Ok(self.standard_count() + self.parts().len() as i32)
    }

    fn get_accChild(&self, v: &VARIANT) -> windows::core::Result<IDispatch> {
        match self.which(v)? {
            Child::Standard => unsafe { self.standard.get_accChild(v) },
            // A simple element: no object of its own.
            Child::Part(_) | Child::Me => none(),
        }
    }

    fn get_accName(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => Ok(bstr(&self.part(i)?.name)),
            Child::Me => match self.source().and_then(|s| s.own(self.kind)) {
                Some((name, _)) => Ok(bstr(&name)),
                None => unsafe { self.standard.get_accName(v) },
            },
            Child::Standard => unsafe { self.standard.get_accName(v) },
        }
    }

    fn get_accValue(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.value {
                Some(value) => Ok(bstr(&value)),
                None => none(),
            },
            _ => unsafe { self.standard.get_accValue(v) },
        }
    }

    fn get_accDescription(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.description {
                Some(d) => Ok(bstr(&d)),
                None => none(),
            },
            _ => unsafe { self.standard.get_accDescription(v) },
        }
    }

    fn get_accRole(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.role as i32)),
            Child::Me => match self.source().and_then(|s| s.own(self.kind)) {
                Some((_, role)) => Ok(variant_i4(role as i32)),
                None => unsafe { self.standard.get_accRole(v) },
            },
            Child::Standard => unsafe { self.standard.get_accRole(v) },
        }
    }

    fn get_accState(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.state as i32)),
            _ => unsafe { self.standard.get_accState(v) },
        }
    }

    fn get_accHelp(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => unsafe { self.standard.get_accHelp(v) },
        }
    }

    fn get_accHelpTopic(&self, file: *mut BSTR, v: &VARIANT) -> windows::core::Result<i32> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => unsafe { self.standard.get_accHelpTopic(file, v) },
        }
    }

    fn get_accKeyboardShortcut(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => unsafe { self.standard.get_accKeyboardShortcut(v) },
        }
    }

    fn accFocus(&self) -> windows::core::Result<VARIANT> {
        unsafe { self.standard.accFocus() }
    }

    fn accSelection(&self) -> windows::core::Result<VARIANT> {
        unsafe { self.standard.accSelection() }
    }

    fn get_accDefaultAction(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.action {
                Some((name, _)) => Ok(bstr(name)),
                None => none(),
            },
            _ => unsafe { self.standard.get_accDefaultAction(v) },
        }
    }

    fn accSelect(&self, flags: i32, v: &VARIANT) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => unsafe { self.standard.accSelect(flags, v) },
        }
    }

    fn accLocation(
        &self,
        x: *mut i32,
        y: *mut i32,
        cx: *mut i32,
        cy: *mut i32,
        v: &VARIANT,
    ) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(i) => {
                let rc = self.screen(self.part(i)?.rect);
                if x.is_null() || y.is_null() || cx.is_null() || cy.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                // SAFETY: the caller's out-parameters, checked above.
                unsafe {
                    *x = rc.left;
                    *y = rc.top;
                    *cx = rc.right - rc.left;
                    *cy = rc.bottom - rc.top;
                }
                Ok(())
            }
            _ => unsafe { self.standard.accLocation(x, y, cx, cy, v) },
        }
    }

    fn accNavigate(&self, dir: i32, start: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(start)? {
            Child::Part(i) => {
                let n = self.standard_count();
                let count = self.parts().len();
                let to = match dir {
                    NAVDIR_NEXT if i + 1 < count => i + 1,
                    NAVDIR_PREVIOUS if i > 0 => i - 1,
                    _ => return none(),
                };
                Ok(variant_i4(n + 1 + to as i32))
            }
            _ => unsafe { self.standard.accNavigate(dir, start) },
        }
    }

    fn accHitTest(&self, x: i32, y: i32) -> windows::core::Result<VARIANT> {
        let n = self.standard_count();
        for (i, p) in self.parts().iter().enumerate() {
            let rc = self.screen(p.rect);
            if x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom {
                return Ok(variant_i4(n + 1 + i as i32));
            }
        }
        unsafe { self.standard.accHitTest(x, y) }
    }

    fn accDoDefaultAction(&self, v: &VARIANT) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(i) => {
                let Some((_, action)) = self.part(i)?.action else {
                    return none();
                };
                if self.part(i)?.state & STATE_UNAVAILABLE != 0 {
                    return Err(windows::Win32::Foundation::E_FAIL.into());
                }
                if let Some(s) = self.source() {
                    s.act(self.kind, self.hwnd, action);
                }
                Ok(())
            }
            _ => unsafe { self.standard.accDoDefaultAction(v) },
        }
    }

    fn put_accName(&self, _v: &VARIANT, _name: &BSTR) -> windows::core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn put_accValue(&self, _v: &VARIANT, _value: &BSTR) -> windows::core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
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
        let mut raw = core::ptr::null_mut();
        // SAFETY: asks for the standard client object of this thread's
        // window; the pointer comes back owned.
        let standard = unsafe {
            CreateStdAccessibleObject(HWND(hwnd), OBJID_CLIENT, &IAccessible::IID, &mut raw)
                .ok()?;
            IAccessible::from_raw(raw)
        };
        let object: IAccessible = Accessible {
            hwnd,
            kind,
            standard,
            source,
        }
        .into();
        Some(Server { object })
    }

    /// WM_GETOBJECT's answer for the client area.
    pub fn answer(&self, w: usize) -> isize {
        // SAFETY: hands out a reference to the object, as WM_GETOBJECT
        // asks.
        let r: LRESULT = unsafe { LresultFromObject(&IAccessible::IID, WPARAM(w), &self.object) };
        r.0
    }
}

/// Whether WM_GETOBJECT's `l` asks for the client area.
pub fn wants_client(l: isize) -> bool {
    l as i32 == OBJID_CLIENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_ids_read_from_variants() {
        assert_eq!(child_id(&variant_i4(3)), Some(3));
        assert_eq!(child_id(&VARIANT::default()), Some(0));
        let p = Part::new("Clear", ROLE_PUSHBUTTON, RECT::default());
        assert_eq!(p.state, 0);
        assert!(wants_client(-4));
        assert!(!wants_client(-3));
    }
}
