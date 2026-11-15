//! Dialogs made in memory: a DLGTEMPLATE built control by control
//! ([`Template`]), run modal or made as a child page, its messages handed
//! to a closure; and the dialog functions on their items.

use std::cell::Cell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, CreateDialogIndirectParamW, DM_SETDEFID, DWLP_MSGRESULT,
    DialogBoxIndirectParamW, EndDialog, GWLP_USERDATA, GetDlgItem, GetWindowLongPtrW,
    MapDialogRect, SetWindowLongPtrW, WM_INITDIALOG, WM_NCDESTROY, WM_NEXTDLGCTL,
};

use windows_sys::Win32::UI::Controls::{CheckDlgButton, DLG_BUTTON_CHECK_STATE};

use super::msg::{Msg, Via};

/// A control's window class in a [`Template`].
pub enum Class {
    Button,
    Edit,
    Static,
    /// A class by name, such as a common control's.
    Named(&'static str),
}

/// A dialog's template, built control by control. Always complete: the
/// header and each control as DLGTEMPLATE and DLGITEMTEMPLATE lay them out,
/// so the dialog functions can be given it.
pub struct Template {
    /// The template as 16-bit words, the unit its fields come in.
    words: Vec<u16>,
}

/// Where the control count sits in the template: after the style and the
/// extended style.
const COUNT_AT: usize = 4;

impl Template {
    /// A dialog of `cx` × `cy` dialog units, styles `style` (which should
    /// include DS_SETFONT, as the font follows) and `ex_style`, titled
    /// `title`, in the font `face` at `points`.
    pub fn with_styles(
        title: &str,
        style: u32,
        ex_style: u32,
        (cx, cy): (i16, i16),
        points: u16,
        face: &[u16],
    ) -> Template {
        let mut t = Template { words: Vec::new() };
        t.dword(style);
        t.dword(ex_style);
        // The control count, then the position and size.
        for v in [0, 0, 0, cx, cy] {
            t.words.push(v as u16);
        }
        // No menu, the standard dialog class.
        t.words.extend([0, 0]);
        t.string(title);
        if style & windows_sys::Win32::UI::WindowsAndMessaging::DS_SETFONT as u32 != 0 {
            t.words.push(points);
            t.words.extend(super::until_nul(face));
            t.words.push(0);
        }
        t
    }

    fn dword(&mut self, v: u32) {
        self.words.extend([v as u16, (v >> 16) as u16]);
    }

    fn string(&mut self, s: &str) {
        self.words.extend(s.encode_utf16().filter(|&c| c != 0));
        self.words.push(0);
    }

    /// Adds a control at `x, y` of `cx` × `cy` dialog units, visible
    /// (WS_CHILD | WS_VISIBLE and `style`). Controls come in tab order.
    pub fn control(
        &mut self,
        class: Class,
        text: &str,
        id: u16,
        style: u32,
        ex_style: u32,
        (x, y, cx, cy): (i16, i16, i16, i16),
    ) -> &mut Template {
        use windows_sys::Win32::UI::WindowsAndMessaging::{WS_CHILD, WS_VISIBLE};
        // Each control starts on a 4-byte boundary.
        if self.words.len() % 2 == 1 {
            self.words.push(0);
        }
        self.dword(WS_CHILD | WS_VISIBLE | style);
        self.dword(ex_style);
        for v in [x, y, cx, cy] {
            self.words.push(v as u16);
        }
        self.words.push(id);
        match class {
            // The predefined classes go by their atoms.
            Class::Button => self.words.extend([0xffff, 0x0080]),
            Class::Edit => self.words.extend([0xffff, 0x0081]),
            Class::Static => self.words.extend([0xffff, 0x0082]),
            Class::Named(name) => self.string(name),
        }
        self.string(text);
        // No creation data.
        self.words.push(0);
        self.words[COUNT_AT] += 1;
        self
    }

    /// The template as the dialog functions take it: on a 4-byte boundary,
    /// which u32s are.
    pub(crate) fn aligned(&self) -> Vec<u32> {
        let mut aligned = vec![0u32; self.words.len().div_ceil(2)];
        for (i, &w) in self.words.iter().enumerate() {
            aligned[i / 2] |= u32::from(w) << (16 * (i % 2));
        }
        aligned
    }

