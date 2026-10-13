use html_core::url;
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
        decode_raster(&bytes)
    };
    if decoded.is_none() {
        crate::ledger::record_dom("img-data-uri-decode-failed");
    }
    decoded
}

/// Decodes raster image bytes to straight RGBA.
///
/// PIXELCORE (SR25): PNG (an APNG: its first composited frame), JPEG, GIF (first frame), BMP, QOI and
/// WebP — lossless, lossy (VP8 + ALPH) through VP8CORE (SR40), and animated (its first composited frame,
/// ANIMWEBP SR44) — go through UnaOS's own decoders — `gneiss_pal::dsp::image`, i.e.
/// `pixel_core`, the code the kernel's Facet runs too — with the EXIF orientation applied, as a browser
/// applies it (`image-orientation: from-image`). WebP NEVER falls back (SR40 removed that chicken
/// wire): a WebP pixel_core refuses (malformed) is a ledgered miss. The `image` crate stays behind
/// it ONLY for the other formats pixel_core refuses (ICO, TIFF, AVIF, …, or a file it calls
/// malformed), and only with the `image-fallback` feature (without it the `image` crate can still read
/// PNG — the codec its PNG writer brings along). Build without the `pixel-core` feature to put the
/// `image` crate back in front for everything (the swap is reversible).
pub fn decode_raster(bytes: &[u8]) -> Option<image::RgbaImage> {
    #[cfg(feature = "pixel-core")]
    {
        match gneiss_pal::dsp::image::decode(bytes) {
            Ok(mut img) => {
                img.apply_orientation();
                return image::RgbaImage::from_raw(img.width, img.height, img.rgba);
            }
            Err(e) if gneiss_pal::dsp::image::sniff(bytes) == Some(gneiss_pal::dsp::image::Format::WebP) => {
                crate::ledger::record_dom(&format!("img-webp-refused:{e}"));
                return None;
            }
            Err(_) => {}
        }
    }
    // AETHERMEDIA (SR107): AVIF through av1_core (AVCODEC, SR24) — never the image crate.
    if is_avif(bytes) {
        return match av1_core::image::decode_avif(bytes) {
            Ok(img) => image::RgbaImage::from_raw(img.w, img.h, img.rgba),
            Err(e) => {
                crate::ledger::record_dom(&format!("img-avif-refused:{e:?}"));
                None
            }
        };
    }
    image::load_from_memory(bytes).ok().map(|i| i.to_rgba8())
}

/// An ISOBMFF file whose `ftyp` major or compatible brands name AVIF (`avif`, `avis`).
pub fn is_avif(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let end = size.clamp(16, bytes.len());
    let brand = |b: &[u8]| b == b"avif" || b == b"avis";
    brand(&bytes[8..12]) || bytes[16..end].chunks_exact(4).any(brand)
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
///
/// Behind the `svg` feature (resvg); without it every SVG is a miss, ledgered by the caller.
#[cfg(not(feature = "svg"))]
pub fn decode_svg(_bytes: &[u8]) -> Option<image::RgbaImage> {
    None
}

/// Rasterizes SVG bytes (see the `svg`-less twin above).
#[cfg(feature = "svg")]
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

/// The viewport width source selection is made against. Fixed, so the fetch side
/// (`net::fetch_page`) and the paint side (layout, render) always pick the same candidate and the
/// store key matches; it is the width Aether's pages are fetched for.
pub const SELECTION_VIEWPORT: (f32, f32) = (800.0, 600.0);

/// One image candidate of a `srcset` (HTML §4.8.4.3.10): a url and its descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: String,
    /// `<n>w`
    pub width: Option<f32>,
    /// `<n>x` (1 when neither descriptor is given)
    pub density: Option<f32>,
}

