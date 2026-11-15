//! Accessibility (MSAA) for what a window paints itself: an `IAccessible`
//! that wraps the system's standard one for the window's client area and
//! adds the painted parts as simple elements after its children.
//!
//! The object is a private type; what is handed out is an
//! [`AccessibleObject`], configured with a Rust value ([`Elements`]) that
//! it asks for the parts each time. Not agile: calls from other processes
//! come through the window's thread's apartment, where that value lives.

use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, HWND as WHWND, LRESULT as WLRESULT, S_FALSE, WPARAM as WWPARAM,
};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, ITypeInfo,
};
use windows::Win32::System::Variant::{VARIANT, VT_EMPTY, VT_I4};
use windows::Win32::UI::Accessibility::{
    CreateStdAccessibleObject, IAccessible, IAccessible_Impl, LresultFromObject,
};
use windows::core::{BSTR, GUID, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};

const OBJID_CLIENT: i32 = -4;
const NAVDIR_NEXT: i32 = 5;
const NAVDIR_PREVIOUS: i32 = 6;
/// STATE_SYSTEM_UNAVAILABLE.
const STATE_UNAVAILABLE: u32 = 0x1;

/// One painted part of a window, as accessibility tools see it: a simple
/// element (no object of its own).
#[derive(Clone)]
pub struct Element {
    /// Its name ("Clear", a tab's title).
    pub name: String,
    /// Its MSAA role (ROLE_SYSTEM_*).
    pub role: u32,
    /// Its MSAA state (STATE_SYSTEM_*).
    pub state: u32,
    /// In the window's client coordinates.
    pub rect: RECT,
    /// Its value (a status bar part's text), if it has one.
    pub value: Option<String>,
    /// Its description, if it has one.
    pub description: Option<String>,
    /// Its default action's name ("Press"), if it has one.
    pub action: Option<&'static str>,
}

/// What a window's accessible object asks of the window, each time it is
/// asked (it keeps nothing of the window between calls).
pub trait Elements {
    /// The painted parts now, in order.
    fn elements(&self) -> Vec<Element>;
    /// The window's own name and role, instead of the standard object's.
    fn own(&self) -> Option<(String, u32)>;
    /// Does the default action of part `index` (one that has an action
    /// and is not unavailable).
    fn act(&self, index: usize);
}

/// A VARIANT holding `v` (VT_I4).
fn variant_i4(v: i32) -> VARIANT {
    let mut out = VARIANT::default();
    // SAFETY: writes the VT_I4 member of a fresh VARIANT and the tag that
    // says it is the one there.
    unsafe {
        let inner = &mut *out.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = v;
    }
    out
}

/// The child id a VARIANT holds (VT_I4; VT_EMPTY is 0, the object itself).
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

/// Which child a VARIANT names.
enum Child {
    Me,
    Standard,
    Part(usize),
}

fn bstr(s: &str) -> BSTR {
    BSTR::from(s)
}

/// Windows' "not supported here" for a part without the property.
fn none<T>() -> windows::core::Result<T> {
    Err(windows::core::Error::from_hresult(S_FALSE))
}

/// A window's object: the system's standard one for the window, its
/// children after it, with the painted parts added as simple elements past
/// the standard children. Not agile: calls from other processes come
/// through the window's thread's apartment, where its closures live.
#[implement(IAccessible, Agile = false)]
struct Accessible {
    hwnd: HWND,
    standard: IAccessible,
    elements: Box<dyn Elements>,
}

impl Accessible {
    fn parts(&self) -> Vec<Element> {
        self.elements.elements()
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

    fn part(&self, i: usize) -> windows::core::Result<Element> {
        self.parts()
            .into_iter()
            .nth(i)
            .ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))
    }

    fn screen(&self, rc: RECT) -> RECT {
        let tl = super::client_to_screen(
            self.hwnd,
            POINT {
                x: rc.left,
                y: rc.top,
            },
        );
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
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _names: *const PCWSTR,
        _count: u32,
        _lcid: u32,
        _ids: *mut i32,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
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
        Err(E_NOTIMPL.into())
    }
}

/// A call on the standard object of this thread's window, with the
/// VARIANTs and out-pointers the caller gave, passed on as they came.
macro_rules! standard {
    ($self:ident . $call:ident ( $($arg:expr),* )) => {{
        // SAFETY: a call on the standard object of this thread's window,
        // with what its caller gave, passed on as it came.
        unsafe { $self.standard.$call($($arg),*) }
    }};
}

impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> windows::core::Result<IDispatch> {
        standard!(self.accParent())
    }

    fn accChildCount(&self) -> windows::core::Result<i32> {
        Ok(self.standard_count() + self.parts().len() as i32)
    }

    fn get_accChild(&self, v: &VARIANT) -> windows::core::Result<IDispatch> {
        match self.which(v)? {
            Child::Standard => standard!(self.get_accChild(v)),
            // A simple element: no object of its own.
            Child::Part(_) | Child::Me => none(),
        }
    }

    fn get_accName(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => Ok(bstr(&self.part(i)?.name)),
            Child::Me => match self.elements.own() {
                Some((name, _)) => Ok(bstr(&name)),
                None => standard!(self.get_accName(v)),
            },
            Child::Standard => standard!(self.get_accName(v)),
        }
    }

    fn get_accValue(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.value {
                Some(value) => Ok(bstr(&value)),
                None => none(),
            },
            _ => standard!(self.get_accValue(v)),
        }
    }

    fn get_accDescription(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.description {
                Some(d) => Ok(bstr(&d)),
                None => none(),
            },
            _ => standard!(self.get_accDescription(v)),
        }
    }

    fn get_accRole(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.role as i32)),
            Child::Me => match self.elements.own() {
                Some((_, role)) => Ok(variant_i4(role as i32)),
                None => standard!(self.get_accRole(v)),
            },
            Child::Standard => standard!(self.get_accRole(v)),
        }
    }

    fn get_accState(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.state as i32)),
            _ => standard!(self.get_accState(v)),
        }
    }

    fn get_accHelp(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accHelp(v)),
        }
    }

    fn get_accHelpTopic(&self, file: *mut BSTR, v: &VARIANT) -> windows::core::Result<i32> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accHelpTopic(file, v)),
        }
    }

    fn get_accKeyboardShortcut(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accKeyboardShortcut(v)),
        }
    }

    fn accFocus(&self) -> windows::core::Result<VARIANT> {
        standard!(self.accFocus())
    }

    fn accSelection(&self) -> windows::core::Result<VARIANT> {
        standard!(self.accSelection())
    }

    fn get_accDefaultAction(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.action {
                Some(name) => Ok(bstr(name)),
                None => none(),
            },
            _ => standard!(self.get_accDefaultAction(v)),
        }
    }

    fn accSelect(&self, flags: i32, v: &VARIANT) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.accSelect(flags, v)),
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
            _ => standard!(self.accLocation(x, y, cx, cy, v)),
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
            _ => standard!(self.accNavigate(dir, start)),
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
        standard!(self.accHitTest(x, y))
    }

    fn accDoDefaultAction(&self, v: &VARIANT) -> windows::core::Result<()> {
        match self.which(v)? {
            Child::Part(i) => {
                if self.part(i)?.action.is_none() {
                    return none();
                }
                if self.part(i)?.state & STATE_UNAVAILABLE != 0 {
                    return Err(E_FAIL.into());
                }
                self.elements.act(i);
                Ok(())
            }
            _ => standard!(self.accDoDefaultAction(v)),
        }
    }

    fn put_accName(&self, _v: &VARIANT, _name: &BSTR) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn put_accValue(&self, _v: &VARIANT, _value: &BSTR) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// A window's accessible object for its client area: the standard one,
/// with the parts `elements` reports added.
pub struct AccessibleObject {
    object: IAccessible,
}

impl AccessibleObject {
    /// The object for window `hwnd` (a window of this thread, whose
    /// standard client object it wraps); `None` if the system would not
    /// make the standard one.
    pub fn new(hwnd: HWND, elements: impl Elements + 'static) -> Option<AccessibleObject> {
        let mut raw = core::ptr::null_mut();
        // SAFETY: asks for the standard client object of the window, as
        // IAccessible, into an out-pointer.
        unsafe {
            CreateStdAccessibleObject(WHWND(hwnd), OBJID_CLIENT, &IAccessible::IID, &mut raw)
        }
        .ok()?;
        if raw.is_null() {
            return None;
        }
        // SAFETY: an IAccessible (the IID asked for), not null, whose
        // reference the call handed over.
        let standard = unsafe { IAccessible::from_raw(raw) };
        let object: IAccessible = Accessible {
            hwnd,
            standard,
            elements: Box::new(elements),
        }
        .into();
        Some(AccessibleObject { object })
    }

    /// WM_GETOBJECT's answer for the client area, given its wParam
    /// (LresultFromObject: a reference to the object for the asker).
    pub fn answer_w(&self, w: usize) -> isize {
        // SAFETY: the interface's id and the object, read during the call;
        // wParam is WM_GETOBJECT's flags, an integer.
        let r: WLRESULT = unsafe { LresultFromObject(&IAccessible::IID, WWPARAM(w), &self.object) };
        r.0
    }
}

/// Whether WM_GETOBJECT's `l` asks for the client area (OBJID_CLIENT).
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
        assert!(wants_client(-4) && !wants_client(-3));
    }
}
