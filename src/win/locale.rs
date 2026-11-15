//! Text as the system's national language support has it: Unicode
//! normalization, comparisons, case, code pages, and the user's formats of
//! dates, times and sizes. Every buffer a call writes is a slice given in,
//! written no further than its length.

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{
    COMPARESTRING_RESULT, CP_ACP, CPINFO, CompareStringOrdinal, GetCPInfo, GetDateFormatEx,
    GetLocaleInfoEx, GetTimeFormatEx, IsDBCSLeadByteEx, LCMAP_UPPERCASE, LCMapStringEx,
    LOCALE_NAME_INVARIANT, MultiByteToWideChar, NORM_FORM, NormalizeString, WideCharToMultiByte,
};
use windows_sys::Win32::UI::Shell::{StrCmpLogicalW, StrFormatByteSizeW};

use super::{len_i32, until_nul, wz_units};

/// A buffer to write to: its start, or null for an empty one (which asks
/// the call for the length it needs).
fn out_ptr<T>(buf: &mut [T]) -> *mut T {
    if buf.is_empty() {
        core::ptr::null_mut()
    } else {
        buf.as_mut_ptr()
    }
}

/// NormalizeString: `src` in Unicode normalization form `form`
/// (NormalizationD...) into `out`; the units written (or, for an empty
/// `out`, needed), 0 or less on failure.
pub fn normalize(form: NORM_FORM, src: &[u16], out: &mut [u16]) -> i32 {
    // SAFETY: the source and its length, read; the buffer and its length,
    // written (a null buffer of length 0 asks for the length).
    unsafe {
        NormalizeString(
            form,
            src.as_ptr(),
            len_i32(src.len()),
            out_ptr(out),
            len_i32(out.len()),
        )
    }
}

/// `a` against `b` by their units (CompareStringOrdinal), case aside when
/// `ignore_case` (as Windows folds case for file names): CSTR_LESS_THAN,
/// CSTR_EQUAL or CSTR_GREATER_THAN, 0 when it failed.
pub fn compare_ordinal(a: &[u16], b: &[u16], ignore_case: bool) -> COMPARESTRING_RESULT {
    // SAFETY: two buffers, each read for its own length.
    unsafe {
        CompareStringOrdinal(
            a.as_ptr(),
            len_i32(a.len()),
            b.as_ptr(),
            len_i32(b.len()),
            ignore_case.into(),
        )
    }
}

/// StrCmpLogicalW of `a` and `b` (no NULs needed): below 0, 0 or above 0
/// as Explorer sorts the names (`2` before `10`, case aside).
pub fn cmp_logical(a: &[u16], b: &[u16]) -> i32 {
    let (a, b) = (wz_units(until_nul(a)), wz_units(until_nul(b)));
    // SAFETY: two NUL-terminated strings read during the call.
    unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) }
}

/// LCMapStringEx of `src` to upper case as the invariant locale has it,
/// into `dst`; the units written, 0 on failure.
pub fn upper_invariant(src: &[u16], dst: &mut [u16]) -> i32 {
    // SAFETY: the source read and the destination written, each for its
    // own length (LCMAP_UPPERCASE writes units, not bytes); no version
    // information, no sort handle.
    unsafe {
        LCMapStringEx(
            LOCALE_NAME_INVARIANT,
            LCMAP_UPPERCASE,
            src.as_ptr(),
            len_i32(src.len()),
            out_ptr(dst),
            len_i32(dst.len()),
            core::ptr::null(),
            core::ptr::null(),
            0,
        )
    }
}

/// WideCharToMultiByte of `wide` in code page `code_page` (CP_ACP...), no
/// flags, the code page's own default character, into `out`; the bytes
/// written (or, for an empty `out`, needed), 0 on failure.
pub fn wide_to_multi_byte(code_page: u32, wide: &[u16], out: &mut [u8]) -> i32 {
    // SAFETY: the source read for its length; the buffer written for its
    // length (null and 0 ask for the length); no default character and no
    // flag to set, which every code page takes.
    unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            wide.as_ptr(),
            len_i32(wide.len()),
            out_ptr(out),
            len_i32(out.len()),
            core::ptr::null(),
            core::ptr::null_mut(),
        )
    }
}

