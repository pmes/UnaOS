use anyhow::{Context, Result};
use reqwest::Client;

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

    let client = Client::builder()
        .user_agent("UnaOS Aether/0.1.0")
        .build()?;
        
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

/// Fetches a page AND its external stylesheets: the async half of the
/// fetch-then-apply pattern. Sheet failures are skipped (and ledgered),
/// never fatal to the page load.
pub async fn fetch_page(input: &str) -> Result<(String, Vec<String>)> {
    let html = fetch_document(input).await?;
    let base = normalize_url(input)
        .into_iter()
        .next()
        .unwrap_or_else(|| input.to_string());
    let mut sheets = Vec::new();
    for sheet_url in collect_stylesheet_urls(&base, &html) {
        match fetch_document(&sheet_url).await {
            Ok(css) => sheets.push(css),
            Err(_) => crate::ledger::record_css(&format!("stylesheet-fetch-failed:{}", sheet_url)),
        }
    }
    Ok((html, sheets))
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
