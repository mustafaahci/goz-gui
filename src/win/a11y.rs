//! Accessibility (MSAA) for what a window paints itself: an `IAccessible`
//! that wraps the system's standard one for the window's client area and
//! adds the painted parts as simple elements after its children.
//!
//! The object is a private type; what is handed out is an
//! [`AccessibleObject`], configured with a Rust value ([`Elements`]) that
//! it asks for the parts each time. Not agile: calls from other processes
//! come through the window's thread's apartment, where that value lives.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, HWND as WHWND, LRESULT as WLRESULT,
    S_FALSE, S_OK, WPARAM as WWPARAM,
};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, IDispatch_Vtbl, ITypeInfo,
};
use windows::Win32::System::Variant::{VARIANT, VT_EMPTY, VT_I4};
use windows::Win32::UI::Accessibility::{
    CreateStdAccessibleObject, IAccessible, IAccessible_Impl, IAccessible_Vtbl, LresultFromObject,
};
use windows::core::{BSTR, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, implement};
use windows_core::IUnknownImpl;
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
/// through the window's thread's apartment, where its closures live. Each
/// method holds a reference to the object while it runs, as the window's
/// code it calls ([`Elements`]) may let the last other one go.
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
        let _hold = self.to_object();
        standard!(self.accParent())
    }

    fn accChildCount(&self) -> windows::core::Result<i32> {
        let _hold = self.to_object();
        Ok(self.standard_count() + self.parts().len() as i32)
    }

    fn get_accChild(&self, v: &VARIANT) -> windows::core::Result<IDispatch> {
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Standard => standard!(self.get_accChild(v)),
            // A simple element: no object of its own.
            Child::Part(_) | Child::Me => none(),
        }
    }

    fn get_accName(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        let _hold = self.to_object();
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
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.value {
                Some(value) => Ok(bstr(&value)),
                None => none(),
            },
            _ => standard!(self.get_accValue(v)),
        }
    }

    fn get_accDescription(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.description {
                Some(d) => Ok(bstr(&d)),
                None => none(),
            },
            _ => standard!(self.get_accDescription(v)),
        }
    }

    fn get_accRole(&self, v: &VARIANT) -> windows::core::Result<VARIANT> {
        let _hold = self.to_object();
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
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(i) => Ok(variant_i4(self.part(i)?.state as i32)),
            _ => standard!(self.get_accState(v)),
        }
    }

    fn get_accHelp(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accHelp(v)),
        }
    }

    fn get_accHelpTopic(&self, file: *mut BSTR, v: &VARIANT) -> windows::core::Result<i32> {
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accHelpTopic(file, v)),
        }
    }

    fn get_accKeyboardShortcut(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(_) => none(),
            _ => standard!(self.get_accKeyboardShortcut(v)),
        }
    }

    fn accFocus(&self) -> windows::core::Result<VARIANT> {
        let _hold = self.to_object();
        standard!(self.accFocus())
    }

    fn accSelection(&self) -> windows::core::Result<VARIANT> {
        let _hold = self.to_object();
        standard!(self.accSelection())
    }

    fn get_accDefaultAction(&self, v: &VARIANT) -> windows::core::Result<BSTR> {
        let _hold = self.to_object();
        match self.which(v)? {
            Child::Part(i) => match self.part(i)?.action {
                Some(name) => Ok(bstr(name)),
                None => none(),
            },
            _ => standard!(self.get_accDefaultAction(v)),
        }
    }

    fn accSelect(&self, flags: i32, v: &VARIANT) -> windows::core::Result<()> {
        let _hold = self.to_object();
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
        let _hold = self.to_object();
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
        let _hold = self.to_object();
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
        let _hold = self.to_object();
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
        let _hold = self.to_object();
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
        let _hold = self.to_object();
        Err(E_NOTIMPL.into())
    }

    fn put_accValue(&self, _v: &VARIANT, _value: &BSTR) -> windows::core::Result<()> {
        let _hold = self.to_object();
        Err(E_NOTIMPL.into())
    }
}

/// What a client is handed for a window: a front of [`Accessible`] that
/// writes every out-parameter (a null, VT_EMPTY, 0) before the call goes
/// on to the object. The object answers S_FALSE (a success: "not here")
/// for what a simple element has none of, through `windows`' error path,
/// which leaves its out-parameter as it was; a client may read one after
/// a success, so it is written first. Its identity is its own: it answers
/// IUnknown, IDispatch and IAccessible with itself, and nothing else.
#[repr(C)]
struct Front {
    vtbl: &'static IAccessible_Vtbl,
    refs: AtomicU32,
    inner: IAccessible,
}