/// HTML §4.8.4.3.10 "parse a srcset attribute": urls may contain commas (only a TRAILING comma
/// ends a url), descriptors are split on whitespace outside parentheses, and a candidate with an
/// invalid or duplicated descriptor is dropped.
pub fn parse_srcset(input: &str) -> Vec<Candidate> {
    let b: Vec<char> = input.chars().collect();
    let ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r');
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        // splitting loop: skip whitespace and commas
        while i < b.len() && (ws(b[i]) || b[i] == ',') {
            i += 1;
        }
        if i >= b.len() {
            return out;
        }
        let start = i;
        while i < b.len() && !ws(b[i]) {
            i += 1;
        }
        let mut url: String = b[start..i].iter().collect();
        let mut descriptors: Vec<String> = Vec::new();
        if url.ends_with(',') {
            // trailing commas end the url; no descriptors
            while url.ends_with(',') {
                url.pop();
            }
        } else {
            // the descriptor tokenizer
            while i < b.len() && ws(b[i]) {
                i += 1;
            }
            let mut cur = String::new();
            let mut in_parens = false;
            while i < b.len() {
                let c = b[i];
                i += 1;
                if in_parens {
                    cur.push(c);
                    if c == ')' {
                        in_parens = false;
                    }
                    continue;
                }
                if ws(c) {
                    if !cur.is_empty() {
                        descriptors.push(std::mem::take(&mut cur));
                    }
                } else if c == ',' {
                    break;
                } else {
                    if c == '(' {
                        in_parens = true;
                    }
                    cur.push(c);
                }
            }
            if !cur.is_empty() {
                descriptors.push(cur);
            }
        }
        if url.is_empty() {
            continue;
        }
        // descriptor parser
        let (mut width, mut density, mut height, mut error) = (None::<f32>, None::<f32>, None::<f32>, false);
        for d in &descriptors {
            let (num, unit) = d.split_at(d.len() - d.chars().last().map(char::len_utf8).unwrap_or(0));
            match unit {
                "w" if width.is_none() && density.is_none() => match num.parse::<u32>() {
                    Ok(n) if n > 0 && !num.starts_with('+') => width = Some(n as f32),
                    _ => error = true,
                },
                "x" if width.is_none() && density.is_none() && height.is_none() => match num.parse::<f32>() {
                    Ok(n) if n >= 0.0 && n.is_finite() && !num.starts_with('+') => density = Some(n),
                    _ => error = true,
                },
                "h" if height.is_none() && density.is_none() => match num.parse::<u32>() {
                    Ok(n) if n > 0 => height = Some(n as f32),
                    _ => error = true,
                },
                _ => error = true,
            }
        }
        if height.is_some() && width.is_none() {
            error = true;
        }
        if !error {
            if width.is_none() && density.is_none() {
                density = Some(1.0);
            }
            out.push(Candidate { url, width, density });
        }
    }
}

/// HTML §4.8.4.3.11 "parse a sizes attribute" → the source size in CSS px against the viewport
/// `vp` (width, height): the first entry whose media condition matches (or that has none), its
/// length in px / vw / vh / em / rem (16 px) / vmin / vmax; `100vw` when nothing applies.
pub fn source_size(sizes: Option<&str>, vp: (f32, f32)) -> f32 {
    let default = vp.0;
    let Some(sizes) = sizes else { return default };
    let length = |t: &str| -> Option<f32> {
        let t = t.trim().to_ascii_lowercase();
        let unit_at = t.find(|c: char| c.is_ascii_alphabetic())?;
        let (n, u) = t.split_at(unit_at);
        let n: f32 = n.trim().parse().ok()?;
        if n < 0.0 {
            return None;
        }
        Some(match u {
            "px" => n,
            "vw" => n * vp.0 / 100.0,
            "vh" => n * vp.1 / 100.0,
            "vmin" => n * vp.0.min(vp.1) / 100.0,
            "vmax" => n * vp.0.max(vp.1) / 100.0,
            "em" | "rem" => n * 16.0,
            _ => return None,
        })
    };
    for entry in sizes.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // the size is the last component value; what comes before it is the media condition
        let (cond, size) = match entry.rfind(|c: char| c.is_whitespace() || c == ')') {
            Some(at) => {
                let (c, sz) = entry.split_at(at + 1);
                (c.trim(), sz.trim())
            }
            None => ("", entry),
        };
        let Some(px) = length(size).or_else(|| (size == "0").then_some(0.0)) else { continue };
        if cond.is_empty() || media_condition_matches(cond, vp) {
            return px;
        }
    }
    default
}

/// A `media` attribute / `sizes` condition against the viewport (the cascade's environment).
fn media_condition_matches(q: &str, vp: (f32, f32)) -> bool {
    let env = crate::css::media_environment(vp.0, vp.1);
    let cvs = css_core::parser::parse_component_values(q);
    css_core::media::parse_media_query_list(&cvs).matches(&env)
}

