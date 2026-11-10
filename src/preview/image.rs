//! Images in the preview pane, goz-gui's own renderer (as Everything has
//! one): WIC decodes the file on the preview's thread into a box of
//! `preview_load_size` pixels (or the pane's size, if bigger), turned
//! upright as its EXIF orientation says, and the pane draws it scaled to fit
//! with its aspect kept. The arithmetic is here, tested; the decoding is
//! `Wic`.

use std::collections::HashSet;

use windows::Win32::Foundation::{E_FAIL, E_OUTOFMEMORY, GENERIC_READ};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatIco, GUID_ContainerFormatJpeg,
    GUID_ContainerFormatTiff, GUID_WICPixelFormat32bppPBGRA, IWICBitmapCodecInfo,
    IWICBitmapDecoder, IWICBitmapFrameDecode, IWICBitmapSource, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICComponentEnumerateDefault, WICDecodeMetadataCacheOnDemand, WICDecoder,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToUInt16,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{GUID, Interface, PCWSTR};

use crate::thumbs::Bitmap;
use crate::wide::{nul, wz};

/// `size` drawn in `bounds`: as it is when it fits (or bigger, filling the
/// bounds, with `enlarge`), else shrunk to fit; the aspect is kept and no
/// side is under 1 pixel.
pub fn fit(size: (i32, i32), bounds: (i32, i32), enlarge: bool) -> (i32, i32) {
    let (w, h) = (i64::from(size.0.max(1)), i64::from(size.1.max(1)));
    let (bw, bh) = (i64::from(bounds.0.max(1)), i64::from(bounds.1.max(1)));
    if !enlarge && w <= bw && h <= bh {
        return (w as i32, h as i32);
    }
    // The side that meets its bound first sets the scale.
    if w * bh >= h * bw {
        (bw as i32, ((h * bw + w / 2) / w).clamp(1, bh) as i32)
    } else {
        (((w * bh + h / 2) / h).clamp(1, bw) as i32, bh as i32)
    }
}

/// Whether an image of EXIF orientation `o` (1 to 8) is stored on its side:
/// shown upright, its width and height trade places.
pub fn swaps(o: u16) -> bool {
    matches!(o, 5..=8)
}

/// Writes into `dst` the `w` by `h` pixels of `src` turned upright from
/// EXIF orientation `o`; `dst` is `h` by `w` when `swaps(o)`. The EXIF
/// definitions: for each value, the side of the picture the stored first
/// row and first column are on (1: top, left; 2: top, right; 3: bottom,
/// right; 4: bottom, left; 5: left, top; 6: right, top; 7: right, bottom;
/// 8: left, bottom).
pub fn orient_into(src: &[[u8; 4]], w: usize, h: usize, o: u16, dst: &mut [[u8; 4]]) {
    debug_assert_eq!(src.len(), w * h);
    debug_assert_eq!(dst.len(), w * h);
    let out_w = if swaps(o) { h } else { w };
    for (i, px) in dst.iter_mut().enumerate() {
        let (x, y) = (i % out_w, i / out_w);
        // The stored column and row shown at (x, y).
        let (c, r) = match o {
            2 => (w - 1 - x, y),
            3 => (w - 1 - x, h - 1 - y),
            4 => (x, h - 1 - y),
            5 => (y, x),
            6 => (y, h - 1 - x),
            7 => (w - 1 - y, h - 1 - x),
            8 => (w - 1 - y, x),
            _ => (x, y),
        };
        *px = src[r * w + c];
    }
}

/// A decoded image: premultiplied pixels, upright, at most the box it was
/// decoded into; `natural` is the file's own size, upright.
pub struct Decoded {
    pub bitmap: Bitmap,
    pub natural: (i32, i32),
    /// No pixel is see-through: it can be scaled straight onto the pane.
    pub opaque: bool,
}

/// WIC on the preview's thread (COM is initialized there).
pub struct Wic {
    factory: IWICImagingFactory,
    /// The extensions WIC's decoders take (`.jpg`, lower case), listed
    /// the first time they are asked for.
    extensions: Option<HashSet<Vec<u16>>>,
}

impl Wic {
    pub fn new() -> Option<Wic> {
        // SAFETY: COM is initialized on this thread; WIC's factory is an
        // in-process server.
        let factory: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .ok()?;
        Some(Wic {
            factory,
            extensions: None,
        })
    }

    /// Whether a decoder takes files of extension `ext` (with its dot, lower
    /// case).
    pub fn decodes(&mut self, ext: &[u16]) -> bool {
        if self.extensions.is_none() {
            self.extensions = Some(self.list_extensions());
        }
        self.extensions.as_ref().is_some_and(|e| e.contains(ext))
    }

