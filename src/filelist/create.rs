//! `goz-gui -create-file-list <file> <folders>`: Everything's switch that
//! writes a file list of the folders (`;` between several) from the disk,
//! with no window, and returns when it is written. With
//! `-create-file-list-no-subfolders` only each folder's own items go in,
//! `-create-file-list-no-folders` leaves folders out and
//! `-create-file-list-no-files` files. The list is an Everything File List
//! as File > Export writes one (UTF-8, `Filename,Size,Date Modified,Date
//! Created,Attributes`, FILETIMEs, a folder with its trailing `\`). Links
//! to other folders (junctions, symbolic links) are listed, not entered.
//!
//! Wave 6: Everything's four filters, each a `;` list of wildcard patterns
//! (or `regex:` and a regular expression; see `super::wild`), matched
//! against the name, or the full path for a pattern with a `\`:
//! `-create-file-list-include-only-files` lists only the files that match,
//! `-create-file-list-exclude-files` leaves out those that do;
//! `-create-file-list-include-only-folders` lists only the folders that
//! match (every folder is still looked into, so what it holds is judged on
//! its own), `-create-file-list-exclude-folders` leaves a folder that
//! matches out with everything under it, as Everything's excluded folders.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::wild::NameFilter;

/// What goes in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub subfolders: bool,
    pub folders: bool,
    pub files: bool,
    /// The filters as given (`;` lists; a switch given twice keeps both).
    pub include_only_files: Option<String>,
    pub exclude_files: Option<String>,
    pub include_only_folders: Option<String>,
    pub exclude_folders: Option<String>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            subfolders: true,
            folders: true,
            files: true,
            include_only_files: None,
            exclude_files: None,
            include_only_folders: None,
            exclude_folders: None,
        }
    }
}

/// Adds `filter` to the filter in `slot` (a switch given twice keeps
/// both).
pub fn add_filter(slot: &mut Option<String>, filter: &str) {
    match slot {
        Some(have) if !have.is_empty() => {
            have.push(';');
            have.push_str(filter);
        }
        _ => *slot = Some(filter.to_owned()),
    }
}

/// The filters read: `None` for a switch not given.
struct Filters {
    include_only_files: Option<NameFilter>,
    exclude_files: Option<NameFilter>,
    include_only_folders: Option<NameFilter>,
    exclude_folders: Option<NameFilter>,
}

impl Filters {
    fn of(o: &Options) -> io::Result<Filters> {
        let read = |t: &Option<String>| -> io::Result<Option<NameFilter>> {
            t.as_deref()
                .map(NameFilter::parse)
                .transpose()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
        };
        Ok(Filters {
            include_only_files: read(&o.include_only_files)?,
            exclude_files: read(&o.exclude_files)?,
            include_only_folders: read(&o.include_only_folders)?,
            exclude_folders: read(&o.exclude_folders)?,
        })
    }

    /// Whether the folder at `path` is left out with all it holds.
    fn excludes_folder(&self, path: &str) -> bool {
        self.exclude_folders
            .as_ref()
            .is_some_and(|f| f.matches(path))
    }