/// HTML §4.8.4.3.6 "select an image source" over `candidates` for device pixel ratio `dpr`: width
/// descriptors become densities against `source_size`; the smallest density at or above the
/// ratio wins, else the densest below it. Ties keep the earlier candidate.
pub fn select_candidate(candidates: &[Candidate], source_size: f32, dpr: f32) -> Option<String> {
    let mut best: Option<(f32, &str)> = None;
    for c in candidates {
        let d = match (c.width, c.density) {
            (Some(w), _) => w / source_size.max(1.0),
            (None, Some(x)) => x,
            (None, None) => 1.0,
        };
        let better = match best {
            None => true,
            Some((bd, _)) => match (bd >= dpr, d >= dpr) {
                (true, true) => d < bd,
                (false, true) => true,
                (true, false) => false,
                (false, false) => d > bd,
            },
        };
        if better {
            best = Some((d, &c.url));
        }
    }
    best.map(|(_, u)| u.to_string())
}

/// Picks the srcset candidate for the selection viewport at 1x, `sizes` absent (`100vw`):
/// a width descriptor counts as `w / 800` x. The smallest candidate that covers the viewport
/// wins, else the largest available. Deterministic — the fetch side and the paint side both call
/// this, so the store key always matches.
pub fn pick_srcset(srcset: &str) -> Option<String> {
    select_candidate(&parse_srcset(srcset), source_size(None, SELECTION_VIEWPORT), 1.0)
}

/// The source set of one `<img>` or `<source>`: its `srcset` candidates, plus (for an `<img>`) its
/// `src` as a 1x candidate when the set has neither a 1x nor a width candidate (HTML §4.8.4.3.6).
fn source_set(srcset: Option<&str>, src: Option<&str>) -> Vec<Candidate> {
    let mut set = srcset.map(parse_srcset).unwrap_or_default();
    if let Some(src) = src.filter(|s| !s.trim().is_empty()) {
        let has_1x = set.iter().any(|c| c.density == Some(1.0));
        let has_w = set.iter().any(|c| c.width.is_some());
        if !has_1x && !has_w {
            set.push(Candidate { url: src.trim().to_string(), width: None, density: Some(1.0) });
        }
    }
    set
}

/// The effective source of an <img> from its own attributes: `srcset` (with `sizes`) and `src`,
/// selected for [`SELECTION_VIEWPORT`] at 1x.
pub fn effective_img_src(attrs: &crate::dom::Attributes) -> Option<String> {
    let set = source_set(attrs.get("srcset"), attrs.get("src"));
    if set.is_empty() {
        return None;
    }
    let size = source_size(attrs.get("sizes"), SELECTION_VIEWPORT);
    select_candidate(&set, size, 1.0)
}

/// Image MIME types `decode_raster` / `decode_svg` take, for a `<picture>`'s `<source type>`:
/// pixel_core's formats (PNG, JPEG, GIF, BMP, QOI, WebP), the `image` crate fallback's
/// (ICO, TIFF), SVG when the `svg` feature rasterises it, and AVIF through av1_core.
pub fn can_decode_type(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match base.as_str() {
        "image/png" | "image/apng" | "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/gif" | "image/bmp" | "image/x-bmp"
        | "image/webp" | "image/qoi" => true,
        "image/x-icon" | "image/vnd.microsoft.icon" | "image/tiff" => cfg!(feature = "image-fallback"),
        "image/svg+xml" => cfg!(feature = "svg"),
        "image/avif" => true,
        _ => false,
    }
}

/// The source of an `<img>` element node with its `<picture>` parent honoured (HTML §4.8.4.3.6
/// "update the source set"): the first `<source>` child of the picture BEFORE the img whose
/// `srcset` is non-empty, whose `media` matches the viewport and whose `type` (when given) is
/// decodable supplies the set (with its own `sizes`); else the img's own attributes do.
pub fn img_source(img: &crate::dom::NodeRef, vp: (f32, f32)) -> Option<String> {
    let el = img.as_element()?;
    if let Some(parent) = img.parent() {
        if parent.as_element().is_some_and(|p| p.name.local.as_ref() == "picture") {
            for child in parent.children() {
                if &child == img {
                    break;
                }
                let Some(c) = child.as_element() else { continue };
                if c.name.local.as_ref() != "source" {
                    continue;
                }
                let a = c.attributes.borrow();
                let Some(srcset) = a.get("srcset").filter(|s| !s.trim().is_empty()) else { continue };
                if let Some(m) = a.get("media").filter(|m| !m.trim().is_empty()) {
                    if !media_condition_matches(m, vp) {
                        continue;
                    }
                }
                if let Some(t) = a.get("type").filter(|t| !t.trim().is_empty()) {
                    if !can_decode_type(t) {
                        continue;
                    }
                }
                let set = source_set(Some(srcset), None);
                if set.is_empty() {
                    continue;
                }
                return select_candidate(&set, source_size(a.get("sizes"), vp), 1.0);
            }
        }
    }
    let a = el.attributes.borrow();
    let set = source_set(a.get("srcset"), a.get("src"));
    if set.is_empty() {
        return None;
    }
    select_candidate(&set, source_size(a.get("sizes"), vp), 1.0)
}

