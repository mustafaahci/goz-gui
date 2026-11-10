//! A file's own icon at a thumbnail's size, made without the shell's
//! system image lists.
//!
//! Programs, shortcuts and icon files carry their own icon. Asked of the
//! shell's image factory (or drawn from the system's 256 px image list),
//! each such file takes an entry of its own in the process's jumbo image
//! list, about 50 KB that is never given back (measured: 35 MB for 683
//! programs, 17 MB more for 318 others). Here the file's icon handler
//! (`IExtractIconW`) names the icon and it is extracted at the size wanted,
//! drawn into a bitmap of goz-gui's own (which the thumbnail cache bounds
//! and lets go), and destroyed: nothing stays behind.
//!
//! An icon with no image that big (an old program's 32 or 48 px one) is
//! taken at its own size, which the view centres, as the shell's picture
//! did, rather than blown up.
//!
//! A handler may name instead an icon of the system image lists that a
//! whole class of files shares (a shortcut to a help file: the .chm type's
//! icon). That one is a single entry of the lists however many files show
//! it, as a type's icon is, so the view draws it from them.

use std::os::windows::ffi::OsStringExt;

use windows::Win32::UI::Shell::{
    BHID_SFUIObject, GIL_FORSHELL, GIL_NOTFILENAME, GIL_PERCLASS, IExtractIconW, IShellItem,
    SHCreateItemFromParsingName, SHDefExtractIconW,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::core::PCWSTR;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::Graphics::Gdi::{
    BITMAP, CreateCompatibleDC, DeleteDC, DeleteObject, GdiFlush, GetObjectW, HGDIOBJ, SelectObject,
};
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::LibraryLoader::{
    EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
    LoadLibraryExW, LoadResource, LockResource, SizeofResource,
};
use windows_sys::Win32::UI::Shell::{
    PathCreateFromUrlW, SHGSI_ICONLOCATION, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_APPLICATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DestroyIcon, DrawIconEx, GetIconInfo, ICONINFO,
};

use crate::thumbs::{Bitmap, Picture, dib};
use crate::wide::nul;

/// `RT_GROUP_ICON`: an icon's directory of images in a program.
const RT_GROUP_ICON: usize = 14;
/// The smallest icon worth extracting for a thumbnail.
const MIN_SIDE: i32 = 16;

/// The icon `path` carries, at most `side` pixels square: an image
/// premultiplied, ready for `AlphaBlend`, or an icon of the system image
/// lists that a class of files shares (a shortcut to a help file shows the
/// .chm type's icon), for the view to draw from them. A program without an
/// icon of its own (or a shortcut to one) gets Windows' icon for programs.
/// `None` when its icon is its type's after all (a control panel item's)
/// or it cannot be read: then the type's icon shows. For a thread with COM
/// initialized; it reads the file.
pub fn own_icon(path: &[u16], side: i32) -> Option<Picture> {
    let path = nul(path);
    // SAFETY: COM calls on this thread's objects with NUL-terminated
    // strings and buffers that outlive them; the icons extracted are ours
    // and destroyed here.
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok()?;
        let handler: IExtractIconW = item.BindToHandler(None, &BHID_SFUIObject).ok()?;
        let mut location = [0u16; 1024];
        let (mut index, mut flags) = (0i32, 0u32);
        handler
            .GetIconLocation(GIL_FORSHELL, &mut location, &mut index, &mut flags)
            .ok()?;
        let len = location.iter().position(|&c| c == 0).unwrap_or(0);
        let named = &location[..len];
        // "*": the icon is the system image lists' entry `index` (its
        // handler would give it only at 32 px), which the view draws from
        // them; one of a class (a shortcut to a document: its target
        // type's icon) is shared, one of the file's own is not.
        if named == [u16::from(b'*')] {
            return shared_icon(index, flags).map(Picture::Shared);
        }
        // An Internet shortcut's handler names its icon file as a file:
        // URL (not a file name to it) and extracts it only up to 96 px: the
        // file it names is extracted here, as any icon file is.
        let from_url = file_url_path(named);
        let names_file = from_url.is_some() || flags & GIL_NOTFILENAME == 0;
        let file = expanded(from_url.as_deref().unwrap_or(named));
        // A program's location is no file and index to extract from (the
        // handler extracts it, and only at a size the program has), but
        // its icon is still its first, found by the file.
        let in_file = if names_file { index } else { 0 };
        let native = largest(&file, in_file);
        let want = extract_side(native, side);
        let mut icon = HICON::default();
        // Low word: the large icon's size; high word: the small one's, not
        // asked for.
        let size = (want as u32) | (16 << 16);
        let _ = handler.Extract(
            PCWSTR(location.as_ptr()),
            index as u32,
            Some(&mut icon),
            None,
            size,
        );
        // S_FALSE: the caller extracts it, from the file and index named,
        // or from the program whose first icon it is (one the handler does
        // not take, such as a 150 px image). Also what an Internet
        // shortcut's handler gives over 96 px: S_OK and no icon.
        if icon.is_invalid() && (names_file || native.is_some()) {
            let file_z = nul(&file);
            let _ = SHDefExtractIconW(
                PCWSTR(file_z.as_ptr()),
                in_file,
                0,
                Some(&mut icon),
                None,
                want as u32,
            );
        }
        if icon.is_invalid() {
            // A program with no icon of its own (or a shortcut to one, or
            // to one that is gone) shows Windows' icon for programs, as
            // the shell's picture did; anything else its type's icon.
            return is_program(&file)
                .then(|| stock_application(side))
                .flatten()
                .map(Picture::Image);
        }
        // A handler may give a smaller icon than asked (one it has): taken
        // at its own size, not blown up.
        let at = icon_width(icon.0 as _).map_or(want, |w| w.clamp(MIN_SIDE, want));
        let out = render(icon.0 as _, at);
        DestroyIcon(icon.0 as _);
        out.map(Picture::Image)
    }
}

