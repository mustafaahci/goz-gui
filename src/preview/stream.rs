//! The stream preview handlers read a file through, when they take one
//! (`IInitializeWithStream`): goz-gui opens the file sharing reading,
//! writing and deleting, so while it is previewed it can still be renamed,
//! moved or deleted (a handler that opens a file by its path may hold it
//! so that it cannot). It only reads; it lives on the preview's thread,
//! where the handler's process calls it.

use windows::Win32::Foundation::{
    E_NOTIMPL, FILETIME, S_FALSE, S_OK, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION,
};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, ISequentialStream_Impl, IStream, IStream_Impl, LOCKTYPE, STATFLAG,
    STATFLAG_NONAME, STATSTG, STGC, STGM_READ, STGTY_STREAM, STREAM_SEEK,
};
use windows::core::{HRESULT, PWSTR, Ref, implement};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING, ReadFile,
    SetFilePointerEx,
};

use crate::wide::nul;

/// A read-only stream over an open file.
#[implement(IStream)]
pub struct FileStream {
    file: usize,
    /// The file's path, NUL-terminated, for `Stat`.
    name: Vec<u16>,
}

impl FileStream {
    /// A stream over `path`, opened for reading and shared with everyone
    /// else; `None` if it cannot be opened.
    pub fn open(path: &[u16]) -> Option<IStream> {
        let name = nul(path);
        // SAFETY: a NUL-terminated path; the handle is owned by the stream
        // and closed as it drops.
        let file = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                core::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_SEQUENTIAL_SCAN,
                core::ptr::null_mut(),
            )
        };
        if file == INVALID_HANDLE_VALUE || file.is_null() {
            return None;
        }
        Some(
            FileStream {
                file: file as usize,
                name,
            }
            .into(),
        )
    }

    fn handle(&self) -> HANDLE {
        self.file as HANDLE
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        // SAFETY: the handle opened in `open`, closed once.
        unsafe { CloseHandle(self.handle()) };
    }
}

/// The last error as an HRESULT.
fn last_error() -> HRESULT {
    // SAFETY: reads this thread's last error.
    HRESULT::from_win32(unsafe { GetLastError() })
}

fn filetime(ft: windows_sys::Win32::Foundation::FILETIME) -> FILETIME {
    FILETIME {
        dwLowDateTime: ft.dwLowDateTime,
        dwHighDateTime: ft.dwHighDateTime,
    }
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        let mut got = 0u32;
        // SAFETY: the caller's buffer of `cb` bytes; `pcbread` is the
        // caller's or null.
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
    fn Seek(
        &self,
        dlibmove: i64,
        dworigin: STREAM_SEEK,
        plibnewposition: *mut u64,
    ) -> windows::core::Result<()> {
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

    fn SetSize(&self, _libnewsize: u64) -> windows::core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(
        &self,
        _pstm: Ref<IStream>,
        _cb: u64,
        _pcbread: *mut u64,
        _pcbwritten: *mut u64,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> windows::core::Result<()> {
        // Nothing is ever written.
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: &LOCKTYPE,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: u32,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, grfstatflag: &STATFLAG) -> windows::core::Result<()> {
        if pstatstg.is_null() {
            return Err(windows::Win32::Foundation::STG_E_INVALIDPOINTER.into());
        }
        // SAFETY: reads the file's information into a plain out-struct,
        // writes the caller's STATSTG, and gives it the name in memory the
        // caller frees (CoTaskMemAlloc).
        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = core::mem::zeroed();
            if GetFileInformationByHandle(self.handle(), &mut info) == 0 {
                return Err(last_error().into());
            }
            let mut name = PWSTR::null();
            if *grfstatflag != STATFLAG_NONAME {
                let bytes = self.name.len() * 2;
                let mem = CoTaskMemAlloc(bytes).cast::<u16>();
                if !mem.is_null() {
                    core::ptr::copy_nonoverlapping(self.name.as_ptr(), mem, self.name.len());
                    name = PWSTR(mem);
                }
            }
            pstatstg.write(STATSTG {
                pwcsName: name,
                r#type: STGTY_STREAM.0 as u32,
                cbSize: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
                mtime: filetime(info.ftLastWriteTime),
                ctime: filetime(info.ftCreationTime),
                atime: filetime(info.ftLastAccessTime),
                grfMode: STGM_READ,
                grfLocksSupported: 0,
                clsid: windows::core::GUID::zeroed(),
                grfStateBits: 0,
                reserved: 0,
            });
        }
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Com::{STREAM_SEEK_END, STREAM_SEEK_SET};

    #[test]
    fn the_stream_reads_seeks_and_lets_the_file_be_renamed() {
        let dir = std::env::temp_dir().join(format!("goz-gui-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"hello, stream")
            .unwrap();
        let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        let stream = FileStream::open(&wide).expect("opened");
        // SAFETY: calls on a live stream of this thread with buffers of
        // the sizes passed.
        unsafe {
            let mut buf = [0u8; 5];
            let mut got = 0u32;
            assert_eq!(
                stream.Read(buf.as_mut_ptr().cast(), 5, Some(&raw mut got)),
                S_OK
            );
            assert_eq!((&buf, got), (b"hello", 5));
            let mut at = 0u64;
            stream.Seek(-6, STREAM_SEEK_END, Some(&raw mut at)).unwrap();
            assert_eq!(at, 7);
            let mut rest = [0u8; 16];
            assert_eq!(
                stream.Read(rest.as_mut_ptr().cast(), 16, Some(&raw mut got)),
                S_FALSE
            );
            assert_eq!(&rest[..got as usize], b"stream");
            stream.Seek(0, STREAM_SEEK_SET, None).unwrap();
            let mut st: STATSTG = core::mem::zeroed();
            stream.Stat(&mut st, STATFLAG(0)).unwrap();
            assert_eq!(st.cbSize, 13);
            assert_eq!(st.pwcsName.to_string().unwrap(), path.to_string_lossy());
            CoTaskMemFree(Some(st.pwcsName.0.cast()));
            // Read only.
            assert_eq!(
                stream.Write(b"x".as_ptr().cast(), 1, None),
                STG_E_ACCESSDENIED
            );
        }
        // While it is open, the file can be renamed and deleted.
        let moved = dir.join("b.txt");
        std::fs::rename(&path, &moved).expect("renamed while open");
        drop(stream);
        std::fs::remove_file(&moved).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }
}