    fn list_extensions(&self) -> HashSet<Vec<u16>> {
        let mut out = HashSet::new();
        // SAFETY: walks WIC's decoders with live COM objects; each buffer is
        // passed with its length.
        unsafe {
            let Ok(list) = self.factory.CreateComponentEnumerator(
                WICDecoder.0 as u32,
                WICComponentEnumerateDefault.0 as u32,
            ) else {
                return out;
            };
            loop {
                let mut item = [None];
                let mut fetched = 0u32;
                if list.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                let Some(info) = item[0]
                    .take()
                    .and_then(|u| u.cast::<IWICBitmapCodecInfo>().ok())
                else {
                    continue;
                };
                let mut buf = vec![0u16; 1024];
                let mut len = 0u32;
                if info.GetFileExtensions(&mut buf, &mut len).is_err() {
                    if len as usize <= buf.len() {
                        continue;
                    }
                    buf = vec![0u16; len as usize];
                    if info.GetFileExtensions(&mut buf, &mut len).is_err() {
                        continue;
                    }
                }
                let text = &buf[..(len as usize).min(buf.len())];
                let text = text.split(|&c| c == 0).next().unwrap_or_default();
                for ext in String::from_utf16_lossy(text).split(',') {
                    let ext = ext.trim().to_lowercase();
                    if ext.starts_with('.') && ext.len() > 1 {
                        out.insert(ext.encode_utf16().collect());
                    }
                }
            }
        }
        out
    }

