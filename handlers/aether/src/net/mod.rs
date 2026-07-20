use anyhow::{Context, Result};
use reqwest::Client;

pub async fn fetch_document(url: &str) -> Result<String> {
    if url.starts_with("file://") {
        let path = url.trim_start_matches("file://");
        let content = std::fs::read_to_string(path).context("Failed to read local file")?;
        return Ok(content);
    }

    let client = Client::builder()
        .user_agent("UnaOS Aether/0.1.0")
        .build()?;
        
    let response = client.get(url).send().await.context("Failed to fetch URL")?;
    
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    
    let text = response.text().await.context("Failed to read response body")?;
    Ok(text)
}