    /// Shows the dialog, owned by `owner`, until it ends, its messages going
    /// to `handler` (from WM_INITDIALOG on); returns what [`end`] was given
    /// (-1 if it could not be made). `handler` answers as a dialog procedure
    /// does: nonzero for a message it handled.
    pub fn modal(&self, owner: HWND, handler: &dyn Fn(&Msg) -> isize) -> isize {
        let aligned = self.aligned();
        let slot: &dyn Fn(&Msg) -> isize = handler;
        // SAFETY: a complete template that outlives the modal call; `slot`
        // outlives it too, and the dialog with it (DialogBoxIndirectParamW
        // destroys the dialog before it returns), so `modal_proc` may read
        // it for every message.
        unsafe {
            DialogBoxIndirectParamW(
                super::class::module(),
                aligned.as_ptr().cast(),
                owner,
                Some(modal_proc),
                (&raw const slot) as LPARAM,
            )
        }
    }

    /// Makes the dialog as a child of `parent` (hidden unless its style has
    /// WS_VISIBLE), its messages going to `handler` until it is destroyed;
    /// null if it could not be made (and then `handler` is dropped).
    pub fn create_child(&self, parent: HWND, handler: impl Fn(&Msg) -> isize + 'static) -> HWND {
        let aligned = self.aligned();
        let pending = Pending {
            handler: Cell::new(Some(Rc::new(handler))),
        };
        // SAFETY: a complete template (the system copies what it needs
        // before returning); `pending` outlives the call, and only
        // `child_proc` reads it, at WM_INITDIALOG, during the call.
        unsafe {
            CreateDialogIndirectParamW(
                super::class::module(),
                aligned.as_ptr().cast(),
                parent,
                Some(child_proc),
                (&raw const pending) as LPARAM,
            )
        }
        // A handler the dialog did not take (it was not made) drops with
        // `pending`.
    }
}

type ChildHandler = Rc<dyn Fn(&Msg) -> isize>;

/// Waits at WM_INITDIALOG for `child_proc` to take the handler.
struct Pending {
    handler: Cell<Option<ChildHandler>>,
}

/// The procedure of a modal dialog: GWLP_USERDATA is the `&dyn Fn` slot
/// `modal` keeps on its stack, from WM_INITDIALOG on.
unsafe extern "system" fn modal_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: WM_INITDIALOG's lParam is what `modal` passed: its slot.
        // A dialog's GWLP_USERDATA is the dialog procedure's to use, and
        // nothing else in goz-gui writes it.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: null, or `modal`'s slot (above), which outlives the dialog.
    let slot = unsafe { GetWindowLongPtrW(dlg, GWLP_USERDATA) } as *const &dyn Fn(&Msg) -> isize;
    // SAFETY: as above.
    let Some(&handler) = (unsafe { slot.as_ref() }) else {
        return 0;
    };
    // SAFETY: the parameters the system delivered, for this call.
    handler(&unsafe { Msg::delivered(dlg, msg, w, l, Via::Dialog) })
}

/// The procedure of a child dialog: GWLP_USERDATA is a `Box<ChildHandler>`
/// from WM_INITDIALOG until WM_NCDESTROY frees it.
unsafe extern "system" fn child_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: WM_INITDIALOG's lParam is the `Pending` `create_child`
        // passed, alive during the call that sends this message.
        let pending = unsafe { (l as *const Pending).as_ref() };
        if let Some(handler) = pending.and_then(|p| p.handler.take()) {
            let slot = Box::into_raw(Box::new(handler));
            // SAFETY: a dialog's GWLP_USERDATA is its procedure's to use,
            // and nothing else in goz-gui writes it.
            unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, slot as isize) };
        }
    }
    // SAFETY: null, or the box set above, not yet freed.
    let slot = unsafe { GetWindowLongPtrW(dlg, GWLP_USERDATA) } as *const ChildHandler;
    // SAFETY: as above; cloned, so the handler lives through a call that
    // destroys the dialog.
    let Some(handler) = (unsafe { slot.as_ref() }).map(Rc::clone) else {
        return 0;
    };
    // SAFETY: the parameters the system delivered, for this call.
    let answer = handler(&unsafe { Msg::delivered(dlg, msg, w, l, Via::Dialog) });
    if msg == WM_NCDESTROY {
        // SAFETY: the box set at WM_INITDIALOG, taken off the dialog first.
        unsafe {
            let slot = SetWindowLongPtrW(dlg, GWLP_USERDATA, 0) as *mut ChildHandler;
            if !slot.is_null() {
                drop(Box::from_raw(slot));
            }
        }
        super::parcel::discard_for(dlg);
    }
    answer
}

/// The dialog's control `id` (null if there is none).
pub fn item(dlg: HWND, id: u16) -> HWND {
    // SAFETY: no pointers.
    unsafe { GetDlgItem(dlg, i32::from(id)) }
}

/// Ends the modal dialog: [`Template::modal`] returns `code`.
pub fn end(dlg: HWND, code: isize) {
    // SAFETY: no pointers.
    unsafe { EndDialog(dlg, code) };
}

