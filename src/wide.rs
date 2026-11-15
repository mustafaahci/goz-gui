//! UTF-16 helpers for the files not yet on `crate::win` (the shell's
//! context menus, the preview pane, thumbnails), which has the same ones
//! (`win::wz`, `win::wz_units`); this goes once they are.

/// `s` as a NUL-terminated UTF-16 string.
pub fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `s` with a NUL appended, for a call that wants a C string.
pub fn nul(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s);
    v.push(0);
    v
}
