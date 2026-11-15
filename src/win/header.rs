//! Header messages (SysHeader32), each sent only to a header. Item
//! structures go in by value, their pointers set here.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    HDHITTESTINFO, HDI_FILTER, HDI_TEXT, HDITEMW, HDLAYOUT, HDM_GETITEMCOUNT, HDM_GETITEMRECT,
    HDM_GETITEMW, HDM_HITTEST, HDM_LAYOUT, HDM_SETITEMW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPOS;

/// The class these messages are for.
pub const CLASS: &str = "SysHeader32";

/// # Safety
/// `w` and `l` are what `msg` defines for a header.
unsafe fn send(h: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a header (or to null).
    unsafe { super::send_raw(super::of_class(h, CLASS), msg, w, l) }
}

/// How many items (columns) the header has.
pub fn item_count(h: HWND) -> usize {
    // SAFETY: no parameters.
    usize::try_from(unsafe { send(h, HDM_GETITEMCOUNT, 0, 0) }).unwrap_or(0)
}

/// Item `i`'s rectangle, in the header's client coordinates.
pub fn item_rect(h: HWND, i: i32) -> Option<RECT> {
    let mut rc = RECT::default();
    // SAFETY: HDM_GETITEMRECT's lParam is a RECT out.
    let ok = unsafe {
        send(
            h,
            HDM_GETITEMRECT,
            i as isize as usize,
            (&raw mut rc) as isize,
        )
    } != 0;
    ok.then_some(rc)
}

/// HDM_HITTEST's answer at `pt` (client coordinates): the item there, -1
/// for none.
pub fn item_at(h: HWND, pt: POINT) -> i32 {
    let mut ht = HDHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: HDM_HITTEST's lParam is an HDHITTESTINFO, in and out.
    unsafe { send(h, HDM_HITTEST, 0, (&raw mut ht) as isize) as i32 }
}

/// HDM_LAYOUT as a header's procedure (or its subclass) gets it: `f`
/// changes where the header goes (the WINDOWPOS) and what of the parent's
/// rectangle is left under it (the RECT). `None` for any other message, a
/// window that is not a header, or a layout without its two structures.
pub fn on_layout<R>(m: &super::Msg, f: impl FnOnce(&mut WINDOWPOS, &mut RECT) -> R) -> Option<R> {
    if m.id != HDM_LAYOUT || !super::class_is(m.hwnd, CLASS) {
        return None;
    }
    // SAFETY: a delivered HDM_LAYOUT's lParam, for a header, is an HDLAYOUT.
    let view = unsafe { m.view_lparam::<HDLAYOUT>() }?;
    let (prc, pwpos) = (view.prc, view.pwpos);
    if prc.is_null() || pwpos.is_null() {
        return None;
    }
    // SAFETY: HDLAYOUT's two pointers are the sender's RECT and WINDOWPOS,
    // two objects apart from the HDLAYOUT and from each other, valid while
    // the header's procedure runs. They are lent to `f` only, while the view
    // is out (so the message cannot be passed on meanwhile).
    let (pos, rest) = unsafe { (&mut *pwpos, &mut *prc) };
    let r = f(pos, rest);
    drop(view);
    Some(r)
}

/// The fields `mask` asks for (HDI_*) of item `i`, and its text when
/// HDI_TEXT is among them (up to 260 units).
pub fn get_item(h: HWND, i: i32, mask: u32) -> Option<(HDITEMW, Option<String>)> {
    let mut buf = [0u16; 260];
    let mut it = HDITEMW {
        mask: mask & !HDI_FILTER,
        ..Default::default()
    };
    if it.mask & HDI_TEXT != 0 {
        it.pszText = buf.as_mut_ptr();
        it.cchTextMax = buf.len() as i32;
    }
    // SAFETY: HDM_GETITEMW's lParam is an HDITEMW whose only pointer is the
    // text buffer (no filter asked for).
    let ok = unsafe { send(h, HDM_GETITEMW, i as isize as usize, (&raw mut it) as isize) } != 0;
    if !ok {
        return None;
    }
    let text = (it.mask & HDI_TEXT != 0).then(|| String::from_utf16_lossy(super::until_nul(&buf)));
    it.pszText = core::ptr::null_mut();
    it.cchTextMax = 0;
    Some((it, text))
}

/// Sets the fields `item.mask` names of item `i`, with `text` when given.
/// No filters (HDI_FILTER is dropped).
pub fn set_item(h: HWND, i: i32, item: HDITEMW, text: Option<&str>) -> bool {
    let mut it = item;
    it.mask &= !HDI_FILTER;
    it.pvFilter = core::ptr::null_mut();
    let z = text.map(super::wz);
    match &z {
        Some(z) => {
            it.mask |= HDI_TEXT;
            it.pszText = z.as_ptr().cast_mut();
        }
        None => {
            it.mask &= !HDI_TEXT;
            it.pszText = core::ptr::null_mut();
        }
    }
    it.cchTextMax = 0;
    // SAFETY: HDM_SETITEMW's lParam is an HDITEMW whose pointers are set
    // above (mutable, as the header may write it).
    unsafe { send(h, HDM_SETITEMW, i as isize as usize, (&raw mut it) as isize) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_message_hands_out_no_layout() {
        let m = super::super::Msg::new(core::ptr::null_mut(), HDM_LAYOUT, 0, 0x1000);
        assert!(on_layout(&m, |_, _| ()).is_none());
        assert_eq!(item_at(core::ptr::null_mut(), POINT::default()), 0);
    }
}