impl Front {
    /// The front over `inner`, with the one reference handed back.
    fn wrap(inner: IAccessible) -> IAccessible {
        let front = Box::new(Front {
            vtbl: &FRONT,
            refs: AtomicU32::new(1),
            inner,
        });
        // SAFETY: a `Front`, whose first field is the IAccessible vtable's
        // address as COM lays an object out, its count at 1: the reference
        // handed over, freed when the last is released (`release`).
        unsafe { IAccessible::from_raw(Box::into_raw(front).cast()) }
    }
}

/// The object a front's method goes on to, and its vtable.
///
/// # Safety
/// `this` is a [`Front`] a client holds a reference to (it is calling it).
unsafe fn inner<'a>(this: *mut c_void) -> (*mut c_void, &'a IAccessible_Vtbl) {
    // SAFETY: as the caller promises; the object's reference is the
    // front's, which outlives the call it is making.
    let front = unsafe { &*(this as *const Front) };
    (front.inner.as_raw(), front.inner.vtable())
}

/// Writes `value` to the out-parameter `p`, if there is one.
///
/// # Safety
/// `p` is null or writable for a `T` (its old contents not dropped).
unsafe fn put<T>(p: *mut T, value: T) {
    if !p.is_null() {
        // SAFETY: as the caller promises.
        unsafe { p.write(value) };
    }
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's out-pointer, not null; its IID, read.
    unsafe {
        out.write(core::ptr::null_mut());
        let Some(iid) = iid.as_ref() else {
            return E_POINTER;
        };
        if *iid == IUnknown::IID || *iid == IDispatch::IID || *iid == IAccessible::IID {
            add_ref(this);
            out.write(this);
            return S_OK;
        }
    }
    E_NOINTERFACE
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    // SAFETY: a front a client holds a reference to.
    let front = unsafe { &*(this as *const Front) };
    front.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    // SAFETY: a front a client holds a reference to.
    let left = unsafe { &*(this as *const Front) }
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        - 1;
    if left == 0 {
        // SAFETY: the last reference released: the box `Front::wrap`
        // made, freed once (the object's reference with it).
        drop(unsafe { Box::from_raw(this as *mut Front) });
    }
    left
}

/// A front method whose last parameter is a pointer out-parameter, from
/// VARIANT `v`: null written, then passed on.
macro_rules! out_pointer {
    ($name:ident, $field:ident) => {
        unsafe extern "system" fn $name(
            this: *mut c_void,
            v: VARIANT,
            out: *mut *mut c_void,
        ) -> HRESULT {
            // SAFETY: a front a client calls, with its out-parameter;
            // passed on as it came.
            unsafe {
                put(out, core::ptr::null_mut());
                let (raw, vtbl) = inner(this);
                (vtbl.$field)(raw, v, out)
            }
        }
    };
}

/// A front method whose last parameter is a VARIANT out-parameter, from
/// VARIANT `v`: VT_EMPTY written, then passed on.
macro_rules! out_variant {
    ($name:ident, $field:ident) => {
        unsafe extern "system" fn $name(
            this: *mut c_void,
            v: VARIANT,
            out: *mut VARIANT,
        ) -> HRESULT {
            // SAFETY: as in `out_pointer`.
            unsafe {
                put(out, VARIANT::default());
                let (raw, vtbl) = inner(this);
                (vtbl.$field)(raw, v, out)
            }
        }
    };
}

out_pointer!(get_acc_child, get_accChild);
out_pointer!(get_acc_name, get_accName);
out_pointer!(get_acc_value, get_accValue);
out_pointer!(get_acc_description, get_accDescription);
out_pointer!(get_acc_help, get_accHelp);
out_pointer!(get_acc_keyboard_shortcut, get_accKeyboardShortcut);
out_pointer!(get_acc_default_action, get_accDefaultAction);
out_variant!(get_acc_role, get_accRole);
out_variant!(get_acc_state, get_accState);

unsafe extern "system" fn get_type_info_count(this: *mut c_void, out: *mut u32) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, 0);
        let (raw, vtbl) = inner(this);
        (vtbl.base__.GetTypeInfoCount)(raw, out)
    }
}

unsafe extern "system" fn get_type_info(
    this: *mut c_void,
    i: u32,
    lcid: u32,
    out: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, core::ptr::null_mut());
        let (raw, vtbl) = inner(this);
        (vtbl.base__.GetTypeInfo)(raw, i, lcid, out)
    }
}