    /// Decodes `path` into a box `side` pixels square (never bigger than
    /// the file's own size), upright.
    pub fn decode(&self, path: &[u16], side: i32) -> windows::core::Result<Decoded> {
        let file = nul(path);
        // SAFETY: COM calls on live objects of this thread; the pixel
        // buffers are passed with their lengths.
        unsafe {
            let decoder = self.factory.CreateDecoderFromFilename(
                PCWSTR(file.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?;
            let container = decoder.GetContainerFormat()?;
            let frame = best_frame(&decoder, container)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h)?;
            if w == 0 || h == 0 || w > i32::MAX as u32 || h > i32::MAX as u32 {
                return Err(E_FAIL.into());
            }
            let o = orientation(&frame, container);
            let stored = (w as i32, h as i32);
            let upright = if swaps(o) {
                (stored.1, stored.0)
            } else {
                stored
            };
            let shown = fit(upright, (side, side), false);
            let (tw, th) = if swaps(o) { (shown.1, shown.0) } else { shown };
            let source: IWICBitmapSource = if (tw, th) == stored {
                frame.cast()?
            } else {
                let scaler = self.factory.CreateBitmapScaler()?;
                scaler.Initialize(&frame, tw as u32, th as u32, WICBitmapInterpolationModeFant)?;
                scaler.cast()?
            };
            let converter = self.factory.CreateFormatConverter()?;
            converter.Initialize(
                &source,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            let (w, h) = (tw as usize, th as usize);
            let stride = (w * 4) as u32;
            let copy = |pixels: &mut [[u8; 4]]| {
                let bytes = core::slice::from_raw_parts_mut(
                    pixels.as_mut_ptr().cast::<u8>(),
                    pixels.len() * 4,
                );
                converter
                    .CopyPixels(core::ptr::null(), stride, bytes)
                    .is_ok()
            };
            let bitmap = if o <= 1 || o > 8 {
                Bitmap::with_pixels(tw, th, copy)
            } else {
                let mut stored_px = vec![[0u8; 4]; w * h];
                if !copy(&mut stored_px) {
                    return Err(E_FAIL.into());
                }
                let (ow, oh) = if swaps(o) { (th, tw) } else { (tw, th) };
                Bitmap::with_pixels(ow, oh, |dst| {
                    orient_into(&stored_px, w, h, o, dst);
                    true
                })
            }
            .ok_or(windows::core::Error::from(E_OUTOFMEMORY))?;
            let opaque = pixels_opaque(&bitmap);
            Ok(Decoded {
                bitmap,
                natural: upright,
                opaque,
            })
        }
    }
}

/// The frame to show: the first, or of an icon file the biggest.
fn best_frame(
    decoder: &IWICBitmapDecoder,
    container: GUID,
) -> windows::core::Result<IWICBitmapFrameDecode> {
    // SAFETY: COM calls on a live decoder of this thread.
    unsafe {
        if container != GUID_ContainerFormatIco {
            return decoder.GetFrame(0);
        }
        let mut best = decoder.GetFrame(0)?;
        let mut area = 0u64;
        for i in 0..decoder.GetFrameCount()?.min(64) {
            let Ok(frame) = decoder.GetFrame(i) else {
                continue;
            };
            let (mut w, mut h) = (0u32, 0u32);
            if frame.GetSize(&mut w, &mut h).is_ok() && u64::from(w) * u64::from(h) > area {
                area = u64::from(w) * u64::from(h);
                best = frame;
            }
        }
        Ok(best)
    }
}

/// The EXIF orientation of a JPEG's or TIFF's frame (1, upright, when it
/// has none); other formats carry their turn in their own data, which
/// their decoders apply.
fn orientation(frame: &IWICBitmapFrameDecode, container: GUID) -> u16 {
    let query = if container == GUID_ContainerFormatJpeg {
        "/app1/ifd/{ushort=274}"
    } else if container == GUID_ContainerFormatTiff {
        "/ifd/{ushort=274}"
    } else {
        return 1;
    };
    let query = wz(query);
    // SAFETY: COM calls on a live frame of this thread; the PROPVARIANT is
    // cleared after it is read.
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else {
            return 1;
        };
        let mut value = PROPVARIANT::default();
        if reader
            .GetMetadataByName(PCWSTR(query.as_ptr()), &mut value)
            .is_err()
        {
            return 1;
        }
        let o = PropVariantToUInt16(&value).unwrap_or(1);
        let _ = PropVariantClear(&mut value);
        if (1..=8).contains(&o) { o } else { 1 }
    }
}

/// Whether no pixel of `bitmap` lets the background through.
fn pixels_opaque(bitmap: &Bitmap) -> bool {
    use windows_sys::Win32::Graphics::Gdi::{DIBSECTION, GetObjectW};
    // SAFETY: reads the DIB section's header, then its pixels, which the
    // bitmap owns and nothing writes meanwhile.
    unsafe {
        let mut ds: DIBSECTION = core::mem::zeroed();
        if GetObjectW(
            bitmap.handle(),
            size_of::<DIBSECTION>() as i32,
            (&raw mut ds).cast(),
        ) == 0
            || ds.dsBm.bmBits.is_null()
        {
            return false;
        }
        let n = (bitmap.width * bitmap.height) as usize;
        let px = core::slice::from_raw_parts(ds.dsBm.bmBits.cast::<[u8; 4]>(), n);
        px.iter().all(|p| p[3] == 255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_fit_the_pane_with_their_aspect() {
        // Small enough: as it is, unless enlarging.
        assert_eq!(fit((100, 50), (256, 700), false), (100, 50));
        assert_eq!(fit((100, 50), (256, 700), true), (256, 128));
        // Too wide, too tall.
        assert_eq!(fit((1000, 500), (256, 700), false), (256, 128));
        assert_eq!(fit((500, 2000), (256, 700), false), (175, 700));
        // Exactly the bounds.
        assert_eq!(fit((256, 700), (256, 700), false), (256, 700));
        // A very thin image keeps a pixel.
        assert_eq!(fit((10_000, 1), (256, 700), false), (256, 1));
        assert_eq!(fit((1, 10_000), (256, 700), false), (1, 700));
        // Degenerate sizes are a pixel.
        assert_eq!(fit((0, 0), (256, 700), false), (1, 1));
        assert_eq!(fit((50, 50), (0, 0), false), (1, 1));
        // Decoding into Everything's 1024 box.
        assert_eq!(fit((4000, 3000), (1024, 1024), false), (1024, 768));
        assert_eq!(fit((3000, 4000), (1024, 1024), false), (768, 1024));
    }

    /// A 3 by 2 image whose pixels are numbered 0..6 in reading order.
    fn numbered() -> Vec<[u8; 4]> {
        (0..6).map(|i| [i, 0, 0, 255]).collect()
    }

    fn turned(o: u16) -> Vec<u8> {
        let src = numbered();
        let mut dst = vec![[0u8; 4]; 6];
        orient_into(&src, 3, 2, o, &mut dst);
        dst.iter().map(|p| p[0]).collect()
    }

    #[test]
    fn exif_orientations_turn_the_image_upright() {
        // Stored:  0 1 2
        //          3 4 5
        assert_eq!(turned(1), [0, 1, 2, 3, 4, 5]);
        // 2, mirrored: the first column is on the right.
        assert_eq!(turned(2), [2, 1, 0, 5, 4, 3]);
        // 3, upside down.
        assert_eq!(turned(3), [5, 4, 3, 2, 1, 0]);
        // 4, flipped: the first row is at the bottom.
        assert_eq!(turned(4), [3, 4, 5, 0, 1, 2]);
        // The rest come out 2 wide and 3 tall.
        // 5, transposed: the first row is the left column.
        assert_eq!(turned(5), [0, 3, 1, 4, 2, 5]);
        // 6, a camera turned clockwise: the first row is the right column,
        // read top down; the first column is the top row.
        assert_eq!(turned(6), [3, 0, 4, 1, 5, 2]);
        // 7: the first row is the right column, the first column the
        // bottom row.
        assert_eq!(turned(7), [5, 2, 4, 1, 3, 0]);
        // 8, turned anticlockwise: the first row is the left column, read
        // bottom up.
        assert_eq!(turned(8), [2, 5, 1, 4, 0, 3]);
        // Unknown values leave it as stored.
        assert_eq!(turned(0), turned(1));
        assert!(swaps(6) && swaps(8) && !swaps(3) && !swaps(1));
    }
}