/// `loading=lazy` on an `<img>` (HTML §2.5.7 lazy loading attribute). The fetch side defers such an
/// image until its box nears the viewport; [`lazy_should_load`] is the "near" test.
pub fn is_lazy(attrs: &crate::dom::Attributes) -> bool {
    attrs.get("loading").is_some_and(|v| v.trim().eq_ignore_ascii_case("lazy"))
}

/// The lazy-load intersection test: a box (document y, height) loads once it is within the
/// viewport extended by Chromium's 1250 px margin (its threshold on a fast connection).
pub fn lazy_should_load(box_y: f32, box_h: f32, scroll_y: f32, viewport_h: f32) -> bool {
    const MARGIN: f32 = 1250.0;
    box_y + box_h >= scroll_y - MARGIN && box_y <= scroll_y + viewport_h + MARGIN
}

// ------------------------------------------------------------------ loading=lazy (AETHERMEDIA SR107)

/// Lazy images asked for (absolute urls) and the ones decoded off-thread, waiting for the engine.
static LAZY_ASKED: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);
static LAZY_DONE: std::sync::Mutex<Vec<(String, image::RgbaImage)>> = std::sync::Mutex::new(Vec::new());

/// True when `abs` is decoded in the current page's store.
pub fn has(abs: &str) -> bool {
    STORE.with(|s| s.borrow().images.contains_key(abs))
}

/// Fetch and decode one lazy image off the engine thread (once per url per process); the result
/// lands in the page store at the engine's next [`take_lazy_loaded`]. A refused, failed or
/// undecodable image is ledgered.
pub fn request_lazy(abs: &str) {
    let Some(url) = crate::security::subresource_url(abs, "image") else { return };
    {
        let mut asked = LAZY_ASKED.lock().unwrap();
        if !asked.get_or_insert_with(Default::default).insert(abs.to_string()) {
            return;
        }
    }
    let key = abs.to_string();
    std::thread::Builder::new()
        .name("aether-lazy-img".into())
        .spawn(move || {
            use std::io::Read;
            let fetched = (|| -> Result<Vec<u8>, String> {
                let client = crate::net::blocking_client_builder().build().map_err(|e| e.to_string())?;
                let mut resp = client.get(&url).send().map_err(|e| e.to_string())?;
                let status = resp.status().as_u16();
                if !(200..300).contains(&status) {
                    return Err(format!("HTTP {status}"));
                }
                let mut body = Vec::new();
                resp.read_to_end(&mut body).map_err(|e| e.to_string())?;
                Ok(body)
            })();
            match fetched {
                Ok(bytes) => match decode_raster(&bytes).or_else(|| decode_svg(&bytes)) {
                    Some(img) => LAZY_DONE.lock().unwrap().push((key, img)),
                    None => crate::ledger::record_dom(&format!("img-lazy-decode-failed:{}", &key[..key.len().min(48)])),
                },
                Err(e) => crate::ledger::record_dom(&format!("img-lazy-fetch-failed:{}: {e}", &key[..key.len().min(48)])),
            }
        })
        .ok();
}

/// Install the lazy images decoded since the last call into this page's store; returns how many
/// (the engine relayouts when any arrived).
pub fn take_lazy_loaded() -> usize {
    let done: Vec<(String, image::RgbaImage)> = std::mem::take(&mut *LAZY_DONE.lock().unwrap());
    let n = done.len();
    if n > 0 {
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            for (k, img) in done {
                s.images.insert(k, Rc::new(img));
            }
        });
    }
    n
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
