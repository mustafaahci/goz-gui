//! 32-bit DIB sections: a canvas of pixels drawn with GDI (a section in a
//! memory DC of its own), a section of pixels owned on its own, and the
//! pixels of any bitmap. Pixels go in and out by copy only (GetDIBits,
//! SetDIBits), never through a pointer into a section: GDI finds the
//! bitmap by its handle each time, so no Rust reference into memory the
//! system lends outlives the call that copied it, and a bitmap deleted
//! meanwhile (by any thread) is a failure, not freed memory read.

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, GetDIBits, HBITMAP, HDC, HGDIOBJ, SelectObject, SetDIBits,
};

use super::gdi::WindowDc;

/// A 32-bit top-down DIB section in a memory DC of its own, to draw on
/// with GDI ([`Canvas::hdc`]) and read back. Its pixels go in and out
/// by copy, through GDI (GetDIBits, SetDIBits), never through a pointer
/// into the section: GDI checks the bitmap each time, whatever was done
/// with the DC meanwhile. Pixels are BGRA, rows top-down.
pub struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    /// The bitmap the DC had, put back to read or write the pixels.
    old: HGDIOBJ,
    width: i32,
    height: i32,
}

impl Canvas {
    /// A `width` × `height` canvas, every pixel zero (clear black);
    /// `None` for a size of no pixels, or if it could not be made.
    pub fn new(width: i32, height: i32) -> Option<Canvas> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let bitmap = section(width, height)?;
        // SAFETY: no pointers.
        let dc = unsafe { CreateCompatibleDC(core::ptr::null_mut()) };
        if dc.is_null() {
            // SAFETY: no pointers; the bitmap made above, in no DC.
            unsafe { DeleteObject(bitmap) };
            return None;
        }
        // SAFETY: no pointers.
        let old = unsafe { SelectObject(dc, bitmap) };
        Some(Canvas {
            dc,
            bitmap,
            old,
            width,
            height,
        })
    }

    /// The DC to draw on (with the canvas selected into it).
    pub fn hdc(&self) -> HDC {
        self.dc
    }

    /// How many pixels it has (its size is positive, checked by `new`).
    fn count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Runs `f` with the canvas out of its DC, as GetDIBits and
    /// SetDIBits want it, after what was drawn has been done (GdiFlush).
    fn out_of_dc<R>(&self, f: impl FnOnce() -> R) -> R {
        // SAFETY: no pointers.
        unsafe {
            GdiFlush();
            SelectObject(self.dc, self.old);
        }
        let r = f();
        // SAFETY: no pointers.
        unsafe { SelectObject(self.dc, self.bitmap) };
        r
    }

    /// A copy of the pixels; `None` if they could not be read.
    pub fn pixels(&self) -> Option<Vec<[u8; 4]>> {
        let mut out = vec![[0u8; 4]; self.count()];
        let mut bmi = info(self.width, self.height);
        let lines = self.out_of_dc(|| {
            // SAFETY: the header gives the size and format of the DIB
            // written (whatever the bitmap is): `height` rows of
            // `width` 32-bit pixels (rows of 32-bit pixels need no
            // padding), which the buffer holds; the header is in-out
            // (BI_RGB at 32 bits: no colour table or masks follow it).
            unsafe {
                GetDIBits(
                    self.dc,
                    self.bitmap,
                    0,
                    self.height as u32,
                    out.as_mut_ptr().cast(),
                    &mut bmi,
                    DIB_RGB_COLORS,
                )
            }
        });
        (lines == self.height).then_some(out)
    }

    /// Sets the pixels from `pixels` (`width` × `height` of them); false
    /// if they are not that many or could not be written.
    pub fn set_pixels(&self, pixels: &[[u8; 4]]) -> bool {
        if pixels.len() != self.count() {
            return false;
        }
        let bmi = info(self.width, self.height);
        let lines = self.out_of_dc(|| {
            // SAFETY: `height` rows of `width` 32-bit pixels, read
            // during the call, as the header says (and as many as the
            // slice holds, checked above).
            unsafe {
                SetDIBits(
                    self.dc,
                    self.bitmap,
                    0,
                    self.height as u32,
                    pixels.as_ptr().cast(),
                    &bmi,
                    DIB_RGB_COLORS,
                )
            }
        });
        lines == self.height
    }

    /// Sets every pixel to `pixel`.
    pub fn fill(&self, pixel: [u8; 4]) -> bool {
        self.set_pixels(&vec![pixel; self.count()])
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: no pointers; the DC and bitmap this made, the bitmap
        // taken out of the DC before both are deleted.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteDC(self.dc);
            DeleteObject(self.bitmap);
        }
    }
}

/// A new 32-bit top-down DIB section of `width` × `height`, all zero;
/// `None` for a size of no pixels, or if it could not be made. The caller
/// deletes it.
fn section(width: i32, height: i32) -> Option<HBITMAP> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let bmi = info(width, height);
    let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: a complete header; the pointer to the pixels is written to a
    // local and never used (they are reached through GDI only).
    let bitmap = unsafe {
        CreateDIBSection(
            core::ptr::null_mut(),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            core::ptr::null_mut(),
            0,
        )
    };
    if bitmap.is_null() {
        return None;
    }
    if bits.is_null() {
        // SAFETY: no pointers; the bitmap made above, in no DC.
        unsafe { DeleteObject(bitmap) };
        return None;
    }
    Some(bitmap)
}