/// The system image lists' entry `index` that a handler named as `"*"`
/// with `flags`, when a class of files shares it (`GIL_PERCLASS`: a
/// shortcut to a document gives its target type's icon, an offline
/// program Windows' program icon). One of a single file's own (none seen)
/// would be an entry of the jumbo list for that file alone, the 50 KB this
/// module keeps clear of: `None`, and the type's icon shows.
fn shared_icon(index: i32, flags: u32) -> Option<i32> {
    (flags & GIL_PERCLASS != 0 && index >= 0).then_some(index)
}

/// Whether `text` is a `file:` URL (any case).
fn is_file_url(text: &[u16]) -> bool {
    let scheme: Vec<u16> = "file:".encode_utf16().collect();
    text.len() > scheme.len()
        && text[..scheme.len()]
            .iter()
            .zip(&scheme)
            .all(|(&c, &s)| crate::highlight::fold(c) == s)
}

/// The path a `file:` URL names (`file:///C:/Program%20Files/x.ico` is
/// `C:\Program Files\x.ico`); `None` for anything else.
fn file_url_path(text: &[u16]) -> Option<Vec<u16>> {
    if !is_file_url(text) {
        return None;
    }
    let url = nul(text);
    let mut out = vec![0u16; 1024];
    let mut len = out.len() as u32;
    // SAFETY: a NUL-terminated URL; the buffer with its length in
    // characters, which the call sets to the path's (without its NUL).
    let status = unsafe { PathCreateFromUrlW(url.as_ptr(), out.as_mut_ptr(), &mut len, 0) };
    if status < 0 || len == 0 || len as usize >= out.len() {
        return None;
    }
    out.truncate(len as usize);
    Some(out)
}

/// `text` with its environment variables (`%SystemRoot%`) expanded, as
/// icon locations may be written.
fn expanded(text: &[u16]) -> Vec<u16> {
    if !text.contains(&u16::from(b'%')) {
        return text.to_vec();
    }
    let source = nul(text);
    let mut out = vec![0u16; 1024];
    // SAFETY: a NUL-terminated source; the buffer with its length.
    let n = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), out.as_mut_ptr(), 1024) };
    match n as usize {
        // The count includes the NUL.
        1..=1024 => {
            out.truncate(n as usize - 1);
            out
        }
        _ => text.to_vec(),
    }
}

/// Whether `file` (an icon's location) is a program, whose missing icon
/// is Windows' icon for programs.
fn is_program(file: &[u16]) -> bool {
    let lower: Vec<u16> = file.iter().map(|&c| crate::highlight::fold(c)).collect();
    [".exe", ".scr", ".com"].iter().any(|ext| {
        let ext: Vec<u16> = ext.encode_utf16().collect();
        lower.ends_with(&ext)
    })
}

