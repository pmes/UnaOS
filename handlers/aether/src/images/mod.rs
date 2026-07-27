use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Decoded images for the current page, keyed by ABSOLUTE url.
/// Thread-local like js::DOM_STATE — the engine, layout, and renderer all
/// run on one thread; callers populate it via `set_page` before load.
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

/// Decodes the payload of a data: URI (base64 raster formats only).
fn decode_data_uri(rest: &str) -> Option<image::RgbaImage> {
    use base64::Engine as _;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.contains("base64") || meta.contains("svg") {
        crate::ledger::record_dom("img-data-uri-unsupported");
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()?;
    match image::load_from_memory(&bytes) {
        Ok(img) => Some(img.to_rgba8()),
        Err(_) => {
            crate::ledger::record_dom("img-data-uri-decode-failed");
            None
        }
    }
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
