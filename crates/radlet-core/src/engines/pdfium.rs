//! Renders PDF pages to pixels with PDFium (BSD-3-Clause/Apache-2.0), the
//! engine Chrome uses. The library is loaded at run time from next to the
//! `radlet` binary, so builds without it simply don't offer PDF-to-image.
//!
//! Only the dozen C functions needed for rendering are bound. PDFium isn't
//! thread-safe, so every call happens under one process-wide lock; workers
//! are separate processes, so batches still render in parallel.

use std::ffi::{c_char, c_int, c_ulong, c_void, CString};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use image::{DynamicImage, RgbImage};

use crate::catalog::Capabilities;
use crate::error::{Error, Result};

type Handle = *mut c_void;

// Signatures from PDFium's public header, fpdfview.h.
type FnInit = unsafe extern "C" fn();
type FnLoadDocument = unsafe extern "C" fn(*const c_char, *const c_char) -> Handle;
type FnLastError = unsafe extern "C" fn() -> c_ulong;
type FnPageCount = unsafe extern "C" fn(Handle) -> c_int;
type FnLoadPage = unsafe extern "C" fn(Handle, c_int) -> Handle;
type FnPageSize = unsafe extern "C" fn(Handle) -> f32;
type FnBitmapCreate = unsafe extern "C" fn(c_int, c_int, c_int) -> Handle;
type FnBitmapFill = unsafe extern "C" fn(Handle, c_int, c_int, c_int, c_int, c_ulong) -> c_int;
type FnRender = unsafe extern "C" fn(Handle, Handle, c_int, c_int, c_int, c_int, c_int, c_int);
type FnBitmapBuffer = unsafe extern "C" fn(Handle) -> *mut c_void;
type FnBitmapStride = unsafe extern "C" fn(Handle) -> c_int;
type FnClose = unsafe extern "C" fn(Handle);

/// Function pointers resolved from the PDFium shared library.
struct Api {
    load_document: FnLoadDocument,
    get_last_error: FnLastError,
    get_page_count: FnPageCount,
    load_page: FnLoadPage,
    page_width: FnPageSize,
    page_height: FnPageSize,
    bitmap_create: FnBitmapCreate,
    bitmap_fill: FnBitmapFill,
    render: FnRender,
    bitmap_buffer: FnBitmapBuffer,
    bitmap_stride: FnBitmapStride,
    bitmap_destroy: FnClose,
    close_page: FnClose,
    close_document: FnClose,
}

// SAFETY: the struct only holds C function pointers, which may be called
// from any thread; PDFium's own thread-unsafety is handled by `LOCK`.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static LOCK: Mutex<()> = Mutex::new(());
static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();

/// Render annotations and form field appearances, like a viewer does.
const FPDF_ANNOT: c_int = 0x01;
/// Largest page bitmap: 40 megapixels and 16,384 pixels a side.
const MAX_PIXELS: f64 = 40_000_000.0;
const MAX_SIDE: f64 = 16_384.0;