unsafe extern "system" fn get_ids_of_names(
    this: *mut c_void,
    riid: *const GUID,
    names: *const PCWSTR,
    count: u32,
    lcid: u32,
    ids: *mut i32,
) -> HRESULT {
    // SAFETY: as in `out_pointer` (the object writes no id: not
    // implemented).
    unsafe {
        let (raw, vtbl) = inner(this);
        (vtbl.base__.GetIDsOfNames)(raw, riid, names, count, lcid, ids)
    }
}

#[allow(clippy::too_many_arguments, reason = "IDispatch::Invoke's own")]
unsafe extern "system" fn invoke(
    this: *mut c_void,
    id: i32,
    riid: *const GUID,
    lcid: u32,
    flags: DISPATCH_FLAGS,
    params: *const DISPPARAMS,
    result: *mut VARIANT,
    excep: *mut EXCEPINFO,
    arg: *mut u32,
) -> HRESULT {
    // SAFETY: as in `out_pointer` (the object answers E_NOTIMPL and
    // writes nothing).
    unsafe {
        let (raw, vtbl) = inner(this);
        (vtbl.base__.Invoke)(raw, id, riid, lcid, flags, params, result, excep, arg)
    }
}

unsafe extern "system" fn acc_parent(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, core::ptr::null_mut());
        let (raw, vtbl) = inner(this);
        (vtbl.accParent)(raw, out)
    }
}

unsafe extern "system" fn acc_child_count(this: *mut c_void, out: *mut i32) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, 0);
        let (raw, vtbl) = inner(this);
        (vtbl.accChildCount)(raw, out)
    }
}

unsafe extern "system" fn get_acc_help_topic(
    this: *mut c_void,
    file: *mut *mut c_void,
    v: VARIANT,
    topic: *mut i32,
) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(file, core::ptr::null_mut());
        put(topic, 0);
        let (raw, vtbl) = inner(this);
        (vtbl.get_accHelpTopic)(raw, file, v, topic)
    }
}

unsafe extern "system" fn acc_focus(this: *mut c_void, out: *mut VARIANT) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, VARIANT::default());
        let (raw, vtbl) = inner(this);
        (vtbl.accFocus)(raw, out)
    }
}

unsafe extern "system" fn acc_selection(this: *mut c_void, out: *mut VARIANT) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, VARIANT::default());
        let (raw, vtbl) = inner(this);
        (vtbl.accSelection)(raw, out)
    }
}

unsafe extern "system" fn acc_select(this: *mut c_void, flags: i32, v: VARIANT) -> HRESULT {
    // SAFETY: as in `out_pointer` (no out-parameter).
    unsafe {
        let (raw, vtbl) = inner(this);
        (vtbl.accSelect)(raw, flags, v)
    }
}

unsafe extern "system" fn acc_location(
    this: *mut c_void,
    x: *mut i32,
    y: *mut i32,
    cx: *mut i32,
    cy: *mut i32,
    v: VARIANT,
) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        for p in [x, y, cx, cy] {
            put(p, 0);
        }
        let (raw, vtbl) = inner(this);
        (vtbl.accLocation)(raw, x, y, cx, cy, v)
    }
}

unsafe extern "system" fn acc_navigate(
    this: *mut c_void,
    dir: i32,
    start: VARIANT,
    out: *mut VARIANT,
) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, VARIANT::default());
        let (raw, vtbl) = inner(this);
        (vtbl.accNavigate)(raw, dir, start, out)
    }
}

unsafe extern "system" fn acc_hit_test(
    this: *mut c_void,
    x: i32,
    y: i32,
    out: *mut VARIANT,
) -> HRESULT {
    // SAFETY: as in `out_pointer`.
    unsafe {
        put(out, VARIANT::default());
        let (raw, vtbl) = inner(this);
        (vtbl.accHitTest)(raw, x, y, out)
    }
}

unsafe extern "system" fn acc_do_default_action(this: *mut c_void, v: VARIANT) -> HRESULT {
    // SAFETY: as in `out_pointer` (no out-parameter).
    unsafe {
        let (raw, vtbl) = inner(this);
        (vtbl.accDoDefaultAction)(raw, v)
    }
}

unsafe extern "system" fn put_acc_name(
    this: *mut c_void,
    v: VARIANT,
    name: *mut c_void,
) -> HRESULT {
    // SAFETY: as in `out_pointer` (no out-parameter; the name is the
    // caller's, passed on as it came).
    unsafe {
        let (raw, vtbl) = inner(this);
        (vtbl.put_accName)(raw, v, name)
    }
}

