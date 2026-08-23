//! Loading the PDFium dynamic library and rasterising a page to RGBA.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::*;

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

/// A page rasterised to tightly packed RGBA8, ready to upload as a texture.
pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Where the dynamic library is expected: alongside the executable.
fn library_path() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    PathBuf::from(Pdfium::pdfium_platform_library_name_at_path(&dir))
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

    /// Rasterises the first page to fit within `max_width` x `max_height`
    /// pixels, preserving the page's aspect ratio.
    pub fn render_first_page(&self, max_width: i32, max_height: i32) -> Result<Raster, String> {
        let page = self
            .inner
            .pages()
            .get(0)
            .map_err(|e| format!("Could not read page 1:\n{e}"))?;

        let page_width = page.width().value;
        let page_height = page.height().value;
        if page_width <= 0.0 || page_height <= 0.0 {
            return Err("Page 1 has no extent.".to_owned());
        }

        let scale = (max_width as f32 / page_width).min(max_height as f32 / page_height);
        let target_width = (page_width * scale).round().max(1.0) as i32;
        let target_height = (page_height * scale).round().max(1.0) as i32;

        let config = PdfRenderConfig::new().set_target_size(target_width, target_height);
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| format!("Could not render page 1:\n{e}"))?;

        let width = bitmap.width() as usize;
        let height = bitmap.height() as usize;
        let mut rgba = bitmap.as_raw_bytes().to_vec();

        // PDFium hands back BGRA; the painter wants RGBA.
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        Ok(Raster {
            width,
            height,
            rgba,
        })
    }
}