/// The answer of the dialog's current message, for the ones whose answer
/// goes in DWLP_MSGRESULT (WM_NOTIFY's): the handler then returns 1.
pub fn set_result(dlg: HWND, answer: isize) {
    // SAFETY: an integer; DWLP_MSGRESULT is not a pointer.
    unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, answer) };
}

/// `rc`, in the dialog's units, in its pixels.
pub fn map_rect(dlg: HWND, rc: RECT) -> RECT {
    let mut r = rc;
    // SAFETY: an in-out struct.
    unsafe { MapDialogRect(dlg, &mut r) };
    r
}

/// The dialog's default push button becomes `id` (DM_SETDEFID).
pub fn set_def_id(dlg: HWND, id: u16) {
    let dlg = super::of_class(dlg, "#32770");
    // SAFETY: DM_SETDEFID's parameter is an id; a dialog's class checked.
    unsafe { super::send_raw(dlg, DM_SETDEFID, usize::from(id), 0) };
}

/// Moves the keyboard to control `to` of the dialog, as the dialog manager
/// does it (WM_NEXTDLGCTL), so default buttons follow.
pub fn focus_ctl(dlg: HWND, to: HWND) {
    // SAFETY: WM_NEXTDLGCTL's parameters: a handle and a flag.
    unsafe { super::send_raw(dlg, WM_NEXTDLGCTL, to as WPARAM, 1) };
}

/// Moves the keyboard to the dialog's next (or previous) tab stop.
pub fn next_ctl(dlg: HWND, previous: bool) {
    // SAFETY: WM_NEXTDLGCTL's parameters: a flag and a flag.
    unsafe { super::send_raw(dlg, WM_NEXTDLGCTL, usize::from(previous), 0) };
}

/// Checks or clears check box or radio button `id` (BST_*).
pub fn check(dlg: HWND, id: u16, state: DLG_BUTTON_CHECK_STATE) {
    // SAFETY: no pointers.
    unsafe { CheckDlgButton(dlg, i32::from(id), state) };
}

/// Whether check box or radio button `id` is checked (BST_CHECKED).
pub fn is_checked(dlg: HWND, id: u16) -> bool {
    // SAFETY: BM_GETCHECK has no parameters.
    unsafe { super::send_raw(item(dlg, id), BM_GETCHECK, 0, 0) == 1 }
}

/// Sets check box or radio button `id` to checked or not.
pub fn set_checked(dlg: HWND, id: u16, on: bool) {
    // SAFETY: BM_SETCHECK's parameter is a state.
    unsafe { super::send_raw(item(dlg, id), BM_SETCHECK, usize::from(on), 0) };
}

/// The text of control `id`.
pub fn item_text(dlg: HWND, id: u16) -> String {
    super::text(item(dlg, id))
}

/// Sets the text of control `id`.
pub fn set_item_text(dlg: HWND, id: u16, text: &str) {
    super::set_text(item(dlg, id), text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DS_SETFONT, WS_POPUP};

    #[test]
    fn template_layout() {
        let face: Vec<u16> = "Segoe UI".encode_utf16().collect();
        let style = WS_POPUP | DS_SETFONT as u32;
        let mut t = Template::with_styles("T", style, 0, (100, 50), 9, &face);
        let header = t.words.len();
        t.control(Class::Edit, "ab", 7, 0, 0, (1, 2, 3, 4));
        t.control(Class::Named("X"), "", 8, 0, 0, (0, 0, 1, 1));
        assert_eq!(t.words[COUNT_AT], 2);
        // Size, then the title "T", after the menu and class words.
        assert_eq!(&t.words[7..12], &[100, 50, 0, 0, u16::from(b'T')]);
        // The first control starts 4-byte aligned, its id after its styles
        // and rectangle, then the Edit atom and the text.
        let at = header + header % 2;
        assert_eq!(t.words[at + 8], 7);
        assert_eq!(&t.words[at + 9..at + 11], &[0xffff, 0x0081]);
        assert_eq!(
            &t.words[at + 11..at + 14],
            &[u16::from(b'a'), u16::from(b'b'), 0]
        );
        let second = at + 15 + (at + 15) % 2;
        assert_eq!(t.words[second + 8], 8);
        assert_eq!(&t.words[second + 9..second + 11], &[u16::from(b'X'), 0]);
        assert_eq!(second % 2, 0);
        // As u32s for the dialog functions, every word in.
        assert_eq!(t.aligned().len(), t.words.len().div_ceil(2));
    }

    #[test]
    fn a_nul_in_a_title_does_not_end_the_template_early() {
        let t = Template::with_styles("a\0b", WS_POPUP, 0, (1, 1), 9, &[]);
        let title_at = 2 + 2 + 5 + 2;
        assert_eq!(&t.words[title_at..title_at + 3], &[97, 98, 0]);
    }
}