unsafe extern "system" fn put_acc_value(
    this: *mut c_void,
    v: VARIANT,
    value: *mut c_void,
) -> HRESULT {
    // SAFETY: as in `put_acc_name`.
    unsafe {
        let (raw, vtbl) = inner(this);
        (vtbl.put_accValue)(raw, v, value)
    }
}

/// The front's vtable: IUnknown's, IDispatch's and IAccessible's methods,
/// in their order.
static FRONT: IAccessible_Vtbl = IAccessible_Vtbl {
    base__: IDispatch_Vtbl {
        base__: IUnknown_Vtbl {
            QueryInterface: query_interface,
            AddRef: add_ref,
            Release: release,
        },
        GetTypeInfoCount: get_type_info_count,
        GetTypeInfo: get_type_info,
        GetIDsOfNames: get_ids_of_names,
        Invoke: invoke,
    },
    accParent: acc_parent,
    accChildCount: acc_child_count,
    get_accChild: get_acc_child,
    get_accName: get_acc_name,
    get_accValue: get_acc_value,
    get_accDescription: get_acc_description,
    get_accRole: get_acc_role,
    get_accState: get_acc_state,
    get_accHelp: get_acc_help,
    get_accHelpTopic: get_acc_help_topic,
    get_accKeyboardShortcut: get_acc_keyboard_shortcut,
    accFocus: acc_focus,
    accSelection: acc_selection,
    get_accDefaultAction: get_acc_default_action,
    accSelect: acc_select,
    accLocation: acc_location,
    accNavigate: acc_navigate,
    accHitTest: acc_hit_test,
    accDoDefaultAction: acc_do_default_action,
    put_accName: put_acc_name,
    put_accValue: put_acc_value,
};

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
        Some(AccessibleObject {
            object: Front::wrap(object),
        })
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

    /// One painted part with no value, description or action.
    struct One;

    impl Elements for One {
        fn elements(&self) -> Vec<Element> {
            vec![Element {
                name: "Clear".into(),
                role: 43,
                state: 0,
                rect: RECT::default(),
                value: None,
                description: None,
                action: None,
            }]
        }
        fn own(&self) -> Option<(String, u32)> {
            None
        }
        fn act(&self, _index: usize) {}
    }

    #[test]
    fn a_part_without_a_value_says_so_with_its_out_parameter_written() {
        use crate::win::class::{self, Class, Create};
        let _ = Class::new("goz-gui a11y test").register();
        let hwnd = class::create(
            &Create {
                ex_style: 0,
                class: "goz-gui a11y test",
                title: "",
                style: 0,
                rect: (0, 0, 10, 10),
                parent: core::ptr::null_mut(),
                menu: core::ptr::null_mut(),
            },
            |m| m.def(),
        );
        let object = AccessibleObject::new(hwnd, One).expect("the standard object");
        let acc = &object.object;
        // The part is the child after the standard ones.
        // SAFETY: a call on this thread's object, an out-value.
        let count = unsafe { acc.accChildCount() }.expect("a count");
        let part = variant_i4(count);
        let vtbl = acc.vtable();
        for get in [
            vtbl.get_accValue,
            vtbl.get_accDescription,
            vtbl.get_accChild,
        ] {
            // Not what the caller left there: whatever came back is the
            // object's.
            let mut out: *mut c_void = core::ptr::without_provenance_mut(1);
            // SAFETY: the object's method with a VARIANT (a VT_I4, nothing
            // to free) and an out-pointer, alive during the call.
            let hr = unsafe { get(acc.as_raw(), variant_i4(count), &mut out) };
            assert_eq!(hr, S_FALSE);
            assert!(out.is_null());
        }
        // SAFETY: a call on this thread's object.
        let name = unsafe { acc.get_accName(&part) }.expect("a name");
        assert_eq!(name.to_string(), "Clear");
        // Its own identity: asked for IUnknown, the same object.
        let unknown: IUnknown = acc.cast().expect("IUnknown");
        assert_eq!(unknown.as_raw(), acc.as_raw());
        assert!(acc.cast::<windows::Win32::System::Com::IStream>().is_err());
        drop(object);
        crate::win::destroy(hwnd);
    }

    #[test]
    fn child_ids_read_from_variants() {
        assert_eq!(child_id(&variant_i4(3)), Some(3));
        assert_eq!(child_id(&VARIANT::default()), Some(0));
        assert!(wants_client(-4) && !wants_client(-3));
    }
}
