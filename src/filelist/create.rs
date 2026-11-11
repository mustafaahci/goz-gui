//! `goz-gui -create-file-list <file> <folders>`: Everything's switch that
//! writes a file list of the folders (`;` between several) from the disk,
//! with no window, and returns when it is written. With
//! `-create-file-list-no-subfolders` only each folder's own items go in,
//! `-create-file-list-no-folders` leaves folders out and
//! `-create-file-list-no-files` files. The list is an Everything File List
//! as File > Export writes one (UTF-8, `Filename,Size,Date Modified,Date
//! Created,Attributes`, FILETIMEs, a folder with its trailing `\`). Links
//! to other folders (junctions, symbolic links) are listed, not entered.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// What goes in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub subfolders: bool,
    pub folders: bool,
    pub files: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            subfolders: true,
            folders: true,
            files: true,
        }
    }
}

const DIRECTORY: u32 = 0x10;
const REPARSE_POINT: u32 = 0x400;

fn push_quoted(text: &str, out: &mut String) {
    out.push('"');
    out.push_str(&text.replace('"', "\"\""));
    out.push('"');
}

/// One row of the list for `path`.
fn row(path: &Path, meta: &fs::Metadata, out: &mut String) {
    let attributes = meta.file_attributes();
    let dir = attributes & DIRECTORY != 0;
    let mut text = path.display().to_string();
    if dir && !text.ends_with('\\') {
        text.push('\\');
    }
    push_quoted(&text, out);
    out.push(',');
    if !dir {
        out.push_str(&meta.file_size().to_string());
    }
    out.push(',');
    let time = |t: u64| if t == 0 { String::new() } else { t.to_string() };
    out.push_str(&time(meta.last_write_time()));
    out.push(',');
    out.push_str(&time(meta.creation_time()));
    out.push(',');
    out.push_str(&attributes.to_string());
    out.push_str("\r\n");
}

/// Writes the list of `roots` (`;` between folders) to `file`; how many
/// rows it holds.
pub fn create(file: &Path, roots: &str, o: Options) -> io::Result<u64> {
    let mut w = BufWriter::new(fs::File::create(file)?);
    w.write_all(b"Filename,Size,Date Modified,Date Created,Attributes\r\n")?;
    let mut rows = 0u64;
    let mut line = String::new();
    for root in roots.split(';').map(str::trim).filter(|r| !r.is_empty()) {
        let root = PathBuf::from(root);
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not a folder", root.display()),
            ));
        }
        let mut stack = vec![root];
        while let Some(folder) = stack.pop() {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            let mut subfolders = Vec::new();
            for entry in entries.flatten() {
                // The entry itself, not where a link leads.
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                let attributes = meta.file_attributes();
                let dir = attributes & DIRECTORY != 0;
                if (dir && o.folders) || (!dir && o.files) {
                    line.clear();
                    row(&entry.path(), &meta, &mut line);
                    w.write_all(line.as_bytes())?;
                    rows += 1;
                }
                if dir && o.subfolders && attributes & REPARSE_POINT == 0 {
                    subfolders.push(entry.path());
                }
            }
            // In name order, each folder's contents after it.
            subfolders.sort();
            stack.extend(subfolders.into_iter().rev());
        }
    }
    w.flush()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_folders_as_asked_and_reads_back() {
        let dir = crate::store::file::tests::fresh_dir("goz-gui-create", "");
        let tree = dir.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("a.txt"), "hello").unwrap();
        fs::write(tree.join("sub").join("b.txt"), "x").unwrap();
        let out = dir.join("list.efu");
        let n = create(&out, &tree.display().to_string(), Options::default()).unwrap();
        assert_eq!(n, 3);
        let text = fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("Filename,Size,Date Modified,Date Created,Attributes\r\n"));
        assert!(text.contains(&format!("\"{}\\sub\\\",,", tree.display())));
        assert!(text.contains(&format!("\"{}\\a.txt\",5,", tree.display())));
        let only_files = Options {
            folders: false,
            subfolders: false,
            ..Options::default()
        };
        assert_eq!(
            create(&out, &tree.display().to_string(), only_files).unwrap(),
            1
        );
        // What it writes opens as a list.
        create(&out, &tree.display().to_string(), Options::default()).unwrap();
        let list = super::super::FileList::open(&out, None).unwrap();
        while !list.loaded() {
            std::thread::yield_now();
        }
        assert_eq!(list.rows(), 3);
        assert!(
            create(
                &out,
                &dir.join("missing").display().to_string(),
                Options::default()
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
