use anyhow::{Context, Result};
use reqwest::Client;

/// One shared HTTP client: connection pooling + TLS session reuse across
/// every fetch of a page load (a per-fetch Client paid a fresh handshake
/// for each of yahoo's dozens of resources).
fn client() -> &'static Client {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent("UnaOS Aether/0.1.0")
            .build()
            .expect("http client")
    })
}

pub fn normalize_url(input: &str) -> Vec<String> {
    if input.starts_with("file://") {
        return vec![];
    }
    if !input.contains("://") {
        vec![format!("https://{}", input), format!("http://{}", input)]
    } else {
        vec![input.to_string()]
    }
}

pub async fn fetch_document(input: &str) -> Result<String> {
    if input.starts_with("file://") {
        anyhow::bail!("file:// fetches are disabled for security reasons.");
    }
    let urls = normalize_url(input);
    if urls.is_empty() {
        anyhow::bail!("Invalid URL");
    }

    let client = client();
    let mut last_err = None;
    for url in urls {
        match client.get(&url).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    return Err(anyhow::anyhow!("HTTP Error: {}", response.status()));
                }
                
                if response.content_length().unwrap_or(0) > 10 * 1024 * 1024 {
                    anyhow::bail!("Response exceeds size limit");
                }
                
                let bytes = response.bytes().await.context("Failed to read body")?;
                if bytes.len() > 10 * 1024 * 1024 {
                    anyhow::bail!("Response body exceeds size limit");
                }
                
                let text = String::from_utf8(bytes.to_vec()).context("Invalid UTF-8")?;
                return Ok(text);
            }
            Err(e) => {
                last_err = Some(anyhow::anyhow!(e).context("Connection failure"));
            }
        }
    }
    
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Failed to fetch URL")))
}

/// POSTs a urlencoded form body and returns the response document.
pub async fn post_document(url: &str, body: &str) -> Result<String> {
    let response = client()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    String::from_utf8(bytes.to_vec()).context("Invalid UTF-8")
}

/// Collects absolute URLs of `<link rel="stylesheet">` sheets in `html`,
/// resolved against `base_url`. Pure (no network) — unit-testable offline.
pub fn collect_stylesheet_urls(base_url: &str, html: &str) -> Vec<String> {
    let document = crate::dom::parse_html(html);
    let base = url::Url::parse(base_url).ok();
    let mut out = Vec::new();
    if let Ok(links) = document.select("link") {
        for link in links {
            let attrs = link.attributes.borrow();
            let is_sheet = attrs
                .get("rel")
                .map(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet")))
                .unwrap_or(false);
            if !is_sheet {
                continue;
            }
            let Some(href) = attrs.get("href") else { continue };
            let resolved = match &base {
                Some(b) => b.join(href).map(|u| u.to_string()).ok(),
                None => Some(href.to_string()),
            };
            if let Some(u) = resolved {
                if u.starts_with("http://") || u.starts_with("https://") {
                    out.push(u);
                }
            }
        }
    }
    out
}

/// Everything a page load needs, fetched up front (fetch-then-apply).
#[derive(Default)]
pub struct Page {
    pub base_url: String,
    pub html: String,
    pub sheets: Vec<String>,
    pub images: Vec<(String, image::RgbaImage)>,
}

/// Collects absolute `<img src>` URLs (capped; data:/svg skipped for now).
pub fn collect_image_urls(base_url: &str, html: &str) -> Vec<String> {
    let document = crate::dom::parse_html(html);
    let mut out = Vec::new();
    if let Ok(imgs) = document.select("img") {
        for img in imgs {
            let attrs = img.attributes.borrow();
            let Some(src) = attrs.get("src") else { continue };
            if src.starts_with("data:") {
                continue; // decoded synchronously at load, no fetch needed
            }
            let abs = crate::images::resolve(base_url, src);
            if (abs.starts_with("http://") || abs.starts_with("https://")) && !out.contains(&abs) {
                out.push(abs);
            }
            if out.len() >= 30 {
                crate::ledger::record_dom("img-fetch-cap-reached");
                break;
            }
        }
    }
    out
}

/// Collects absolute raster `url(...)` references from CSS text (external
/// sheets, <style> blocks, style="" attrs — callers pass raw text; the scan
/// is textual so it needs no CSS object model). Capped like images.
pub fn collect_css_image_urls(base_url: &str, texts: &[&str], out: &mut Vec<String>) {
    let raster = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"];
    for text in texts {
        let mut rest: &str = text;
        while let Some(pos) = rest.find("url(") {
            rest = &rest[pos + 4..];
            let Some(end) = rest.find(')') else { break };
            let inner = rest[..end].trim().trim_matches(|c| c == '"' || c == '\'').trim();
            rest = &rest[end..];
            let path_end = inner.find(['?', '#']).unwrap_or(inner.len());
            let is_raster = raster.iter().any(|ext| inner[..path_end].to_ascii_lowercase().ends_with(ext));
            if !is_raster {
                continue;
            }
            let abs = crate::images::resolve(base_url, inner);
            if (abs.starts_with("http://") || abs.starts_with("https://")) && !out.contains(&abs) {
                out.push(abs);
            }
            if out.len() >= 50 {
                crate::ledger::record_css("css-image-fetch-cap-reached");
                return;
            }
        }
    }
}

/// Fetches raw bytes (images etc), same limits as fetch_document.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let response = client().get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    Ok(bytes.to_vec())
}

