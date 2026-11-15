//! Preview handlers (IPreviewHandler): COM objects, usually in a
//! surrogate process, that draw a file into a window they are given; the
//! frame goz-gui gives them to pass back the keys they do not use, and the
//! read-only stream they can read their file through.
//!
//! The frame and the stream are objects goz-gui implements itself, as
//! private types: what is handed out is only a wrapper of their interface
//! ([`Frame`], [`Stream`]), configured with Rust values and closures. Like
//! every COM object here they stay on the thread (the apartment) that made
//! them.

use windows::Win32::Foundation::{
    COLORREF, E_NOTIMPL, E_POINTER, FILETIME as WFILETIME, HWND as WHWND, RECT as WRECT, S_FALSE,
    S_OK, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER,
};
use windows::Win32::Graphics::Gdi::{
    FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, LOGFONTW as WLOGFONTW,
};
use windows::Win32::System::Com::{
    CLSCTX, CoCreateInstance, CoTaskMemAlloc, ISequentialStream_Impl, IStream, IStream_Impl,
    LOCKTYPE, STATFLAG, STATFLAG_NONAME, STATSTG, STGC, STGM_READ, STGTY_STREAM, STREAM_SEEK,
};
use windows::Win32::System::Ole::IObjectWithSite;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    IInitializeWithItem, IPreviewHandler, IPreviewHandlerFrame, IPreviewHandlerFrame_Impl,
    IPreviewHandlerVisuals, PREVIEWHANDLERFRAMEINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{HACCEL as WHACCEL, MSG as WMSG};
use windows::core::{GUID, HRESULT, Interface, PCWSTR, PWSTR, Ref, Result, implement};
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GENERIC_READ, HANDLE, HWND, INVALID_HANDLE_VALUE, POINT,
};
use windows_sys::Win32::Graphics::Gdi::LOGFONTW;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_MODE, GetFileInformationByHandle, OPEN_EXISTING, ReadFile, SetFilePointerEx,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{HACCEL, MSG};

use super::shell::ShellItem;
use super::{nul_ended, sys, wz_units};

/// A preview handler (IPreviewHandler).
pub struct Handler(IPreviewHandler);

impl Handler {
    /// A new one of class `clsid`, made as `context` says
    /// (CLSCTX_LOCAL_SERVER…). COM must be initialised on this thread.
    pub fn create(clsid: &GUID, context: CLSCTX) -> Result<Handler> {
        // SAFETY: a class id read during the call, no outer object; the
        // object comes back owned.
        unsafe { CoCreateInstance(clsid, None, context) }.map(Handler)
    }

    /// Gives it its file as a stream, if it takes one
    /// (IInitializeWithStream): `open` makes the stream once that is known.
    /// False if it takes none, `open` made none, or it refused it.
    pub fn init_with_stream(&self, open: impl FnOnce() -> Option<Stream>) -> bool {
        let Ok(init) = self.0.cast::<IInitializeWithStream>() else {
            return false;
        };
        let Some(stream) = open() else {
            return false;
        };
        // SAFETY: a stream of this thread, which the handler keeps a
        // reference to (counted) if it wants.
        unsafe { init.Initialize(&stream.0, STGM_READ.0) }.is_ok()
    }

    /// Gives it its file by path, if it takes one (IInitializeWithFile);
    /// false if it does not or refused it.
    pub fn init_with_file(&self, path: &[u16]) -> bool {
        let path = nul_ended(path);
        let Ok(init) = self.0.cast::<IInitializeWithFile>() else {
            return false;
        };
        // SAFETY: a NUL-terminated path read during the call.
        unsafe { init.Initialize(PCWSTR(path.as_ptr()), STGM_READ.0) }.is_ok()
    }

    /// Gives it its file as a shell item, if it takes one
    /// (IInitializeWithItem); false if it does not, the shell has no item
    /// there, or it refused it.
    pub fn init_with_item(&self, path: &[u16]) -> bool {
        let Ok(init) = self.0.cast::<IInitializeWithItem>() else {
            return false;
        };
        let Some(item) = ShellItem::from_parsing_name(path) else {
            return false;
        };
        // SAFETY: an item of this thread, kept (counted) if it wants.
        unsafe { init.Initialize(&item.0, STGM_READ.0) }.is_ok()
    }

