use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum YtError {
    #[error("Video is unavailable: {0}")]
    Unavailable(String),
    #[error("Video is age-gated")]
    AgeGated,
    #[error("Video is a live stream")]
    Live,
    #[error("Video is region-locked")]
    RegionLocked,
    #[error("Stream is ciphered-only (requires signature descrambling)")]
    CipheredOnly,
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub title: String,
    pub author: String,
    pub duration_secs: u64,
    pub formats: Vec<ResolvedFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFormat {
    pub url: String,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Resolved {
    pub fn best_progressive(&self) -> Option<&ResolvedFormat> {
        self.formats.iter().max_by_key(|f| f.height.unwrap_or(0))
    }
}

#[derive(Deserialize)]
struct InnerTubeResponse {
    #[serde(rename = "playabilityStatus")]
    playability_status: Option<PlayabilityStatus>,
    #[serde(rename = "videoDetails")]
    video_details: Option<VideoDetails>,
    #[serde(rename = "streamingData")]
    streaming_data: Option<StreamingData>,
}

#[derive(Deserialize)]
struct PlayabilityStatus {
    status: Option<String>,
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetails {
    title: Option<String>,
    author: Option<String>,
    length_seconds: Option<String>,
    is_live: Option<bool>,
    is_live_content: Option<bool>,
}

#[derive(Deserialize)]
struct StreamingData {
    formats: Option<Vec<Format>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Format {
    url: Option<String>,
    signature_cipher: Option<String>,
    mime_type: String,
    width: Option<u32>,
    height: Option<u32>,
}

pub fn extract_video_id(url_or_id: &str) -> Option<String> {
    if url_or_id.len() == 11 && !url_or_id.contains('/') {
        return Some(url_or_id.to_string());
    }

    if let Some(idx) = url_or_id.find("v=") {
        let end = url_or_id[idx + 2..]
            .find('&')
            .map(|i| idx + 2 + i)
            .unwrap_or(url_or_id.len());
        let id = &url_or_id[idx + 2..end];
        if id.len() == 11 {
            return Some(id.to_string());
        }
    }

    if let Some(idx) = url_or_id.find("youtu.be/") {
        let start = idx + 9;
        let end = url_or_id[start..]
            .find('?')
            .map(|i| start + i)
            .unwrap_or(url_or_id.len());
        let id = &url_or_id[start..end];
        if id.len() == 11 {
            return Some(id.to_string());
        }
    }

    if let Some(idx) = url_or_id.find("/shorts/") {
        let start = idx + 8;
        let end = url_or_id[start..]
            .find('?')
            .map(|i| start + i)
            .unwrap_or(url_or_id.len());
        let id = &url_or_id[start..end];
        if id.len() == 11 {
            return Some(id.to_string());
        }
    }

    None
}

pub fn parse_response(json: &str) -> Result<Resolved, YtError> {
    let resp: InnerTubeResponse = serde_json::from_str(json)
        .map_err(|e| YtError::Parse(e.to_string()))?;

    if let Some(status) = resp.playability_status {
        match status.status.as_deref() {
            Some("OK") => {},
            Some("LOGIN_REQUIRED") => return Err(YtError::AgeGated),
            Some("UNPLAYABLE") => return Err(YtError::Unavailable(status.reason.unwrap_or_default())),
            Some("ERROR") => return Err(YtError::RegionLocked),
            _ => return Err(YtError::Unavailable("Unknown status".into())),
        }
    }

    let details = resp.video_details.ok_or_else(|| YtError::Parse("Missing videoDetails".into()))?;
    if details.is_live.unwrap_or(false) || details.is_live_content.unwrap_or(false) {
        return Err(YtError::Live);
    }

    let streaming_data = resp.streaming_data.ok_or_else(|| YtError::Parse("Missing streamingData".into()))?;
    let formats = streaming_data.formats.unwrap_or_default();

    if formats.is_empty() {
        return Err(YtError::Parse("No formats found".into()));
    }

    let mut resolved_formats = Vec::new();
    let mut has_ciphered = false;
    
    for f in formats {
        if f.signature_cipher.is_some() {
            has_ciphered = true;
            continue;
        }
        if let Some(url) = f.url {
            resolved_formats.push(ResolvedFormat {
                url,
                mime_type: f.mime_type,
                width: f.width,
                height: f.height,
            });
        }
    }

    if resolved_formats.is_empty() && has_ciphered {
        return Err(YtError::CipheredOnly);
    }

    Ok(Resolved {
        title: details.title.unwrap_or_default(),
        author: details.author.unwrap_or_default(),
        duration_secs: details.length_seconds.unwrap_or_else(|| "0".to_string()).parse().unwrap_or(0),
        formats: resolved_formats,
    })
}

pub async fn resolve(video_id_or_url: &str) -> Result<Resolved, YtError> {
    let video_id = extract_video_id(video_id_or_url)
        .ok_or_else(|| YtError::Parse("Invalid video ID or URL".into()))?;

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    // Client contexts to try in order: newer ANDROID first, IOS fallback.
    // The innertube contract drifts; each candidate is a known-working
    // (clientName, clientVersion, extra) tuple as of 2026-07.
    let candidates = [
        serde_json::json!({
            "clientName": "ANDROID",
            "clientVersion": "19.09.37",
            "androidSdkVersion": 30,
            "userAgent": "com.google.android.youtube/19.09.37 (Linux; U; Android 11) gzip"
        }),
        serde_json::json!({
            "clientName": "IOS",
            "clientVersion": "19.09.3",
            "deviceModel": "iPhone14,3",
            "userAgent": "com.google.ios.youtube/19.09.3 (iPhone14,3; U; CPU iOS 15_6 like Mac OS X)"
        }),
    ];

    let mut last_err = YtError::Parse("no client candidates".into());
    for client_ctx in &candidates {
        let body = serde_json::json!({
            "context": { "client": client_ctx },
            "videoId": video_id
        });
        let resp = client
            .post("https://www.youtube.com/youtubei/v1/player")
            .json(&body)
            .send()
            .await?
            .text()
            .await?;
        match parse_response(&resp) {
            Ok(resolved) => return Ok(resolved),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_video_id() {
        assert_eq!(extract_video_id("dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_video_id("https://youtu.be/dQw4w9WgXcQ?t=10").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(extract_video_id("invalid_id"), None);
    }

    #[test]
    fn test_parse_success() {
        let json = r#"{
            "playabilityStatus": { "status": "OK" },
            "videoDetails": {
                "title": "Test Video",
                "author": "Test Author",
                "lengthSeconds": "120"
            },
            "streamingData": {
                "formats": [
                    { "url": "https://test.com/v.mp4", "mimeType": "video/mp4", "width": 1920, "height": 1080 }
                ]
            }
        }"#;
        let res = parse_response(json).unwrap();
        assert_eq!(res.title, "Test Video");
        assert_eq!(res.duration_secs, 120);
        assert_eq!(res.formats.len(), 1);
        assert_eq!(res.best_progressive().unwrap().height, Some(1080));
    }

    #[test]
    fn test_parse_live() {
        let json = r#"{
            "playabilityStatus": { "status": "OK" },
            "videoDetails": {
                "title": "Live Stream",
                "author": "Test Author",
                "lengthSeconds": "0",
                "isLive": true
            }
        }"#;
        let err = parse_response(json).unwrap_err();
        assert!(matches!(err, YtError::Live));
    }

    #[test]
    fn test_parse_age_gated() {
        let json = r#"{
            "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "Sign in to confirm your age." }
        }"#;
        let err = parse_response(json).unwrap_err();
        assert!(matches!(err, YtError::AgeGated));
    }

    #[test]
    fn test_parse_unavailable() {
        let json = r#"{
            "playabilityStatus": { "status": "UNPLAYABLE", "reason": "Video unavailable" }
        }"#;
        let err = parse_response(json).unwrap_err();
        if let YtError::Unavailable(msg) = err {
            assert_eq!(msg, "Video unavailable");
        } else {
            panic!("Expected Unavailable, got {:?}", err);
        }
    }

    #[test]
    fn test_parse_region_locked() {
        let json = r#"{
            "playabilityStatus": { "status": "ERROR", "reason": "The uploader has not made this video available in your country" }
        }"#;
        let err = parse_response(json).unwrap_err();
        assert!(matches!(err, YtError::RegionLocked), "Expected RegionLocked, got {:?}", err);
    }

    #[test]
    fn test_parse_ciphered_only() {
        let json = r#"{
            "playabilityStatus": { "status": "OK" },
            "videoDetails": {
                "title": "Ciphered",
                "author": "Test Author",
                "lengthSeconds": "120"
            },
            "streamingData": {
                "formats": [
                    { "signatureCipher": "s=...", "mimeType": "video/mp4", "width": 1920, "height": 1080 }
                ]
            }
        }"#;
        let err = parse_response(json).unwrap_err();
        assert!(matches!(err, YtError::CipheredOnly));
    }

    #[tokio::test]
    async fn test_live_resolve() {
        if std::env::var("AETHER_YT_LIVE").unwrap_or_default() != "1" {
            return;
        }
        let res = resolve("dQw4w9WgXcQ").await.expect("Failed to resolve real video");
        assert!(res.duration_secs > 0);
        assert!(!res.formats.is_empty());
        assert!(res.formats[0].url.starts_with("https://"));
    }
}