/// Fetches a page, its external stylesheets, and its images: the async
/// half of the fetch-then-apply pattern. Resource failures are skipped
/// (and ledgered), never fatal to the page load.
pub async fn fetch_page(input: &str) -> Result<Page> {
    let html = fetch_document(input).await?;
    let base = normalize_url(input)
        .into_iter()
        .next()
        .unwrap_or_else(|| input.to_string());
    // All stylesheets fetch CONCURRENTLY (a page with a dozen sheets paid
    // a dozen round-trips in series before this).
    let sheet_results = futures_util::future::join_all(
        collect_stylesheet_urls(&base, &html)
            .into_iter()
            .map(|u| async move { (fetch_document(&u).await, u) }),
    )
    .await;
    let mut sheets = Vec::new();
    for (result, sheet_url) in sheet_results {
        match result {
            Ok(css) => sheets.push(css),
            Err(_) => crate::ledger::record_css(&format!("stylesheet-fetch-failed:{}", sheet_url)),
        }
    }
    // <img> srcs plus CSS background images (scanned from the html — which
    // carries <style> blocks and style="" attrs — and every fetched sheet).
    // CSS urls resolve against the page base; a sheet-relative path on
    // another host will miss and ledger as img-missing (honest gap).
    let mut wanted = collect_image_urls(&base, &html);
    let mut css_texts: Vec<&str> = vec![&html];
    css_texts.extend(sheets.iter().map(|s| s.as_str()));
    collect_css_image_urls(&base, &css_texts, &mut wanted);
    // Images likewise fetch concurrently; decode stays on this thread.
    let img_results = futures_util::future::join_all(
        wanted
            .into_iter()
            .map(|u| async move { (fetch_bytes(&u).await, u) }),
    )
    .await;
    let mut images = Vec::new();
    for (result, img_url) in img_results {
        match result {
            Ok(bytes) => {
                let decoded = image::load_from_memory(&bytes)
                    .ok()
                    .map(|i| i.to_rgba8())
                    .or_else(|| crate::images::decode_svg(&bytes));
                match decoded {
                    Some(img) => images.push((img_url, img)),
                    None => crate::ledger::record_dom(&format!("img-decode-failed:{}", &img_url[..img_url.len().min(48)])),
                }
            }
            Err(_) => crate::ledger::record_dom(&format!("img-fetch-failed:{}", &img_url[..img_url.len().min(48)])),
        }
    }
    Ok(Page { base_url: base, html, sheets, images })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_stylesheet_urls() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
            <link rel="stylesheet" href="https://cdn.example.org/x.css">
            <link rel="icon" href="/favicon.ico">
            <link href="/no-rel.css">
        </head><body></body></html>"#;
        let urls = collect_stylesheet_urls("https://example.com/page/", html);
        assert_eq!(urls, vec![
            "https://example.com/main.css".to_string(),
            "https://cdn.example.org/x.css".to_string(),
        ]);
    }

    #[test]
    fn test_normalize_url() {
        assert_eq!(normalize_url("google.com"), vec!["https://google.com", "http://google.com"]);
        assert_eq!(normalize_url("https://example.com"), vec!["https://example.com"]);
        assert_eq!(normalize_url("http://localhost"), vec!["http://localhost"]);
        assert!(normalize_url("file:///etc/passwd").is_empty());
    }
}
