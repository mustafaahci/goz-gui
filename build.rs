//! Embeds the application manifest (Common Controls v6, per-monitor DPI v2,
//! UTF-8 code page), the app icon (if assets/goz.ico exists) and version
//! information through the linker, so no resource compiler is needed.
//!
//! The icon and version go in a `.res` file written here byte by byte: the
//! format is a run of entries, each a header naming the resource's type, id
//! and language followed by its data, and link.exe takes it as an input.

use std::path::Path;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
/// U.S. English, the language the version strings are tagged with.
const LANG: u16 = 0x0409;
/// Unicode, the code page of the version strings.
const CODE_PAGE: u16 = 1200;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=goz-gui.manifest");
    println!("cargo:rerun-if-changed=assets/goz.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let dir = Path::new(&dir);
    let manifest = dir.join("goz-gui.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );

    let mut res = Vec::new();
    // A .res file starts with an empty entry that marks it as 32-bit.
    entry(&mut res, 0, 0, 0, 0, &[]);
    // The app icon is optional: without assets/goz.ico the windows get
    // Windows' default application icon.
    match std::fs::read(dir.join("assets").join("goz.ico")) {
        Ok(ico) => icon(&mut res, &ico),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("read assets/goz.ico: {e}"),
    }
    let version = version_info();
    entry(&mut res, RT_VERSION, 1, LANG, 0x0030, &version);

    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let path = Path::new(&out).join("goz-gui.res");
    std::fs::write(&path, res).expect("write goz-gui.res");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// One resource: a header naming it by numeric type and id, then its data.
fn entry(out: &mut Vec<u8>, kind: u16, id: u16, lang: u16, flags: u16, data: &[u8]) {
    // DataSize, HeaderSize, TYPE and NAME as ordinals (0xFFFF + id),
    // DataVersion, MemoryFlags, LanguageId, Version, Characteristics.
    u32le(out, data.len() as u32);
    u32le(out, 32);
    u16le(out, 0xffff);
    u16le(out, kind);
    u16le(out, 0xffff);
    u16le(out, id);
    u32le(out, 0);
    u16le(out, flags);
    u16le(out, lang);
    u32le(out, 0);
    u32le(out, 0);
    out.extend_from_slice(data);
    pad4(out);
}

/// Each image of an .ico as its own RT_ICON (ids 1..), and the directory
/// that groups them as RT_GROUP_ICON 1, which is what LoadImage(1) finds.
fn icon(out: &mut Vec<u8>, ico: &[u8]) {
    let word = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let dword = |at: usize| u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]);
    assert!(
        word(0) == 0 && word(2) == 1,
        "assets/goz.ico is not an icon"
    );
    let count = word(4);
    let mut group = Vec::new();
    u16le(&mut group, 0);
    u16le(&mut group, 1);
    u16le(&mut group, count);
    for i in 0..count {
        let e = 6 + 16 * usize::from(i);
        let size = dword(e + 8) as usize;
        let offset = dword(e + 12) as usize;
        let id = i + 1;
        entry(out, RT_ICON, id, LANG, 0x1010, &ico[offset..offset + size]);
        // The .ico entry minus its file offset, plus the resource id.
        group.extend_from_slice(&ico[e..e + 12]);
        u16le(&mut group, id);
    }
    entry(out, RT_GROUP_ICON, 1, LANG, 0x1030, &group);
}

/// A VS_VERSIONINFO node: its length, value length and type, a key, then
/// the value and the children, each aligned to 32 bits.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    u16le(&mut out, value_len);
    u16le(&mut out, u16::from(text));
    for unit in key.encode_utf16().chain(Some(0)) {
        u16le(&mut out, unit);
    }
    pad4(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad4(&mut out);
        out.extend_from_slice(child);
    }
    let len = out.len() as u16;
    out[..2].copy_from_slice(&len.to_le_bytes());
    out
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let mut data = Vec::new();
    let units: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    for &unit in &units {
        u16le(&mut data, unit);
    }
    node(key, &data, units.len() as u16, true, &[])
}

/// File and product version, description and copyright, which Explorer's
/// Properties and Task Manager show.
fn version_info() -> Vec<u8> {
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let part = |name: &str| -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    let mut fixed = Vec::new();
    u32le(&mut fixed, 0xfeef_04bd); // signature
    u32le(&mut fixed, 0x0001_0000); // structure version
    for _ in 0..2 {
        // File version, then product version.
        u32le(&mut fixed, (major << 16) | minor);
        u32le(&mut fixed, patch << 16);
    }
    u32le(&mut fixed, 0x3f); // flags mask
    u32le(&mut fixed, 0); // flags
    u32le(&mut fixed, 0x0004_0004); // VOS_NT_WINDOWS32
    u32le(&mut fixed, 1); // VFT_APP
    u32le(&mut fixed, 0); // subtype
    u32le(&mut fixed, 0); // date, high
    u32le(&mut fixed, 0); // date, low

    let authors = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
    let table = node(
        &format!("{LANG:04x}{CODE_PAGE:04x}"),
        &[],
        0,
        true,
        &[
            string("CompanyName", &authors),
            string("FileDescription", "goz-gui"),
            string("FileVersion", &version),
            string("InternalName", "goz-gui"),
            string(
                "LegalCopyright",
                &format!("Copyright (c) {authors}. MIT License."),
            ),
            string("OriginalFilename", "goz-gui.exe"),
            string("ProductName", "goz-gui"),
            string("ProductVersion", &version),
        ],
    );
    let strings = node("StringFileInfo", &[], 0, true, &[table]);
    let mut translation = Vec::new();
    u16le(&mut translation, LANG);
    u16le(&mut translation, CODE_PAGE);
    let var = node("Translation", &translation, 4, false, &[]);
    let vars = node("VarFileInfo", &[], 0, true, &[var]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[strings, vars],
    )
}
