use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
}

#[derive(Serialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Serialize)]
struct Part {
    text: String,
}

#[derive(Deserialize, Debug)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
}

#[derive(Deserialize, Debug)]
struct Candidate {
    content: ContentResponse,
}

#[derive(Deserialize, Debug)]
struct ContentResponse {
    parts: Vec<PartResponse>,
}

#[derive(Deserialize, Debug)]
struct PartResponse {
    text: String,
}

pub struct GeminiClient {
    client: Client,
    api_key: String,
}

impl GeminiClient {
    pub fn new() -> Result<Self, String> {
        let api_key = env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set".to_string())?;
        Ok(Self {
            client: Client::new(),
            api_key,
        })
    }

    pub async fn generate_content(&self, prompt: &str) -> Result<String, String> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent?key={}",
            self.api_key
        );

        let request_body = GenerateContentRequest {
            contents: vec![Content {
                parts: vec![Part {
                    text: prompt.to_string(),
                }],
            }],
        };

        let response = self.client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
             let status = response.status();
             let text = response.text().await.unwrap_or_default();
             return Err(format!("API Error {}: {}", status, text));
        }

        let response_data: GenerateContentResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        // Extract text safely
        if let Some(candidates) = response_data.candidates {
            if let Some(first_candidate) = candidates.first() {
                if let Some(first_part) = first_candidate.content.parts.first() {
                    return Ok(first_part.text.clone());
                }
            }
        }

        Err("[Signal Lost] - Unexpected response format".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let request = GenerateContentRequest {
            contents: vec![Content {
                parts: vec![Part {
                    text: "Hello".to_string(),
                }],
            }],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"contents":[{"parts":[{"text":"Hello"}]}]}"#);
    }

    #[test]
    fn test_response_deserialization() {
        let json = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "Hello Vein"
                            }
                        ]
                    }
                }
            ]
        }"#;
        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.candidates.unwrap()[0].content.parts[0].text, "Hello Vein");
    }
}