/// The width of `icon`'s image, in pixels.
fn icon_width(icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<i32> {
    // SAFETY: reads the icon's bitmaps' headers into plain out-structs;
    // the two bitmaps GetIconInfo makes are ours and deleted here.
    unsafe {
        let mut info: ICONINFO = core::mem::zeroed();
        if GetIconInfo(icon, &mut info) == 0 {
            return None;
        }
        let shown = if info.hbmColor.is_null() {
            info.hbmMask
        } else {
            info.hbmColor
        };
        let mut bm: BITMAP = core::mem::zeroed();
        let read = GetObjectW(shown, size_of::<BITMAP>() as i32, (&raw mut bm).cast());
        for b in [info.hbmColor, info.hbmMask] {
            if !b.is_null() {
                DeleteObject(b);
            }
        }
        (read != 0 && bm.bmWidth > 0).then_some(bm.bmWidth)
    }
}

/// Windows' icon for a program that has none of its own, `side` pixels
/// square, extracted as [`own_icon`] does.
pub fn stock_application(side: i32) -> Option<Bitmap> {
    // SAFETY: a plain out-struct whose size is set; the icon extracted is
    // ours and destroyed here.
    unsafe {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..core::mem::zeroed()
        };
        if SHGetStockIconInfo(SIID_APPLICATION, SHGSI_ICONLOCATION, &mut info) != 0 {
            return None;
        }
        let side = side.max(MIN_SIDE);
        let mut icon = HICON::default();
        let _ = SHDefExtractIconW(
            PCWSTR(info.szPath.as_ptr()),
            info.iIcon,
            0,
            Some(&mut icon),
            None,
            side as u32,
        );
        if icon.is_invalid() {
            return None;
        }
        let out = render(icon.0 as _, side);
        DestroyIcon(icon.0 as _);
        out
    }
}

/// The side to extract an icon at for a cell of `side` pixels: its largest
/// image's (`native`) when that is smaller, as a small icon is centred, not
/// blown up; `side` when it is not known.
fn extract_side(native: Option<i32>, side: i32) -> i32 {
    let side = side.max(MIN_SIDE);
    native.map_or(side, |n| n.clamp(MIN_SIDE, side))
}

/// The largest image of icon `index` in `file` (a program or library, or an
/// .ico/.cur file), in pixels: `index` counts the file's icons from 0, or
/// is minus a resource id.
fn largest(file: &[u16], index: i32) -> Option<i32> {
    if file.is_empty() {
        return None;
    }
    let lower: Vec<u16> = file.iter().map(|&c| crate::highlight::fold(c)).collect();
    let ends = |ext: &str| {
        let ext: Vec<u16> = ext.encode_utf16().collect();
        lower.ends_with(&ext)
    };
    if ends(".ico") || ends(".cur") {
        // ICONDIR: 6 bytes, then 16 per image.
        let path = std::ffi::OsString::from_wide(file);
        let mut head = [0u8; 6 + 16 * 64];
        let n = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
        return largest_in_directory(&head[..n], 16);
    }
    group_directory(file, index).and_then(|dir| largest_in_directory(&dir, 14))
}

/// The bytes of icon group `index` (see [`largest`]) of the program
/// `file`, loaded as data (nothing in it runs).
fn group_directory(file: &[u16], index: i32) -> Option<Vec<u8>> {
    struct Search {
        want: usize,
        seen: usize,
        found: Option<Vec<u8>>,
    }
    /// One icon group of the module: copied when it is the one wanted.
    unsafe extern "system" fn each(
        module: HMODULE,
        kind: *const u16,
        name: *const u16,
        param: isize,
    ) -> i32 {
        // SAFETY: `param` is the `Search` below, alive across the
        // enumeration; `name` is valid during this call.
        unsafe {
            let search = &mut *(param as *mut Search);
            if search.seen == search.want {
                search.found = copy_resource(module, name, kind);
                return 0;
            }
            search.seen += 1;
            1
        }
    }
    let path = nul(file);
    // SAFETY: the module is loaded as an image resource (nothing in it
    // runs) and freed here; the resource bytes are copied while it is
    // loaded.
    unsafe {
        let module = LoadLibraryExW(
            path.as_ptr(),
            core::ptr::null_mut(),
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        );
        if module.is_null() {
            return None;
        }
        let found = if index < 0 {
            copy_resource(
                module,
                index.unsigned_abs() as usize as *const u16,
                RT_GROUP_ICON as *const u16,
            )
        } else {
            let mut search = Search {
                want: index as usize,
                seen: 0,
                found: None,
            };
            EnumResourceNamesW(
                module,
                RT_GROUP_ICON as *const u16,
                Some(each),
                (&raw mut search) as isize,
            );
            search.found
        };
        FreeLibrary(module);
        found
    }
}

