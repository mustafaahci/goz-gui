//! W9 stream B2: reading a file for one term of goz-core's exact plan
//! (`super::plan::plan_exact`), with goz-core's own reading, as the goz
//! CLI reads: Everything's plain-text extensions by their byte order mark,
//! other files as bytes in each encoding, nothing from a file Windows reads
//! through the null filter (`.exe`, `.zip`), and the term's modifiers and
//! value as gozd reads a name term's. Files whose data is not on the disk
//! (offline, in the cloud) are not read, as `super::scan` leaves them.

use std::collections::HashMap;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Mutex;

use goz_core::query::{ContentEnv, ContentTerm, TextFilter};

use super::scan::Unread;

/// The null filter's class id: Windows reads no text from such a file.
const NULL_FILTER: &str = "{098F2470-BAE0-11CD-B579-08002B30BFEB}";
/// `FILE_ATTRIBUTE_OFFLINE`, `FILE_ATTRIBUTE_RECALL_ON_OPEN` and
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`: the data is not on the disk.
const NOT_HERE: u32 = 0x1000 | 0x4_0000 | 0x40_0000;
/// `FILE_FLAG_SEQUENTIAL_SCAN`.
const SEQUENTIAL_SCAN: u32 = 0x0800_0000;

/// What goz-core's reading needs from Windows: the ANSI code page, and
/// each extension's filter (looked up once).
pub struct Env {
    filters: Mutex<HashMap<String, TextFilter>>,
}

impl Env {
    pub fn new() -> Env {
        Env {
            filters: Mutex::new(HashMap::new()),
        }
    }
}

/// The env of one read: the run's, and its stop.
struct Reading<'a> {
    env: &'a Env,
    stop: &'a dyn Fn() -> bool,
}

impl ContentEnv for Reading<'_> {
    fn ansi(&self, bytes: &[u8], out: &mut String) {
        goz_winfs::ansi_to_string(bytes, out);
    }

    fn filter(&self, ext: &str) -> TextFilter {
        let mut filters = self.env.filters.lock().unwrap_or_else(|e| e.into_inner());
        *filters
            .entry(ext.to_owned())
            .or_insert_with(|| match goz_winfs::persistent_handler(ext) {
                None => TextFilter::None,
                Some(id) if id.eq_ignore_ascii_case(NULL_FILTER) => TextFilter::Null,
                Some(_) => TextFilter::Other,
            })
    }

    fn cancelled(&self) -> bool {
        (self.stop)()
    }
}

/// Whether the file at `path`, named `name`, holds `term`.
pub fn search(
    term: &ContentTerm,
    path: &Path,
    name: &str,
    env: &Env,
    stop: &dyn Fn() -> bool,
) -> Result<bool, Unread> {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.file_attributes() & NOT_HERE != 0
    {
        return Err(Unread::Offline);
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(SEQUENTIAL_SCAN)
        .open(path)
        .map_err(|_| Unread::Failed)?;
    let reading = Reading { env, stop };
    match term.search(&mut file, name, &reading) {
        _ if stop() => Err(Unread::Stopped),
        Ok(found) => Ok(found),
        Err(_) => Err(Unread::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::ContentQuery;

    #[test]
    fn terms_read_as_goz_reads_them() {
        let dir = std::env::temp_dir().join(format!("goz-gui-w9-exact-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Chicken Noodle".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let files = [
            ("a.txt", b"chicken noodle soup".to_vec()),
            ("b.txt", utf16),
            ("c.md", b"tomato soup".to_vec()),
        ];
        for (name, body) in &files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let env = Env::new();
        let never = || false;
        let read = |query: &str, name: &str| {
            let split = ContentQuery::new(query, false).expect("parses");
            search(&split.terms()[0], &dir.join(name), name, &env, &never)
        };
        assert_eq!(read("content:\"chicken noodle\"", "a.txt"), Ok(true));
        assert_eq!(read("content:\"chicken noodle\"", "b.txt"), Ok(true));
        assert_eq!(read("content:\"chicken noodle\"", "c.md"), Ok(false));
        // A group value, a regex, whole words: as gozd reads a name term.
        assert_eq!(read("content:<tomato|beef>", "c.md"), Ok(true));
        assert_eq!(read("regex:content:chick.n", "a.txt"), Ok(true));
        assert_eq!(read("ww:content:chick", "a.txt"), Ok(false));
        assert_eq!(read("case:content:Chicken", "a.txt"), Ok(false));
        // A file that is not there: not read, counted as such.
        assert_eq!(read("content:x", "gone.txt"), Err(Unread::Failed));
        // Stopped: no answer.
        let split = ContentQuery::new("content:soup", false).unwrap();
        let stopped = || true;
        assert_eq!(
            search(&split.terms()[0], &dir.join("c.md"), "c.md", &env, &stopped),
            Err(Unread::Stopped)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