    /// Whether the item at `path` is listed (a folder not excluded).
    fn lists(&self, path: &str, dir: bool) -> bool {
        if dir {
            return self
                .include_only_folders
                .as_ref()
                .is_none_or(|f| f.matches(path));
        }
        self.include_only_files
            .as_ref()
            .is_none_or(|f| f.matches(path))
            && !self.exclude_files.as_ref().is_some_and(|f| f.matches(path))
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
pub fn create(file: &Path, roots: &str, o: &Options) -> io::Result<u64> {
    // A filter that does not read stops it before the file is made.
    let filters = Filters::of(o)?;
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
                let path = entry.path();
                let text = path.display().to_string();
                if dir && filters.excludes_folder(&text) {
                    continue;
                }
                if ((dir && o.folders) || (!dir && o.files)) && filters.lists(&text, dir) {
                    line.clear();
                    row(&path, &meta, &mut line);
                    w.write_all(line.as_bytes())?;
                    rows += 1;
                }
                if dir && o.subfolders && attributes & REPARSE_POINT == 0 {
                    subfolders.push(path);
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
        let dir = std::env::temp_dir().join(format!("goz-gui-create-{}", std::process::id()));
        let tree = dir.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("a.txt"), "hello").unwrap();
        fs::write(tree.join("sub").join("b.txt"), "x").unwrap();
        let out = dir.join("list.efu");
        let n = create(&out, &tree.display().to_string(), &Options::default()).unwrap();
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
            create(&out, &tree.display().to_string(), &only_files).unwrap(),
            1
        );
        // What it writes opens as a list.
        create(&out, &tree.display().to_string(), &Options::default()).unwrap();
        let list = super::super::FileList::open(&out, None).unwrap();
        while !list.loaded() {
            std::thread::yield_now();
        }
        assert_eq!(list.rows(), 3);
        assert!(
            create(
                &out,
                &dir.join("missing").display().to_string(),
                &Options::default()
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// W6: Everything's include and exclude filters.
    #[test]
    fn include_and_exclude_filters() {
        let dir = std::env::temp_dir().join(format!("goz-gui-create-f-{}", std::process::id()));
        let tree = dir.join("tree");
        let _ = fs::remove_dir_all(&dir);
        for d in [r"Music\live", r"cache\deep", "docs"] {
            fs::create_dir_all(tree.join(d)).unwrap();
        }
        for f in [
            "a.mp3",
            "b.FLAC",
            "c.txt",
            r"Music\d.mp3",
            r"Music\live\e.mp3",
            r"cache\f.mp3",
            r"cache\deep\g.mp3",
            r"docs\h.txt",
        ] {
            fs::write(tree.join(f), "x").unwrap();
        }
        let out = dir.join("list.efu");
        let root = tree.display().to_string();
        let listed = |o: Options| -> Vec<String> {
            create(&out, &root, &o).unwrap();
            let text = fs::read_to_string(&out).unwrap();
            let mut names: Vec<String> = text
                .lines()
                .skip(1)
                .map(|l| {
                    let path = l.split('"').nth(1).unwrap_or_default();
                    path.strip_prefix(&root).unwrap_or(path).to_owned()
                })
                .collect();
            names.sort();
            names
        };
        let mut o = Options::default();
        add_filter(&mut o.include_only_files, "*.mp3");
        add_filter(&mut o.include_only_files, "*.flac");
        add_filter(&mut o.exclude_folders, "cache");
        assert_eq!(o.include_only_files.as_deref(), Some("*.mp3;*.flac"));
        assert_eq!(
            listed(o),
            [
                r"\Music\",
                r"\Music\d.mp3",
                r"\Music\live\",
                r"\Music\live\e.mp3",
                r"\a.mp3",
                r"\b.FLAC",
                r"\docs\"
            ]
        );
        // Only the folders that match are listed; the others are still
        // looked into. Files left out by a pattern on the full path.
        let o = Options {
            include_only_folders: Some("mus*;deep".into()),
            exclude_files: Some(r"*.txt;*\cache\*".into()),
            ..Options::default()
        };
        assert_eq!(
            listed(o),
            [
                r"\Music\",
                r"\Music\d.mp3",
                r"\Music\live\e.mp3",
                r"\a.mp3",
                r"\b.FLAC",
                r"\cache\deep\",
            ]
        );
        let o = Options {
            include_only_files: Some(r"regex:^[ab]\.".into()),
            folders: false,
            ..Options::default()
        };
        assert_eq!(listed(o), [r"\a.mp3", r"\b.FLAC"]);
        // A regular expression that does not read: no file is made.
        let _ = fs::remove_file(&out);
        let bad = Options {
            exclude_files: Some("regex:(".into()),
            ..Options::default()
        };
        assert!(create(&out, &root, &bad).is_err());
        assert!(!out.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