#[cfg(unix)]
fn load_api(path: &Path) -> std::result::Result<Api, String> {
    use std::os::unix::ffi::OsStrExt;
    let cpath =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| "bad library path".to_string())?;
    // SAFETY: dlopen/dlsym are called with valid C strings; each symbol is
    // checked for null and cast to the signature declared in fpdfview.h.
    unsafe {
        let lib = libc::dlopen(cpath.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if lib.is_null() {
            return Err(format!("couldn't load PDFium from {}", path.display()));
        }
        let sym = |name: &str| -> std::result::Result<*mut c_void, String> {
            let cname = CString::new(name).expect("static name");
            let p = libc::dlsym(lib, cname.as_ptr());
            if p.is_null() {
                Err(format!("PDFium is missing {name}"))
            } else {
                Ok(p)
            }
        };
        macro_rules! bind {
            ($name:literal, $ty:ty) => {
                std::mem::transmute::<*mut c_void, $ty>(sym($name)?)
            };
        }
        let init = bind!("FPDF_InitLibrary", FnInit);
        let api = Api {
            load_document: bind!("FPDF_LoadDocument", FnLoadDocument),
            get_last_error: bind!("FPDF_GetLastError", FnLastError),
            get_page_count: bind!("FPDF_GetPageCount", FnPageCount),
            load_page: bind!("FPDF_LoadPage", FnLoadPage),
            page_width: bind!("FPDF_GetPageWidthF", FnPageSize),
            page_height: bind!("FPDF_GetPageHeightF", FnPageSize),
            bitmap_create: bind!("FPDFBitmap_Create", FnBitmapCreate),
            bitmap_fill: bind!("FPDFBitmap_FillRect", FnBitmapFill),
            render: bind!("FPDF_RenderPageBitmap", FnRender),
            bitmap_buffer: bind!("FPDFBitmap_GetBuffer", FnBitmapBuffer),
            bitmap_stride: bind!("FPDFBitmap_GetStride", FnBitmapStride),
            bitmap_destroy: bind!("FPDFBitmap_Destroy", FnClose),
            close_page: bind!("FPDF_ClosePage", FnClose),
            close_document: bind!("FPDF_CloseDocument", FnClose),
        };
        init();
        Ok(api)
    }
}

#[cfg(not(unix))]
fn load_api(_path: &Path) -> std::result::Result<Api, String> {
    Err("PDF rendering isn't available on this platform yet".into())
}

fn api(caps: &Capabilities) -> Result<&'static Api> {
    let path = caps.pdfium.as_ref().ok_or_else(|| {
        Error::Missing("PDF to image needs the PDFium library next to radlet".into())
    })?;
    API.get_or_init(|| load_api(path))
        .as_ref()
        .map_err(|e| Error::Missing(e.clone()))
}

/// Closes a PDFium object when dropped, so every early return cleans up.
struct Guard {
    handle: Handle,
    close: FnClose,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: `handle` came from the matching PDFium constructor and is
        // closed exactly once, here.
        unsafe { (self.close)(self.handle) }
    }
}

/// Pixel size for a page of `w` x `h` points at `dpi`, scaled down to stay
/// within the bitmap limits.
pub fn page_pixels(w_pt: f32, h_pt: f32, dpi: u32) -> (u32, u32) {
    let mut scale = dpi.max(1) as f64 / 72.0;
    let (w, h) = (w_pt.max(1.0) as f64, h_pt.max(1.0) as f64);
    let side = (w.max(h)) * scale;
    if side > MAX_SIDE {
        scale *= MAX_SIDE / side;
    }
    let area = w * h * scale * scale;
    if area > MAX_PIXELS {
        scale *= (MAX_PIXELS / area).sqrt();
    }
    (
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    )
}

/// Number of pages, using PDFium (handles files lopdf can't parse).
pub fn page_count(caps: &Capabilities, path: &Path) -> Result<u32> {
    let api = api(caps)?;
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let doc = open(api, path)?;
    // SAFETY: `doc` is a live document handle.
    let n = unsafe { (api.get_page_count)(doc.handle) };
    Ok(n.max(0) as u32)
}

fn open(api: &Api, path: &Path) -> Result<Guard> {
    #[cfg(unix)]
    let cpath = {
        use std::os::unix::ffi::OsStrExt;
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| Error::Invalid("bad file name".into()))?
    };
    #[cfg(not(unix))]
    let cpath = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| Error::Invalid("bad file name".into()))?;
    // SAFETY: valid C string; a null password means "none".
    let doc = unsafe { (api.load_document)(cpath.as_ptr(), std::ptr::null()) };
    if doc.is_null() {
        // SAFETY: no arguments; reads PDFium's last error for this thread.
        let code = unsafe { (api.get_last_error)() };
        return Err(Error::Pdf(
            match code {
                2 => "the PDF couldn't be opened",
                3 => "this file isn't a valid PDF",
                4 => "this PDF is password-protected",
                5 => "this PDF uses unsupported security",
                _ => "PDFium couldn't read this PDF",
            }
            .into(),
        ));
    }
    Ok(Guard {
        handle: doc,
        close: api.close_document,
    })
}

