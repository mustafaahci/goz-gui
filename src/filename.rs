//! What Windows refuses as a file's name, in Explorer's words: for Rename in
//! place (`window/files.rs`) and the Advanced Rename preview
//! (`crate::filetools::rename`).

pub const EMPTY: &str = "You must type a file name.";
pub const BAD_CHARS: &str =
    "A file name can't contain any of the following characters:\n\\ / : * ? \" < > |";
pub const TRAILING: &str = "A file name can't end with a space or a period.";
pub const RESERVED: &str = "The specified device name is invalid.";
/// Everything's words for a name longer than a volume takes.
pub const TOO_LONG: &str = "A file name can't contain more than 255 characters.";

/// The longest name NTFS takes, in UTF-16 code units.
pub const MAX_NAME: usize = 255;

/// Why `name`, typed for a rename, cannot be a file's name; `None` if it
/// can. Empty, a character Windows refuses (or a control character), a
/// trailing space or dot (Windows would drop it), a device name (`CON`,
/// `nul.txt`, `COM1`…), or longer than 255 characters.
pub fn problem(name: &[u16]) -> Option<&'static str> {
    let text = String::from_utf16_lossy(name);
    if text.trim().is_empty() {
        return Some(EMPTY);
    }
    if text
        .chars()
        .any(|c| "\\/:*?\"<>|".contains(c) || u32::from(c) < 0x20)
    {
        return Some(BAD_CHARS);
    }
    if text.ends_with([' ', '.']) {
        return Some(TRAILING);
    }
    if name.len() > MAX_NAME {
        return Some(TOO_LONG);
    }
    let base = text.split('.').next().unwrap_or("").trim_end();
    let upper = base.to_ascii_uppercase();
    let device = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'));
    device.then_some(RESERVED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn names_windows_refuses() {
        let p = |s: &str| problem(&w(s));
        for good in [
            "a.txt",
            "report 2024.pdf",
            " lead",
            "CONSOLE.txt",
            "com10",
            "x.con",
        ] {
            assert_eq!(p(good), None, "{good}");
        }
        assert_eq!(p(""), Some(EMPTY));
        assert_eq!(p("   "), Some(EMPTY));
        for bad in [
            "a/b", "a\\b", "a:b", "a*", "a?", "\"a\"", "<a>", "a|b", "a\tb",
        ] {
            assert_eq!(p(bad), Some(BAD_CHARS), "{bad}");
        }
        assert_eq!(p("name."), Some(TRAILING));
        assert_eq!(p("name "), Some(TRAILING));
        for device in [
            "CON",
            "nul",
            "nul.txt",
            "Aux.tar.gz",
            "COM1",
            "lpt9.log",
            "con .txt",
        ] {
            assert_eq!(p(device), Some(RESERVED), "{device}");
        }
        assert_eq!(p(&"x".repeat(255)), None);
        assert_eq!(p(&"x".repeat(256)), Some(TOO_LONG));
    }
}