/// `src` decoded from code page `cp` (MultiByteToWideChar with `flags`)
/// into `out`: how many units were written (0 when it failed, or `out` is
/// too small for them all).
pub fn multi_byte_to_wide(cp: u32, flags: u32, src: &[u8], out: &mut [u16]) -> usize {
    // SAFETY: the source and its length, the room and its length.
    let n = unsafe {
        MultiByteToWideChar(
            cp,
            flags,
            src.as_ptr(),
            len_i32(src.len()),
            out.as_mut_ptr(),
            len_i32(out.len()),
        )
    };
    usize::try_from(n).unwrap_or(0)
}

/// `bytes` in the system's ANSI code page (MultiByteToWideChar, CP_ACP).
pub fn ansi_to_wide(bytes: &[u8]) -> Vec<u16> {
    // A code page character is never more UTF-16 units than bytes.
    let mut out = vec![0u16; bytes.len()];
    let n = multi_byte_to_wide(CP_ACP, 0, bytes, &mut out);
    out.truncate(n);
    out
}

/// What code page `cp` is (GetCPInfo); `None` if it is not one.
pub fn code_page_info(cp: u32) -> Option<CPINFO> {
    let mut info = CPINFO::default();
    // SAFETY: an out-struct of the right type.
    (unsafe { GetCPInfo(cp, &mut info) } != 0).then_some(info)
}

/// Whether `byte` starts a two-byte character in code page `cp`.
pub fn is_dbcs_lead_byte(cp: u32, byte: u8) -> bool {
    // SAFETY: no pointers.
    unsafe { IsDBCSLeadByteEx(cp, byte) != 0 }
}

/// GetLocaleInfoEx of the user's locale for `kind` into `buf`: the units
/// written, its NUL included (0 when it failed, or `buf` is too small).
pub fn user_locale_info(kind: u32, buf: &mut [u16]) -> i32 {
    // SAFETY: a null locale name is LOCALE_NAME_USER_DEFAULT; the buffer
    // and its length.
    unsafe {
        GetLocaleInfoEx(
            core::ptr::null(),
            kind,
            buf.as_mut_ptr(),
            len_i32(buf.len()),
        )
    }
}

/// `st`'s date as the user's locale writes it with `flags`
/// (DATE_SHORTDATE…) into `buf`: the units written, its NUL included (0
/// when it failed).
pub fn date_format(flags: u32, st: &SYSTEMTIME, buf: &mut [u16]) -> i32 {
    // SAFETY: the user's locale, its own format and calendar (nulls); an
    // in-struct; the buffer and its length.
    unsafe {
        GetDateFormatEx(
            core::ptr::null(),
            flags,
            st,
            core::ptr::null(),
            buf.as_mut_ptr(),
            len_i32(buf.len()),
            core::ptr::null(),
        )
    }
}

/// `st`'s time as the user's locale writes it with `flags`
/// (TIME_NOSECONDS…) into `buf`: the units written, its NUL included (0
/// when it failed).
pub fn time_format(flags: u32, st: &SYSTEMTIME, buf: &mut [u16]) -> i32 {
    // SAFETY: the user's locale and its own format (nulls); an in-struct;
    // the buffer and its length.
    unsafe {
        GetTimeFormatEx(
            core::ptr::null(),
            flags,
            st,
            core::ptr::null(),
            buf.as_mut_ptr(),
            len_i32(buf.len()),
        )
    }
}

/// `n` bytes as Explorer writes a size (StrFormatByteSizeW).
pub fn format_byte_size(n: i64) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length.
    unsafe { StrFormatByteSizeW(n, buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(until_nul(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn text_is_compared_and_mapped_as_windows_does() {
        assert_eq!(
            compare_ordinal(&w("Abc"), &w("aBC"), true),
            windows_sys::Win32::Globalization::CSTR_EQUAL
        );
        let mut out = [0u16; 3];
        assert_eq!(upper_invariant(&w("abc"), &mut out), 3);
        assert_eq!(out, *w("ABC").as_slice());
        let mut d = [0u16; 8];
        let n = normalize(
            windows_sys::Win32::Globalization::NormalizationD,
            &w("é"),
            &mut d,
        );
        assert_eq!(n, 2);
        assert_eq!(d[0], u16::from(b'e'));
        let need = wide_to_multi_byte(0, &w("hi"), &mut []);
        assert_eq!(need, 2);
    }

    #[test]
    fn names_compare_as_explorer_sorts_them() {
        assert!(cmp_logical(&w("b2"), &w("b10")) < 0);
        assert!(cmp_logical(&w("B10"), &w("b2")) > 0);
        assert_eq!(cmp_logical(&w("abc"), &w("ABC")), 0);
        // What follows a NUL is not compared.
        assert_eq!(cmp_logical(&[97, 0, 98], &w("a")), 0);
    }
}