/// A 32-bit top-down DIB section on its own (in no DC), deleted when this
/// drops: a bitmap for GDI to draw from (`AlphaBlend`, with premultiplied
/// pixels). Its pixels go in and out by copy, through GDI
/// ([`Section::set_pixels`], [`Section::pixels`]); while it is selected
/// into a DC, GDI refuses both. A GDI bitmap belongs to no thread, so
/// neither does this: it is `Send`.
pub struct Section {
    bitmap: HBITMAP,
    width: i32,
    height: i32,
}

// SAFETY: a GDI bitmap is a process-wide object that any thread may use
// and delete; its handle is all this holds.
unsafe impl Send for Section {}

impl Section {
    /// A `width` × `height` section, every pixel zero (clear black);
    /// `None` for a size of no pixels, or if it could not be made.
    pub fn new(width: i32, height: i32) -> Option<Section> {
        section(width, height).map(|bitmap| Section {
            bitmap,
            width,
            height,
        })
    }

    /// Its handle, for GDI to draw from (it stays this section's).
    pub fn handle(&self) -> HBITMAP {
        self.bitmap
    }

    /// How many pixels it has (its size is positive, checked by `new`).
    fn count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// A copy of its pixels (BGRA, rows top-down); `None` if they could not
    /// be read.
    pub fn pixels(&self) -> Option<Vec<[u8; 4]>> {
        bitmap_pixels(self.bitmap, self.width, self.height)
    }

    /// Sets its pixels from `pixels` (BGRA, rows top-down, `width` ×
    /// `height` of them); false if they are not that many or could not be
    /// written.
    pub fn set_pixels(&self, pixels: &[[u8; 4]]) -> bool {
        if pixels.len() != self.count() {
            return false;
        }
        let bmi = info(self.width, self.height);
        // What GDI has still to draw lands before, not after, the copy.
        // SAFETY: no pointers.
        unsafe { GdiFlush() };
        let screen = WindowDc::get(core::ptr::null_mut());
        // SAFETY: `height` rows of `width` 32-bit pixels, read during the
        // call, as the header says (and as many as the slice holds, checked
        // above); the bitmap is found by its handle, which GDI checks.
        let lines = unsafe {
            SetDIBits(
                screen.hdc(),
                self.bitmap,
                0,
                self.height as u32,
                pixels.as_ptr().cast(),
                &bmi,
                DIB_RGB_COLORS,
            )
        };
        lines == self.height
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        // SAFETY: no pointers; the section this owns, deleted once.
        unsafe { DeleteObject(self.bitmap) };
    }
}

/// The pixels of bitmap `src` (of any kind and depth) as `w` × `h` 32-bit
/// top-down BGRA, through the screen's DC (GetDIBits); `None` if GDI could
/// not give them.
pub fn bitmap_pixels(src: HBITMAP, w: i32, h: i32) -> Option<Vec<[u8; 4]>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let count = (w as usize).checked_mul(h as usize)?;
    let mut px = vec![[0u8; 4]; count];
    let mut bmi = info(w, h);
    // What GDI has still to draw is in the bits before they are read.
    // SAFETY: no pointers.
    unsafe { GdiFlush() };
    let screen = WindowDc::get(core::ptr::null_mut());
    // SAFETY: a buffer of `w * h` 32-bit pixels: what the header asks for
    // (`h` rows of `w * 4` bytes; BI_RGB at 32 bits: no colour table or
    // masks follow the header, which is in-out); the bitmap is found by its
    // handle, which GDI checks.
    let lines = unsafe {
        GetDIBits(
            screen.hdc(),
            src,
            0,
            h as u32,
            px.as_mut_ptr().cast(),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    (lines != 0).then_some(px)
}

/// The header of a 32-bit top-down DIB of `width` × `height`.
fn info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Top-down.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_canvas_starts_clear_and_keeps_what_it_is_given() {
        let canvas = Canvas::new(3, 2).unwrap();
        assert_eq!(canvas.pixels().unwrap(), vec![[0u8; 4]; 6]);
        let px: Vec<[u8; 4]> = (0..6u8).map(|i| [i, i + 1, i + 2, 255]).collect();
        assert!(canvas.set_pixels(&px));
        assert_eq!(canvas.pixels().unwrap(), px);
        assert!(!canvas.set_pixels(&px[..5]), "too few pixels");
        assert!(canvas.fill([255, 255, 255, 0]));
        assert_eq!(canvas.pixels().unwrap(), vec![[255, 255, 255, 0]; 6]);
        assert!(Canvas::new(0, 4).is_none());
    }

    #[test]
    fn pixels_go_into_a_section_and_come_back() {
        let section = Section::new(3, 2).unwrap();
        let px: Vec<[u8; 4]> = (0..6).map(|i| [i, 2 * i, 3 * i, 255]).collect();
        assert!(!section.set_pixels(&px[..5]), "the wrong count is refused");
        assert_eq!(section.pixels(), Some(vec![[0; 4]; 6]));
        assert!(section.set_pixels(&px));
        assert_eq!(section.pixels(), Some(px.clone()));
        // Read through GDI as from any bitmap.
        assert_eq!(bitmap_pixels(section.handle(), 3, 2), Some(px));
        assert!(Section::new(0, 5).is_none());
        assert!(bitmap_pixels(section.handle(), 0, 2).is_none());
    }
}