/// Renders pages (all, or `pages` numbered from 1) at `dpi` on a white
/// background, calling `each(page_number, total_pages, image)` in order.
pub fn render(
    caps: &Capabilities,
    path: &Path,
    dpi: u32,
    pages: Option<&[u32]>,
    each: &mut dyn FnMut(u32, u32, DynamicImage) -> Result<()>,
) -> Result<u32> {
    let api = api(caps)?;
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let doc = open(api, path)?;
    // SAFETY: `doc` is a live document handle.
    let total = unsafe { (api.get_page_count)(doc.handle) }.max(0) as u32;
    if total == 0 {
        return Err(Error::Pdf("this PDF has no pages".into()));
    }
    let wanted: Vec<u32> = match pages {
        Some(p) => p.to_vec(),
        None => (1..=total).collect(),
    };
    for &number in &wanted {
        if number == 0 || number > total {
            return Err(Error::Invalid(format!(
                "page {number} doesn't exist (the PDF has {total})"
            )));
        }
        // SAFETY: index checked against the page count above.
        let page = unsafe { (api.load_page)(doc.handle, (number - 1) as c_int) };
        if page.is_null() {
            return Err(Error::Pdf(format!("page {number} couldn't be read")));
        }
        let page = Guard {
            handle: page,
            close: api.close_page,
        };
        // SAFETY: live page handle. Sizes include the page's rotation.
        let (w_pt, h_pt) = unsafe {
            (
                (api.page_width)(page.handle),
                (api.page_height)(page.handle),
            )
        };
        let (w, h) = page_pixels(w_pt, h_pt, dpi);
        // SAFETY: dimensions are positive and bounded by MAX_PIXELS; alpha 0
        // gives a 4-byte BGRx bitmap.
        let bitmap = unsafe { (api.bitmap_create)(w as c_int, h as c_int, 0) };
        if bitmap.is_null() {
            return Err(Error::TooLarge(format!(
                "page {number} is too large to render"
            )));
        }
        let bitmap = Guard {
            handle: bitmap,
            close: api.bitmap_destroy,
        };
        let image = unsafe {
            // SAFETY: bitmap and page are live; the fill and render cover
            // exactly the bitmap's area.
            (api.bitmap_fill)(bitmap.handle, 0, 0, w as c_int, h as c_int, 0xFFFF_FFFF);
            (api.render)(
                bitmap.handle,
                page.handle,
                0,
                0,
                w as c_int,
                h as c_int,
                0,
                FPDF_ANNOT,
            );
            let stride = (api.bitmap_stride)(bitmap.handle);
            let buffer = (api.bitmap_buffer)(bitmap.handle) as *const u8;
            if buffer.is_null() || stride < (w * 4) as c_int {
                return Err(Error::Pdf(format!("page {number} couldn't be rendered")));
            }
            // SAFETY: PDFium owns `h` rows of `stride` bytes at `buffer`
            // until the bitmap is destroyed (after this block).
            let bytes = std::slice::from_raw_parts(buffer, stride as usize * h as usize);
            let mut rgb = RgbImage::new(w, h);
            for (y, row) in bytes.chunks_exact(stride as usize).enumerate() {
                for (x, px) in row[..(w * 4) as usize]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                {
                    // BGRx -> RGB
                    rgb.put_pixel(x as u32, y as u32, image::Rgb([px[2], px[1], px[0]]));
                }
            }
            rgb
        };
        drop(bitmap);
        drop(page);
        each(number, total, DynamicImage::ImageRgb8(image))?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_limits() {
        // A4 at 300 dpi.
        assert_eq!(page_pixels(595.0, 842.0, 300), (2479, 3508));
        // An A0 poster is scaled down to the pixel budget.
        let (w, h) = page_pixels(2384.0, 3370.0, 300);
        assert!((w as f64) * (h as f64) <= MAX_PIXELS * 1.001);
        assert!(w as f64 <= MAX_SIDE && h as f64 <= MAX_SIDE);
        // A very long strip is limited by its longest side.
        let (_, h) = page_pixels(100.0, 100_000.0, 300);
        assert!(h as f64 <= MAX_SIDE);
    }
}
