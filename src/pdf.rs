//! Loading the PDFium dynamic library and rasterising part of a page to RGBA.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use kurbo::{Point, Rect, Size};
use pdfium_render::prelude::*;

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

/// A rasterised patch of a page: tightly packed RGBA8, ready to upload as a
/// texture, together with the page-space rectangle it covers.
pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Page space, in points. Snapped outwards to whole rendered pixels, so
    /// this is not exactly the region that was asked for.
    pub region: Rect,
}

/// Where the dynamic library is expected: alongside the executable.
fn library_path() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    Pdfium::pdfium_platform_library_name_at_path(&dir)
}

/// Binds to PDFium once per process, preferring a copy next to the executable
/// and falling back to a system-wide install.
fn library() -> Result<&'static Pdfium, String> {
    if let Some(pdfium) = PDFIUM.get() {
        return Ok(pdfium);
    }

    let path = library_path();
    let bindings = Pdfium::bind_to_library(&path)
        .or_else(|_| Pdfium::bind_to_system_library())
        .map_err(|e| {
            format!(
                "Could not load the PDFium library ({e}).\n\n\
                 Place it at:\n{}\n\nor install it system-wide.",
                path.display()
            )
        })?;

    Ok(PDFIUM.get_or_init(|| Pdfium::new(bindings)))
}

pub struct Document {
    inner: PdfDocument<'static>,
}

impl Document {
    pub fn open(path: &Path) -> Result<Self, String> {
        let pdfium = library()?;
        let inner = pdfium
            .load_pdf_from_file(path, None)
            .map_err(|e| format!("Could not open {}:\n{e}", path.display()))?;

        Ok(Self { inner })
    }

    pub fn page_count(&self) -> usize {
        self.inner.pages().len().max(0) as usize
    }

    /// The page's extent in points.
    pub fn page_size(&self, index: usize) -> Result<Size, String> {
        let page = self.page(index)?;
        let size = Size::new(page.width().value as f64, page.height().value as f64);

        if size.width <= 0.0 || size.height <= 0.0 {
            return Err(format!("Page {} has no extent.", index + 1));
        }

        Ok(size)
    }

    /// Rasterises the part of page `index` covered by `region` (page space, in
    /// points) at `scale` pixels per point.
    ///
    /// Only the requested region is rendered: a large sheet at a high zoom
    /// never allocates a texture for the whole page. `region` is clipped to
    /// the page and snapped outwards to whole pixels; the region actually
    /// covered comes back on the [`Raster`].
    pub fn render_region(&self, index: usize, region: Rect, scale: f64) -> Result<Raster, String> {
        let size = self.page_size(index)?;
        let page = self.page(index)?;

        let wanted = region.intersect(Rect::from_origin_size(Point::ZERO, size));
        if wanted.is_zero_area() {
            return Err(format!("Page {} is not in view.", index + 1));
        }

        // Snap to whole pixels of the scaled page so the texture lands on
        // pixel boundaries and the covered region is exact.
        let left = (wanted.x0 * scale).floor();
        let top = (wanted.y0 * scale).floor();
        let width = ((wanted.x1 * scale).ceil() - left).max(1.0) as i32;
        let height = ((wanted.y1 * scale).ceil() - top).max(1.0) as i32;

        let config = PdfRenderConfig::new()
            .set_target_size(
                (size.width * scale).round().max(1.0) as i32,
                (size.height * scale).round().max(1.0) as i32,
            )
            // Push the page up and left so the wanted region lands at the
            // bitmap's origin; PDFium clips the rest away for us.
            .set_origin(-left as i32, -top as i32);

        let mut bitmap = PdfBitmap::empty(width, height, PdfBitmapFormat::BGRA)
            .map_err(|e| format!("Could not allocate a {width}x{height} bitmap:\n{e}"))?;

        page.render_into_bitmap_with_config(&mut bitmap, &config)
            .map_err(|e| format!("Could not render page {}:\n{e}", index + 1))?;

        Ok(Raster {
            width: width as usize,
            height: height as usize,
            // PDFium is configured to reverse its own byte order, so this is
            // a channel normalisation rather than a copy of a swap loop.
            rgba: bitmap.as_rgba_bytes(),
            region: Rect::new(
                left / scale,
                top / scale,
                (left + width as f64) / scale,
                (top + height as f64) / scale,
            ),
        })
    }

    fn page(&self, index: usize) -> Result<PdfPage<'static>, String> {
        self.inner
            .pages()
            .get(index as PdfPageIndex)
            .map_err(|e| format!("Could not read page {}:\n{e}", index + 1))
    }
}
