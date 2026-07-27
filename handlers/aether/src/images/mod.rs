use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// Decoded images for the current page, keyed by ABSOLUTE url.
// Thread-local like js::DOM_STATE — the engine, layout, and renderer all
// run on one thread; callers populate it via `set_page` before load.
thread_local! {
    static STORE: RefCell<PageImages> = RefCell::new(PageImages {
        base_url: String::new(),
        images: HashMap::new(),
    });
}

struct PageImages {
    base_url: String,
    images: HashMap<String, Rc<image::RgbaImage>>,
}

/// Installs the current page's decoded images (see net::fetch_page).
pub fn set_page(base_url: &str, images: Vec<(String, image::RgbaImage)>) {
    STORE.with(|s| {
        let mut s = s.borrow_mut();
        s.base_url = base_url.to_string();
        s.images = images.into_iter().map(|(k, v)| (k, Rc::new(v))).collect();
    });
}

/// Resolves an `src` attribute against the page base and returns the
/// decoded image. Misses are ledgered (img-missing) — honest gaps.
pub fn get(src: &str) -> Option<Rc<image::RgbaImage>> {
    // data: URIs decode synchronously (and cache) on first use.
    if let Some(rest) = src.strip_prefix("data:") {
        return STORE.with(|s| {
            if let Some(img) = s.borrow().images.get(src) {
                return Some(img.clone());
            }
            let decoded = decode_data_uri(rest)?;
            let rc = Rc::new(decoded);
            s.borrow_mut().images.insert(src.to_string(), rc.clone());
            Some(rc)
        });
    }
    STORE.with(|s| {
        let s = s.borrow();
        let abs = resolve(&s.base_url, src);
        match s.images.get(&abs) {
            Some(img) => Some(img.clone()),
            None => {
                crate::ledger::record_dom(&format!("img-missing:{}", &abs[..abs.len().min(48)]));
                None
            }
        }
    })
}

/// Decodes the payload of a data: URI (base64 raster, or SVG in base64 or
/// percent-encoded/plain form).
fn decode_data_uri(rest: &str) -> Option<image::RgbaImage> {
    use base64::Engine as _;
    let (meta, payload) = rest.split_once(',')?;
    let bytes = if meta.contains("base64") {
        base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()?
    } else {
        // Plain/percent-encoded payload (common for inline SVG).
        percent_decode(payload).into_bytes()
    };
    let decoded = if meta.contains("svg") {
        decode_svg(&bytes)
    } else {
        image::load_from_memory(&bytes).ok().map(|i| i.to_rgba8())
    };
    if decoded.is_none() {
        crate::ledger::record_dom("img-data-uri-decode-failed");
    }
    decoded
}

/// Minimal %XX decoder for data: URI payloads ('+' left as-is per RFC 2397).
fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Rasterizes SVG bytes at the document's intrinsic size (capped) into a
/// straight-alpha RGBA image.
pub fn decode_svg(bytes: &[u8]) -> Option<image::RgbaImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    // Cap the raster so a viewBox-huge svg cannot allocate wildly; the
    // renderer scales at paint time anyway.
    let scale = (1024.0 / size.width().max(size.height())).min(1.0);
    let w = (size.width() * scale).ceil().max(1.0) as u32;
    let h = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia pixels are premultiplied; the blitters expect straight alpha.
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            px[0] = (px[0] as u32 * 255 / a).min(255) as u8;
            px[1] = (px[1] as u32 * 255 / a).min(255) as u8;
            px[2] = (px[2] as u32 * 255 / a).min(255) as u8;
        }
    }
    image::RgbaImage::from_raw(w, h, data)
}

/// The current page's base url (set by set_page).
pub fn page_base() -> String {
    STORE.with(|s| s.borrow().base_url.clone())
}

/// Resolves a possibly-relative URL against a base.
pub fn resolve(base: &str, src: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return src.to_string();
    }
    url::Url::parse(base)
        .and_then(|b| b.join(src))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| src.to_string())
}
