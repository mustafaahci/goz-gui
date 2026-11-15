//! Header messages (SysHeader32), each sent only to a header. Item
//! structures go in by value, their pointers set here.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    HDHITTESTINFO, HDI_FILTER, HDI_TEXT, HDITEMW, HDLAYOUT, HDM_GETITEMCOUNT, HDM_GETITEMRECT,
    HDM_GETITEMW, HDM_GETORDERARRAY, HDM_HITTEST, HDM_LAYOUT, HDM_ORDERTOINDEX, HDM_SETIMAGELIST,
    HDM_SETITEMW, HIMAGELIST,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPOS;

pub const CLASS: &str = "SysHeader32";

/// # Safety
/// `w` and `l` are what `msg` defines for a header.
unsafe fn send(h: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: as the caller promises, to a header (or to null).
    unsafe { super::send_raw(super::of_class(h, CLASS), msg, w, l) }
}

pub fn item_count(h: HWND) -> usize {
    // SAFETY: no parameters.
    usize::try_from(unsafe { send(h, HDM_GETITEMCOUNT, 0, 0) }).unwrap_or(0)
}

/// The index of the item shown at position `order`.
pub fn order_to_index(h: HWND, order: i32) -> i32 {
    // SAFETY: an integer.
    unsafe { send(h, HDM_ORDERTOINDEX, order as isize as usize, 0) as i32 }
}

/// The items' display order.
pub fn order(h: HWND) -> Vec<i32> {
    let mut v = vec![0i32; item_count(h)];
    // SAFETY: HDM_GETORDERARRAY's lParam is an array of wParam ints.
    let ok = unsafe { send(h, HDM_GETORDERARRAY, v.len(), v.as_mut_ptr() as isize) } != 0;
    if ok { v } else { Vec::new() }
}

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

/// HDM_HITTEST at `pt` (client coordinates).
pub fn hit_test(h: HWND, pt: POINT) -> HDHITTESTINFO {
    let mut ht = HDHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: HDM_HITTEST's lParam is an HDHITTESTINFO, in and out.
    unsafe { send(h, HDM_HITTEST, 0, (&raw mut ht) as isize) };
    ht
}

/// HDM_LAYOUT: where the header goes in `rc`, and what of `rc` is left.
pub fn layout(h: HWND, rc: RECT) -> (WINDOWPOS, RECT) {
    let mut rest = rc;
    let mut pos = WINDOWPOS::default();
    let lay = HDLAYOUT {
        prc: &mut rest,
        pwpos: &mut pos,
    };
    // SAFETY: HDM_LAYOUT's lParam is an HDLAYOUT of a RECT in-out and a
    // WINDOWPOS out, all alive for the call.
    unsafe { send(h, HDM_LAYOUT, 0, (&raw const lay) as isize) };
    (pos, rest)
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
    // above.
    unsafe {
        send(
            h,
            HDM_SETITEMW,
            i as isize as usize,
            (&raw const it) as isize,
        ) != 0
    }
}

pub fn set_image_list(h: HWND, il: HIMAGELIST) -> HIMAGELIST {
    // SAFETY: a handle.
    unsafe { send(h, HDM_SETIMAGELIST, 0, il) }
}