    /// The colours and font it draws with, if it takes them
    /// (IPreviewHandlerVisuals); what it refuses is let be.
    pub fn set_visuals(&self, bg: u32, fg: u32, font: &LOGFONTW) {
        let Ok(visuals) = self.0.cast::<IPreviewHandlerVisuals>() else {
            return;
        };
        let lf = WLOGFONTW {
            lfHeight: font.lfHeight,
            lfWidth: font.lfWidth,
            lfEscapement: font.lfEscapement,
            lfOrientation: font.lfOrientation,
            lfWeight: font.lfWeight,
            lfItalic: font.lfItalic,
            lfUnderline: font.lfUnderline,
            lfStrikeOut: font.lfStrikeOut,
            lfCharSet: FONT_CHARSET(font.lfCharSet),
            lfOutPrecision: FONT_OUTPUT_PRECISION(font.lfOutPrecision),
            lfClipPrecision: FONT_CLIP_PRECISION(font.lfClipPrecision),
            lfQuality: FONT_QUALITY(font.lfQuality),
            lfPitchAndFamily: font.lfPitchAndFamily,
            lfFaceName: font.lfFaceName,
        };
        // SAFETY: values, and a font structure read during the call.
        unsafe {
            let _ = visuals.SetBackgroundColor(COLORREF(bg));
            let _ = visuals.SetTextColor(COLORREF(fg));
            let _ = visuals.SetFont(&lf);
        }
    }

    /// Its site (IObjectWithSite), if it takes one: the frame it passes keys
    /// to, or none.
    pub fn set_site(&self, frame: Option<&Frame>) {
        let Ok(site) = self.0.cast::<IObjectWithSite>() else {
            return;
        };
        // SAFETY: a frame of this thread (kept, counted, if it wants), or
        // none.
        let _ = unsafe {
            match frame {
                Some(frame) => site.SetSite(&frame.0),
                None => site.SetSite(None::<&windows::core::IUnknown>),
            }
        };
    }

    /// The window it draws in, and the size it takes up there (SetWindow).
    pub fn set_window(&self, parent: HWND, size: (i32, i32)) -> Result<()> {
        let rc = rect_of(size);
        // SAFETY: a window handle (the system checks it) and a rectangle
        // read during the call.
        unsafe { self.0.SetWindow(WHWND(parent), &rc) }
    }

    /// The size it takes up in its window (SetRect).
    pub fn set_rect(&self, size: (i32, i32)) -> Result<()> {
        let rc = rect_of(size);
        // SAFETY: a rectangle read during the call.
        unsafe { self.0.SetRect(&rc) }
    }

    /// Shows its file (DoPreview).
    pub fn do_preview(&self) -> Result<()> {
        // SAFETY: no pointers.
        unsafe { self.0.DoPreview() }
    }

    /// Lets its file go (Unload).
    pub fn unload(&self) -> Result<()> {
        // SAFETY: no pointers.
        unsafe { self.0.Unload() }
    }

    /// Gives its window the keyboard (SetFocus).
    pub fn set_focus(&self) -> Result<()> {
        // SAFETY: no pointers.
        unsafe { self.0.SetFocus() }
    }
}

/// A rectangle at 0, 0 of `size`, at least a pixel each way.
fn rect_of(size: (i32, i32)) -> WRECT {
    WRECT {
        left: 0,
        top: 0,
        right: size.0.max(1),
        bottom: size.1.max(1),
    }
}

/// A preview handler's frame (IPreviewHandlerFrame): the object it passes
/// back the keys it does not use.
#[derive(Clone)]
pub struct Frame(IPreviewHandlerFrame);

impl Frame {
    /// A frame whose GetWindowContext answers the accelerator table and its
    /// count `context` gives, and whose TranslateAccelerator hands
    /// `translate` a copy of each message the handler passes on (true: it
    /// was one of the window's keys, S_OK; false: S_FALSE, the handler's to
    /// use).
    pub fn new(
        context: impl Fn() -> (HACCEL, u32) + 'static,
        translate: impl Fn(&MSG) -> bool + 'static,
    ) -> Frame {
        Frame(
            KeyFrame {
                context: Box::new(context),
                translate: Box::new(translate),
            }
            .into(),
        )
    }
}

/// The object behind a [`Frame`]. Not agile: calls from elsewhere come
/// through its thread's apartment, one at a time, as its closures need.
#[implement(IPreviewHandlerFrame, Agile = false)]
struct KeyFrame {
    context: Box<dyn Fn() -> (HACCEL, u32)>,
    translate: Box<dyn Fn(&MSG) -> bool>,
}

impl IPreviewHandlerFrame_Impl for KeyFrame_Impl {
    fn GetWindowContext(&self) -> Result<PREVIEWHANDLERFRAMEINFO> {
        let (table, count) = (self.context)();
        Ok(PREVIEWHANDLERFRAMEINFO {
            haccel: WHACCEL(table),
            cAccelEntries: count,
        })
    }

