use anyhow::{Context, Result};
use reqwest::Client;

pub async fn fetch_document(url: &str) -> Result<String> {
    if url.starts_with("file://") {
        anyhow::bail!("file:// fetches are disabled for security reasons.");
    }

    let client = Client::builder()
        .user_agent("UnaOS Aether/0.1.0")
        .build()?;
        
    let response = client.get(url).send().await.context("Failed to fetch URL")?;
    
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    
    if response.content_length().unwrap_or(0) > 10 * 1024 * 1024 {
        anyhow::bail!("Response exceeds size limit");
    }
    
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    
    let text = String::from_utf8(bytes.to_vec()).context("Invalid UTF-8")?;
    Ok(text)
}
