//! The stream preview handlers read a file through, when they take one
//! (`IInitializeWithStream`): goz-gui opens the file sharing reading,
//! writing and deleting, so while it is previewed it can still be renamed,
//! moved or deleted (a handler that opens a file by its path may hold it
//! so that it cannot). It only reads; it lives on the preview's thread,
//! where the handler's process calls it. The stream itself is
//! `win::ext::b7::file_stream`.

use windows::Win32::System::Com::IStream;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::win::ext::b7;

/// A read-only stream over an open file.
pub struct FileStream;

impl FileStream {
    /// A stream over `path`, opened for reading and shared with everyone
    /// else; `None` if it cannot be opened.
    pub fn open(path: &[u16]) -> Option<IStream> {
        b7::file_stream(
            path,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_SEQUENTIAL_SCAN,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use windows::Win32::Foundation::{S_FALSE, S_OK, STG_E_ACCESSDENIED};
    use windows::Win32::System::Com::{STREAM_SEEK_END, STREAM_SEEK_SET};

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
        let stream = FileStream::open(&wide).expect("opened");
        let mut buf = [0u8; 5];
        let (hr, got) = b7::stream_read(&stream, &mut buf);
        assert_eq!(hr, S_OK);
        assert_eq!((&buf, got), (b"hello", 5));
        let at = b7::stream_seek(&stream, -6, STREAM_SEEK_END).unwrap();
        assert_eq!(at, 7);
        let mut rest = [0u8; 16];
        let (hr, got) = b7::stream_read(&stream, &mut rest);
        assert_eq!(hr, S_FALSE);
        assert_eq!(&rest[..got as usize], b"stream");
        b7::stream_seek(&stream, 0, STREAM_SEEK_SET).unwrap();
        let (size, name) = b7::stream_stat(&stream, true).unwrap();
        assert_eq!(size, 13);
        assert_eq!(name.as_deref(), Some(&*path.to_string_lossy()));
        // Read only.
        assert_eq!(b7::stream_write(&stream, b"x"), STG_E_ACCESSDENIED);
        // While it is open, the file can be renamed and deleted.
        let moved = dir.join("b.txt");
        std::fs::rename(&path, &moved).expect("renamed while open");
        drop(stream);
        std::fs::remove_file(&moved).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }
}
