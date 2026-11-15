//! Batch 5 of the move onto `win`: what the bookmark, filter, history and
//! run dialogs and Advanced Search need that the layer did not have.

use windows_sys::Win32::Foundation::{HWND, LRESULT, RECT, SYSTEMTIME};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, GetDIBits, HBITMAP,
    HDC, RGBQUAD, SetDIBits,
};
use windows_sys::Win32::UI::Controls::{
    COMBOBOXINFO, CloseThemeData, DTM_GETMONTHCAL, DTM_GETSYSTEMTIME, DTM_SETSYSTEMTIME,
    DrawThemeBackground, GDT_NONE, GDT_VALID, GetComboBoxInfo, HIMAGELIST, HTHEME, OpenThemeData,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, SHFILEINFOW, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_PAINT;

use crate::win::{Msg, of_class, send_raw, wz};

/// The system image list and the index in it of the icon the shell shows
/// for an item named `name` with `attributes` (FILE_ATTRIBUTE_*), looked up
/// by the name and the attributes alone (SHGFI_USEFILEATTRIBUTES: no file
/// is read). The small icons' list when `small`. The list is the system's,
/// shared: never to be destroyed. Null (and 0) if the shell gave none.
pub fn shell_icon_index(name: &str, attributes: u32, small: bool) -> (HIMAGELIST, i32) {
    let z = wz(name);
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES;
    if small {
        flags |= SHGFI_SMALLICON;
    }
    // SAFETY: a NUL-terminated name read during the call (a name, not an
    // item ID list: SHGFI_PIDL is not among the flags) and an out-struct of
    // the size passed. SHGFI_SYSICONINDEX answers with the system image
    // list, which is not the caller's to free, and no icon is made
    // (SHGFI_ICON is not asked for).
    let list = unsafe {
        SHGetFileInfoW(
            z.as_ptr(),
            attributes,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    (list as HIMAGELIST, info.iIcon)
}

/// The combo box's parts (its button, item area, edit and list) as the
/// system's GetComboBoxInfo answers them; `None` for a window that is no
/// combo box. (`combo::info` sends CB_GETCOMBOBOXINFO itself, through the
/// box's subclasses, which `dialog::fill_later`'s takes for a use of the
/// list.)
pub fn combo_box_info(c: HWND) -> Option<COMBOBOXINFO> {
    let mut ci = COMBOBOXINFO {
        cbSize: size_of::<COMBOBOXINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct, its size set.
    (unsafe { GetComboBoxInfo(c, &mut ci) } != 0).then_some(ci)
}

/// A window class's theme data opened for a window with OpenThemeData (so
/// at the DPI the system picks for that window), closed when this drops.
/// Only made here, so the handle is always one uxtheme gave.
pub struct WindowTheme(HTHEME);

impl WindowTheme {
    /// The theme data of `classes` ("Combobox") for `hwnd`; `None` without
    /// visual styles.
    pub fn open(hwnd: HWND, classes: &str) -> Option<WindowTheme> {
        let z = wz(classes);
        // SAFETY: a NUL-terminated class list read during the call.
        let t = unsafe { OpenThemeData(hwnd, z.as_ptr()) };
        (t != 0).then_some(WindowTheme(t))
    }

    /// Draws a part's background in `rc` (clipped to `clip`).
    pub fn draw_background(&self, hdc: HDC, part: i32, state: i32, rc: &RECT, clip: Option<&RECT>) {
        let clip = clip.map_or(core::ptr::null(), |c| c as *const RECT);
        // SAFETY: theme data this owns; rectangles read during the call.
        unsafe { DrawThemeBackground(self.0, hdc, part, state, rc, clip) };
    }
}

impl Drop for WindowTheme {
    fn drop(&mut self) {
        // SAFETY: theme data this owns, closed once.
        unsafe { CloseThemeData(self.0) };
    }
}

/// The date and time picker's class. Its messages (DTM_*) are its own:
/// DTM_FIRST is LVM_FIRST, so each is sent only to a picker.
pub const DATE_PICKER: &str = "SysDateTimePick32";

/// The picker's date and time; `None` while it has none (its check box
/// cleared, with DTS_SHOWNONE) and for a window that is no picker.
pub fn picker_time(picker: HWND) -> Option<SYSTEMTIME> {
    // GDT_VALID is 0, which is also what a message to no window answers:
    // no picker, no time.
    let picker = of_class(picker, DATE_PICKER);
    if picker.is_null() {
        return None;
    }
    let mut st = SYSTEMTIME::default();
    // SAFETY: DTM_GETSYSTEMTIME's lParam is a SYSTEMTIME the picker fills,
    // alive for the call; sent only to a picker.
    let r = unsafe { send_raw(picker, DTM_GETSYSTEMTIME, 0, (&raw mut st) as isize) };
    (r == GDT_VALID as isize).then_some(st)
}

/// Sets the picker's date and time, or with `None` takes it away (clears
/// its check box, with DTS_SHOWNONE); false if the picker refused.
pub fn set_picker_time(picker: HWND, time: Option<&SYSTEMTIME>) -> bool {
    let (flag, l) = match time {
        Some(st) => (GDT_VALID, core::ptr::from_ref(st) as isize),
        None => (GDT_NONE, 0),
    };
    // SAFETY: DTM_SETSYSTEMTIME's lParam is a SYSTEMTIME read during the
    // call (not read with GDT_NONE, and null then); sent only to a picker.
    unsafe {
        send_raw(
            of_class(picker, DATE_PICKER),
            DTM_SETSYSTEMTIME,
            flag as usize,
            l,
        ) != 0
    }
}

/// The month calendar the picker has dropped down; null while it has none.
pub fn picker_calendar(picker: HWND) -> HWND {
    // SAFETY: DTM_GETMONTHCAL has no parameters; sent only to a picker.
    unsafe { send_raw(of_class(picker, DATE_PICKER), DTM_GETMONTHCAL, 0, 0) as HWND }
}

/// From a subclass's handling of a WM_PAINT `m`: the next procedure in the
/// window's chain (in the end the window's own) paints into `hdc` instead
/// of the window, as the common controls do with a WM_PAINT that carries a
/// DC. 0 for a message that is not a WM_PAINT the system delivered.
pub fn paint_next_into(m: &Msg, hdc: HDC) -> LRESULT {
    if m.id != WM_PAINT || !m.is_delivered() {
        return 0;
    }
    // SAFETY: WM_PAINT with a DC handle, which GDI checks, and no lParam:
    // no pointers. To the window the message was delivered to, so on its
    // own thread.
    unsafe { DefSubclassProc(m.hwnd, WM_PAINT, hdc as usize, 0) }
}

/// A BITMAPINFO with room for any colour table GDI may write after the
/// header (GetDIBits writes one for some formats).
#[repr(C)]
struct DibInfo {
    header: BITMAPINFOHEADER,
    colors: [RGBQUAD; 256],
}

impl DibInfo {
    /// 32 bits a pixel, top row first, `w` × `h`.
    fn top_down_32(w: i32, h: i32) -> DibInfo {
        DibInfo {
            header: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            colors: [RGBQUAD::default(); 256],
        }
    }
}

/// How many pixels `w` × `h` is; `None` for an empty or negative size.
fn pixel_count(w: i32, h: i32) -> Option<usize> {
    if w <= 0 || h <= 0 {
        return None;
    }
    usize::try_from(w)
        .ok()?
        .checked_mul(usize::try_from(h).ok()?)
}

/// A DIB section of `w` × `h` pixels of 32 bits, top row first, for a
/// memory DC to draw into; null if it could not be made. The caller owns it
/// (`gdi::Owned`). Its pixels are read and written with [`dib_pixels`] and
/// [`set_dib_pixels`], which copy them: no reference into the section's
/// memory is handed out, as it goes with the bitmap.
pub fn create_dib_32(hdc: HDC, w: i32, h: i32) -> HBITMAP {
    if pixel_count(w, h).is_none() {
        return core::ptr::null_mut();
    }
    let info = DibInfo::top_down_32(w, h);
    let mut bits = core::ptr::null_mut();
    // SAFETY: a BITMAPINFO (32-bit BI_RGB: no colour table read) alive for
    // the call; `bits` receives the section's memory, which is not kept.
    // No file mapping. GDI checks the DC.
    unsafe {
        CreateDIBSection(
            hdc,
            (&raw const info).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
            &mut bits,
            core::ptr::null_mut(),
            0,
        )
    }
}

/// The pixels of bitmap `bmp` as `w` × `h` pixels of 32 bits (BGRA), top
/// row first, which GetDIBits copies out; `None` if it copied none. (GDI
/// wants the bitmap out of every DC meanwhile.)
pub fn dib_pixels(hdc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u32>> {
    let len = pixel_count(w, h)?;
    let mut px = vec![0u32; len];
    let mut info = DibInfo::top_down_32(w, h);
    // SAFETY: a buffer of `w * h` pixels of 32 bits, what the header
    // describes for `h` lines of `w` (each line `4 * w` bytes, already
    // DWORD-aligned), and a header with room after it for any colour table;
    // both outlive the call. GDI checks the DC and the bitmap.
    let lines = unsafe {
        GetDIBits(
            hdc,
            bmp,
            0,
            h as u32,
            px.as_mut_ptr().cast(),
            (&raw mut info).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
        )
    };
    (lines > 0).then_some(px)
}

/// Sets the pixels of bitmap `bmp` from `px`, `w` × `h` pixels of 32 bits
/// (BGRA), top row first, which SetDIBits copies in; false if `px` holds
/// fewer or it could not. (GDI wants the bitmap out of every DC meanwhile.)
pub fn set_dib_pixels(hdc: HDC, bmp: HBITMAP, w: i32, h: i32, px: &[u32]) -> bool {
    let Some(len) = pixel_count(w, h) else {
        return false;
    };
    if px.len() < len {
        return false;
    }
    let info = DibInfo::top_down_32(w, h);
    // SAFETY: at least `w * h` pixels of 32 bits, what the header describes,
    // and the header (32-bit BI_RGB: no colour table read), both read during
    // the call. GDI checks the DC and the bitmap.
    unsafe {
        SetDIBits(
            hdc,
            bmp,
            0,
            h as u32,
            px.as_ptr().cast(),
            (&raw const info).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
        ) > 0
    }
}

/// Asks for a folder with the system's folder picker (IFileOpenDialog with
/// FOS_PICKFOLDERS) titled `title`, over `owner`, starting in `start` (if
/// it names one); its path, or `None` when cancelled, or when the picker
/// could not be made (as on a thread without COM).
pub fn pick_folder(owner: HWND, start: &str, title: &str) -> Option<String> {
    use windows::Win32::Foundation::HWND as WHWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
        IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::{HSTRING, PCWSTR};
    let title = wz(title);
    // SAFETY: the system's folder picker, made, shown modal over `owner`
    // (a handle the system checks) and read on this thread (making it
    // fails on a thread without COM); the strings outlive the calls they
    // are given to, and the path the picker gives is copied, then freed
    // with the COM allocator.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(
                options
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR,
            )
            .ok()?;
        let start = start.trim();
        if !start.is_empty()
            && let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start), None)
        {
            let _ = dialog.SetFolder(&folder);
        }
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        dialog.Show(Some(WHWND(owner))).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_counts_its_pixels() {
        assert_eq!(pixel_count(3, 4), Some(12));
        assert_eq!(pixel_count(0, 4), None);
        assert_eq!(pixel_count(3, -1), None);
    }

    #[test]
    fn pixels_go_in_and_come_out() {
        use crate::win::gdi;
        let dc = gdi::MemDc::compatible(core::ptr::null_mut());
        let bmp = gdi::Owned::new(create_dib_32(dc.hdc(), 2, 2));
        assert!(!bmp.get().is_null());
        let px = [0x0011_2233, 0x0044_5566, 0x0077_8899, 0x00aa_bbcc];
        assert!(set_dib_pixels(dc.hdc(), bmp.get(), 2, 2, &px));
        assert_eq!(
            dib_pixels(dc.hdc(), bmp.get(), 2, 2).as_deref(),
            Some(&px[..])
        );
        // Too few pixels for the size: nothing is read past them.
        assert!(!set_dib_pixels(dc.hdc(), bmp.get(), 2, 2, &px[..3]));
    }

    #[test]
    fn picker_messages_go_only_to_pickers() {
        // A window that is not there: no picker, nothing sent.
        assert!(picker_time(core::ptr::null_mut()).is_none());
        assert!(picker_calendar(core::ptr::null_mut()).is_null());
    }
}