    fn TranslateAccelerator(&self, pmsg: *const WMSG) -> Result<()> {
        // SAFETY: the handler passes a message of its own, alive across the
        // call, or null; it is copied.
        let msg = unsafe { pmsg.as_ref() }.ok_or(windows::core::Error::from(E_POINTER))?;
        let msg = MSG {
            hwnd: msg.hwnd.0,
            message: msg.message,
            wParam: msg.wParam.0,
            lParam: msg.lParam.0,
            time: msg.time,
            pt: POINT {
                x: msg.pt.x,
                y: msg.pt.y,
            },
        };
        if (self.translate)(&msg) {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }
}

/// A read-only stream over a file (IStream), for the handlers that take
/// one; the file is closed when the last reference to the stream goes.
pub struct Stream(IStream);

impl Stream {
    /// A stream over the file at `path`, opened for reading with
    /// FILE_SHARE_* `share` and FILE_FLAG_* `flags`; `None` if it cannot be
    /// opened. Its `Stat` names it by `path`.
    pub fn open(
        path: &[u16],
        share: FILE_SHARE_MODE,
        flags: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> Option<Stream> {
        let name = wz_units(path);
        // Never overlapped: `Read` reads synchronously into the caller's
        // buffer, which an overlapped read could still be filling after the
        // call returns.
        let flags = flags & !FILE_FLAG_OVERLAPPED;
        // SAFETY: a NUL-terminated path; no security attributes or
        // template; the handle is owned by the stream and closed as it
        // drops.
        let file = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                share,
                core::ptr::null(),
                OPEN_EXISTING,
                flags,
                core::ptr::null_mut(),
            )
        };
        if file == INVALID_HANDLE_VALUE || file.is_null() {
            return None;
        }
        Some(Stream(
            FileStream {
                file: file as usize,
                name,
            }
            .into(),
        ))
    }
}

/// The object behind a [`Stream`]: an open file, read only.
#[implement(IStream)]
struct FileStream {
    file: usize,
    /// The file's path, NUL-terminated, for `Stat`.
    name: Vec<u16>,
}

impl FileStream {
    fn handle(&self) -> HANDLE {
        self.file as HANDLE
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        // SAFETY: the handle `Stream::open` opened, closed once.
        unsafe { CloseHandle(self.handle()) };
    }
}

/// The last error as an HRESULT.
fn last_error() -> HRESULT {
    HRESULT::from_win32(sys::last_error())
}

