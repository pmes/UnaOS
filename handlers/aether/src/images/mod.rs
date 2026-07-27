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
