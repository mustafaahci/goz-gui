//! The Windows Imaging Component: decoders for the image files Windows (and
//! the codecs installed on it) can read, frames, scaling and conversion to
//! 32-bit BGRA, pixels copied out. COM must be initialised on the thread,
//! which the objects then belong to.

use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapCodecInfo, IWICBitmapDecoder,
    IWICBitmapFrameDecode, IWICBitmapSource, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICComponentEnumerateDefault, WICDecodeMetadataCacheOnDemand, WICDecoder,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToUInt16,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{GUID, Interface, PCWSTR, Result};

use super::{nul_ended, until_nul, wz};

/// WIC's factory (in process).
pub struct Factory(IWICImagingFactory);

impl Factory {
    /// A new one; `None` without COM on the thread.
    pub fn new() -> Option<Factory> {
        // SAFETY: a class id and no outer object; the object comes back
        // owned.
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            .ok()
            .map(Factory)
    }

    /// The file extensions each of WIC's decoders takes, as each lists them
    /// (`.jpg,.jpeg,…`).
    pub fn decoder_extensions(&self) -> Vec<Vec<u16>> {
        let mut out = Vec::new();
        // SAFETY: a call on the factory this holds.
        let list = unsafe {
            self.0.CreateComponentEnumerator(
                WICDecoder.0 as u32,
                WICComponentEnumerateDefault.0 as u32,
            )
        };
        let Ok(list) = list else {
            return out;
        };
        loop {
            let mut item = [None];
            let mut fetched = 0u32;
            // SAFETY: one object into a local array of one, and the count.
            if unsafe { list.Next(&mut item, Some(&mut fetched)) }.is_err() || fetched == 0 {
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
            // SAFETY: the buffer (its length passed with it) and a count
            // out.
            if unsafe { info.GetFileExtensions(&mut buf, &mut len) }.is_err() {
                if len as usize <= buf.len() {
                    continue;
                }
                buf = vec![0u16; len as usize];
                // SAFETY: as above, with a buffer of the length asked for.
                if unsafe { info.GetFileExtensions(&mut buf, &mut len) }.is_err() {
                    continue;
                }
            }
            let text = &buf[..(len as usize).min(buf.len())];
            out.push(until_nul(text).to_vec());
        }
        out
    }

    /// A decoder for the file at `path`, reading it
    /// (CreateDecoderFromFilename, metadata read when asked for).
    pub fn decoder(&self, path: &[u16]) -> Result<Decoder> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated path read during the call; no vendor.
        unsafe {
            self.0.CreateDecoderFromFilename(
                PCWSTR(path.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
        }
        .map(Decoder)
    }

    /// `source` scaled to `w` by `h` (a bitmap scaler, Fant).
    pub fn scaled(&self, source: &Source, w: u32, h: u32) -> Result<Source> {
        // SAFETY: calls on objects of this thread.
        let scaler = unsafe {
            let scaler = self.0.CreateBitmapScaler()?;
            scaler.Initialize(&source.0, w, h, WICBitmapInterpolationModeFant)?;
            scaler
        };
        Ok(Source(scaler.cast()?))
    }

    /// `source` as 32-bit premultiplied BGRA (a format converter, no
    /// dither, no palette).
    pub fn pbgra(&self, source: &Source) -> Result<Source> {
        // SAFETY: calls on objects of this thread; the format's GUID is read
        // during the call.
        let converter = unsafe {
            let converter = self.0.CreateFormatConverter()?;
            converter.Initialize(
                &source.0,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            converter
        };
        Ok(Source(IWICBitmapSource::clone(&converter)))
    }
}

/// A decoder of an image file.
pub struct Decoder(IWICBitmapDecoder);

impl Decoder {
    /// Its container format (GUID_ContainerFormat*).
    pub fn container(&self) -> Result<GUID> {
        // SAFETY: a call on the decoder this holds.
        unsafe { self.0.GetContainerFormat() }
    }

    /// How many frames the file has.
    pub fn frame_count(&self) -> Result<u32> {
        // SAFETY: a call on the decoder this holds.
        unsafe { self.0.GetFrameCount() }
    }

    /// Frame `i`.
    pub fn frame(&self, i: u32) -> Result<Frame> {
        // SAFETY: a call on the decoder this holds; the index it checks.
        unsafe { self.0.GetFrame(i) }.map(Frame)
    }
}

/// A frame of an image file.
pub struct Frame(IWICBitmapFrameDecode);

impl Frame {
    /// Its size: (width, height).
    pub fn size(&self) -> Result<(u32, u32)> {
        size(&self.0)
    }

    /// Its metadata at `query` (`/app1/ifd/{ushort=274}`) as a u16; `None`
    /// when it has none there, or one that is no number.
    pub fn metadata_u16(&self, query: &str) -> Option<u16> {
        let query = wz(query);
        // SAFETY: calls on the frame this holds; the PROPVARIANT is cleared
        // after it is read.
        unsafe {
            let reader = self.0.GetMetadataQueryReader().ok()?;
            let mut value = PROPVARIANT::default();
            reader
                .GetMetadataByName(PCWSTR(query.as_ptr()), &mut value)
                .ok()?;
            let n = PropVariantToUInt16(&value);
            let _ = PropVariantClear(&mut value);
            n.ok()
        }
    }

    /// Its pixels, as a source to scale, convert or copy.
    pub fn source(&self) -> Result<Source> {
        self.0.cast().map(Source)
    }
}

/// Pixels to scale, convert or copy (IWICBitmapSource).
pub struct Source(IWICBitmapSource);

impl Source {
    /// Copies all of its 32-bit pixels, rows of `width`, into `out`
    /// (CopyPixels); false if it failed (the size or format did not fit).
    pub fn copy_pixels(&self, width: u32, out: &mut [[u8; 4]]) -> bool {
        let Some(stride) = width.checked_mul(4) else {
            return false;
        };
        // SAFETY: the whole rectangle, into a buffer whose length is passed
        // with it (WIC writes no more).
        unsafe {
            self.0
                .CopyPixels(core::ptr::null(), stride, out.as_flattened_mut())
        }
        .is_ok()
    }
}

/// A bitmap source's size: (width, height).
fn size(source: &IWICBitmapSource) -> Result<(u32, u32)> {
    let (mut w, mut h) = (0u32, 0u32);
    // SAFETY: two out-values alive during the call.
    unsafe { source.GetSize(&mut w, &mut h) }?;
    Ok((w, h))
}
