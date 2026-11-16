//! The stream preview handlers read a file through, when they take one
//! (`IInitializeWithStream`): goz-gui opens the file sharing reading,
//! writing and deleting, so while it is previewed it can still be renamed,
//! moved or deleted (a handler that opens a file by its path may hold it
//! so that it cannot). It only reads; it lives on the preview's thread,
//! where the handler's process calls it. The stream itself is
//! `win::preview::Stream` (whose tests read one opened this way).

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::win::preview::Stream;

/// A read-only stream over an open file.
pub struct FileStream;

impl FileStream {
    /// A stream over `path`, opened for reading and shared with everyone
    /// else; `None` if it cannot be opened.
    pub fn open(path: &[u16]) -> Option<Stream> {
        Stream::open(
            path,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_SEQUENTIAL_SCAN,
        )
    }
}