/// A copy of resource `name` of type `kind` in `module`.
///
/// # Safety
/// `module` is loaded; `name` and `kind` are resource names (ids or
/// NUL-terminated strings).
unsafe fn copy_resource(module: HMODULE, name: *const u16, kind: *const u16) -> Option<Vec<u8>> {
    // SAFETY: as the caller promises; the bytes are copied while the
    // module stays loaded.
    unsafe {
        let found = FindResourceW(module, name, kind);
        if found.is_null() {
            return None;
        }
        let size = SizeofResource(module, found) as usize;
        let data = LockResource(LoadResource(module, found));
        if data.is_null() || size == 0 {
            return None;
        }
        Some(core::slice::from_raw_parts(data.cast::<u8>(), size).to_vec())
    }
}

/// The widest image an icon directory lists: a count at byte 4, then
/// `entry`-byte entries from byte 6 whose first byte is the width (0 for
/// 256). 14-byte entries in a program's icon group, 16 in an .ico file.
fn largest_in_directory(dir: &[u8], entry: usize) -> Option<i32> {
    let count = u16::from_le_bytes([*dir.get(4)?, *dir.get(5)?]) as usize;
    (0..count)
        .map_while(|i| dir.get(6 + i * entry))
        .map(|&w| if w == 0 { 256 } else { i32::from(w) })
        .max()
}

/// `icon` drawn into a new `side` × `side` bitmap: on black and then on
/// white, which gives each pixel's coverage whatever kind of icon it is
/// (alpha, or a mask), so the result is premultiplied BGRA either way.
fn render(icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON, side: i32) -> Option<Bitmap> {
    let (bitmap, bits) = dib(side, side)?;
    let count = (side * side) as usize;
    // SAFETY: a memory DC made and deleted here holds the new DIB; its
    // pixels (`count` of them) are touched only after GdiFlush, between
    // the draws, while `bitmap` keeps them alive.
    unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let old = SelectObject(dc, bitmap.handle() as HGDIOBJ);
        let pixels = bits.cast::<[u8; 4]>();
        // A new DIB section is all zero: black.
        DrawIconEx(
            dc,
            0,
            0,
            icon,
            side,
            side,
            0,
            core::ptr::null_mut(),
            DI_NORMAL,
        );
        GdiFlush();
        let black = core::slice::from_raw_parts(pixels, count).to_vec();
        core::slice::from_raw_parts_mut(pixels, count).fill([255, 255, 255, 0]);
        DrawIconEx(
            dc,
            0,
            0,
            icon,
            side,
            side,
            0,
            core::ptr::null_mut(),
            DI_NORMAL,
        );
        GdiFlush();
        let out = core::slice::from_raw_parts_mut(pixels, count);
        for (px, b) in out.iter_mut().zip(&black) {
            *px = premultiplied(*b, *px);
        }
        SelectObject(dc, old);
        DeleteDC(dc);
    }
    Some(bitmap)
}