fn filetime(ft: FILETIME) -> WFILETIME {
    WFILETIME {
        dwLowDateTime: ft.dwLowDateTime,
        dwHighDateTime: ft.dwHighDateTime,
    }
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        let mut got = 0u32;
        // SAFETY: the caller's buffer of `cb` bytes, as IStream::Read has it
        // (COM passes the caller's, or its own copy across processes);
        // `pcbread` is the caller's or null.
        unsafe {
            if ReadFile(
                self.handle(),
                pv.cast(),
                cb,
                &mut got,
                core::ptr::null_mut(),
            ) == 0
            {
                return last_error();
            }
            if !pcbread.is_null() {
                *pcbread = got;
            }
        }
        // Fewer bytes than asked for: the end of the file.
        if got == cb { S_OK } else { S_FALSE }
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for FileStream_Impl {
    fn Seek(&self, dlibmove: i64, dworigin: STREAM_SEEK, plibnewposition: *mut u64) -> Result<()> {
        let mut at = 0i64;
        // SAFETY: STREAM_SEEK_SET, _CUR and _END are FILE_BEGIN, _CURRENT
        // and _END; the new position goes to the caller's pointer, if any.
        unsafe {
            if SetFilePointerEx(self.handle(), dlibmove, &mut at, dworigin.0) == 0 {
                return Err(last_error().into());
            }
            if !plibnewposition.is_null() {
                *plibnewposition = at as u64;
            }
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(
        &self,
        _pstm: Ref<IStream>,
        _cb: u64,
        _pcbread: *mut u64,
        _pcbwritten: *mut u64,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> Result<()> {
        // Nothing is ever written.
        Ok(())
    }

    fn Revert(&self) -> Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: &LOCKTYPE) -> Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: u32) -> Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, grfstatflag: &STATFLAG) -> Result<()> {
        if pstatstg.is_null() {
            return Err(STG_E_INVALIDPOINTER.into());
        }
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file's information into a plain out-struct.
        if unsafe { GetFileInformationByHandle(self.handle(), &mut info) } == 0 {
            return Err(last_error().into());
        }
        let mut name = PWSTR::null();
        if *grfstatflag != STATFLAG_NONAME {
            let bytes = self.name.len() * 2;
            // SAFETY: memory the caller frees (CoTaskMemFree), filled with
            // the NUL-terminated name, `name.len()` units, as allocated.
            unsafe {
                let mem = CoTaskMemAlloc(bytes).cast::<u16>();
                if !mem.is_null() {
                    core::ptr::copy_nonoverlapping(self.name.as_ptr(), mem, self.name.len());
                    name = PWSTR(mem);
                }
            }
        }
        // SAFETY: the caller's STATSTG, not null (checked above), written
        // whole.
        unsafe {
            pstatstg.write(STATSTG {
                pwcsName: name,
                r#type: STGTY_STREAM.0 as u32,
                cbSize: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
                mtime: filetime(info.ftLastWriteTime),
                ctime: filetime(info.ftCreationTime),
                atime: filetime(info.ftLastAccessTime),
                grfMode: STGM_READ,
                grfLocksSupported: 0,
                clsid: GUID::zeroed(),
                grfStateBits: 0,
                reserved: 0,
            });
        }
        Ok(())
    }

    fn Clone(&self) -> Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use windows::Win32::System::Com::{CoTaskMemFree, STREAM_SEEK_END, STREAM_SEEK_SET};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    /// Reads from `stream` into `buf` (ISequentialStream::Read): its
    /// answer, and how many bytes came.
    fn read(stream: &Stream, buf: &mut [u8]) -> (HRESULT, u32) {
        let cb = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let mut got = 0u32;
        // SAFETY: a buffer of at least `cb` bytes and a count out, both
        // alive during the call.
        let hr = unsafe {
            stream
                .0
                .Read(buf.as_mut_ptr().cast(), cb, Some(&raw mut got))
        };
        (hr, got.min(cb))
    }

    /// Moves `stream`'s position (Seek, STREAM_SEEK_*); the new one.
    fn seek(stream: &Stream, by: i64, from: STREAM_SEEK) -> Result<u64> {
        let mut at = 0u64;
        // SAFETY: an out-value alive during the call.
        unsafe { stream.0.Seek(by, from, Some(&raw mut at)) }?;
        Ok(at)
    }

    /// `stream`'s size and name (Stat; the name it allocates is copied and
    /// freed here).
    fn stat(stream: &Stream) -> Result<(u64, Option<String>)> {
        let mut st = STATSTG::default();
        // SAFETY: an out-struct alive during the call.
        unsafe { stream.0.Stat(&mut st, STATFLAG(0)) }?;
        let name = (!st.pwcsName.is_null()).then(|| {
            // SAFETY: the NUL-terminated name the stream allocated for its
            // caller, copied, then freed once.
            unsafe {
                let name = String::from_utf16_lossy(st.pwcsName.as_wide());
                CoTaskMemFree(Some(st.pwcsName.0.cast()));
                name
            }
        });
        Ok((st.cbSize, name))
    }

    #[test]
    fn the_stream_reads_seeks_and_lets_the_file_be_renamed() {
        let dir = crate::store::file::tests::fresh_dir("goz-gui-stream", "");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"hello, stream")
            .unwrap();
        let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        // Shared with everyone, as the preview pane opens it.
        let stream = Stream::open(
            &wide,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_SEQUENTIAL_SCAN,
        )
        .expect("opened");
        let mut buf = [0u8; 5];
        let (hr, got) = read(&stream, &mut buf);
        assert_eq!(hr, S_OK);
        assert_eq!((&buf, got), (b"hello", 5));
        let at = seek(&stream, -6, STREAM_SEEK_END).unwrap();
        assert_eq!(at, 7);
        let mut rest = [0u8; 16];
        let (hr, got) = read(&stream, &mut rest);
        assert_eq!(hr, S_FALSE);
        assert_eq!(&rest[..got as usize], b"stream");
        seek(&stream, 0, STREAM_SEEK_SET).unwrap();
        let (size, name) = stat(&stream).unwrap();
        assert_eq!(size, 13);
        assert_eq!(name.as_deref(), Some(&*path.to_string_lossy()));
        // Read only.
        // SAFETY: one byte read during the call; no count asked for.
        let wrote = unsafe { stream.0.Write(b"x".as_ptr().cast(), 1, None) };
        assert_eq!(wrote, STG_E_ACCESSDENIED);
        // While it is open, the file can be renamed and deleted.
        let moved = dir.join("b.txt");
        std::fs::rename(&path, &moved).expect("renamed while open");
        drop(stream);
        std::fs::remove_file(&moved).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }
}