/// A pixel drawn on black (`black`) and on white (`white`) as one
/// premultiplied BGRA pixel: what the white shows through is its
/// transparency, and on black its colour is already multiplied by its
/// coverage.
fn premultiplied(black: [u8; 4], white: [u8; 4]) -> [u8; 4] {
    // Green, the channel drawn most exactly.
    let through = white[1].saturating_sub(black[1]);
    let alpha = 255 - through;
    [
        black[0].min(alpha),
        black[1].min(alpha),
        black[2].min(alpha),
        alpha,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_icon_is_taken_at_its_own_size() {
        // A program with 32 and 48 px images, for a 256 px cell: 48.
        assert_eq!(extract_side(Some(48), 256), 48);
        // One with a 256 px image, for a 128 px cell: 128.
        assert_eq!(extract_side(Some(256), 128), 128);
        // Not known: the cell's size.
        assert_eq!(extract_side(None, 96), 96);
        // Never under 16 px, nor over the cell.
        assert_eq!(extract_side(Some(8), 64), 16);
        assert_eq!(extract_side(Some(64), 8), 16);
    }

    #[test]
    fn the_widest_image_of_an_icon_directory() {
        // A program's group: reserved, type 1, three entries of 14 bytes
        // (16, 48 and a 256, written 0).
        let mut group = vec![0, 0, 1, 0, 3, 0];
        for w in [16u8, 48, 0] {
            let mut e = vec![0u8; 14];
            e[0] = w;
            e[1] = w;
            group.extend(e);
        }
        assert_eq!(largest_in_directory(&group, 14), Some(256));
        // An .ico file's directory: 16-byte entries, 32 and 48.
        let mut ico = vec![0, 0, 1, 0, 2, 0];
        for w in [32u8, 48] {
            let mut e = vec![0u8; 16];
            e[0] = w;
            ico.extend(e);
        }
        assert_eq!(largest_in_directory(&ico, 16), Some(48));
        // Cut short: the entries there count; none or no header: none.
        assert_eq!(largest_in_directory(&ico[..6 + 16], 16), Some(32));
        assert_eq!(largest_in_directory(&ico[..6], 16), None);
        assert_eq!(largest_in_directory(&[0, 0, 1], 16), None);
    }

    #[test]
    fn a_star_location_is_drawn_only_when_a_class_shares_it() {
        // A shortcut to a help file: "*", GIL_PERCLASS | GIL_NOTFILENAME.
        assert_eq!(shared_icon(157, 0x0c), Some(157));
        // One to a text file (an extra flag the shell sets as well).
        assert_eq!(shared_icon(55, 0x2c), Some(55));
        // A file's own entry: not drawn from the lists; nor no index.
        assert_eq!(shared_icon(300, 0x0a), None);
        assert_eq!(shared_icon(-1, 0x0c), None);
    }

    #[test]
    fn file_urls_name_their_file() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let path = |s: &str| file_url_path(&w(s)).map(|p| String::from_utf16_lossy(&p));
        // A Steam game's shortcut, as its handler names its icon.
        assert_eq!(
            path("file:///C:/Program%20Files%20(x86)/Steam/steam/games/29492c.ico").as_deref(),
            Some(r"C:\Program Files (x86)\Steam\steam\games\29492c.ico")
        );
        // Escaped characters a path may hold, and the scheme in any case.
        assert_eq!(
            path("FILE:///C:/a/dir%20%231%20%2541/hk.ico").as_deref(),
            Some(r"C:\a\dir #1 %41\hk.ico")
        );
        // A location written with a variable, expanded after.
        assert_eq!(
            path("file:%25SystemRoot%25/System32/imageres.dll").as_deref(),
            Some(r"%SystemRoot%\System32\imageres.dll")
        );
        // Not a file: URL.
        assert_eq!(path("https://example.com/favicon.ico"), None);
        assert_eq!(path(r"C:\Windows\System32\shell32.dll"), None);
        assert_eq!(path("*"), None);
        assert_eq!(path("file:"), None);
        assert!(is_file_url(&w("File:///x")));
        assert!(!is_file_url(&w("files/x")));
    }

    #[test]
    fn coverage_comes_from_the_two_backgrounds() {
        // Opaque red: the same on both.
        assert_eq!(
            premultiplied([0, 0, 200, 0], [0, 0, 200, 0]),
            [0, 0, 200, 255]
        );
        // Not drawn: black on black, white on white.
        assert_eq!(
            premultiplied([0, 0, 0, 0], [255, 255, 255, 0]),
            [0, 0, 0, 0]
        );
        // Half covered white: 128 on black, 255 on white.
        assert_eq!(
            premultiplied([128, 128, 128, 0], [255, 255, 255, 0]),
            [128, 128, 128, 128]
        );
        // A colour never comes out over its coverage.
        assert_eq!(
            premultiplied([90, 90, 90, 0], [255, 250, 255, 0]),
            [90, 90, 90, 95]
        );
    }
}
