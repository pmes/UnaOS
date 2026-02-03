use serde::{Deserialize, Serialize};
use reqwest::Client;
use std::env;

// --- API Types ---

#[derive(Serialize, Deserialize, Debug)]
pub struct Part {
    pub text: Option<String>,
}

impl Part {
    pub fn text(t: String) -> Self {
        Part { text: Some(t) }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Content {
    pub role: String,
    pub parts: Vec<Part>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<Candidate>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Candidate {
    pub content: Content,
    pub finish_reason: Option<String>,
}

// --- The Client ---

pub struct GeminiClient {
    api_key: String,
    base_url: String,
    model: String,
}

impl GeminiClient {
    pub fn new() -> Self {
        let api_key = env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");

        let model = "gemini-3-pro-preview".to_string();

        Self {
            api_key,
            base_url: "https://generativelanguage.googleapis.com/v1beta/models".to_string(),
            model,
        }
    }

    pub async fn generate_content(&self, history: &Vec<Content>) -> Result<String, String> {
        let client = Client::new();
        let url = format!("{}/{}:generateContent?key={}", self.base_url, self.model, self.api_key);

        let request_body = GenerateContentRequest {
            contents: history.iter().map(|c| Content {
                role: c.role.clone(),
                parts: c.parts.iter().map(|p| Part {
                    text: p.text.clone(),
                }).collect(),
            }).collect(),
        };

        let res = client.post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Network Error: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(format!("API Error {}: {}", status, text));
        }

        let response_body: GenerateContentResponse = res.json()
            .await
            .map_err(|e| format!("Parse Error: {}", e))?;

        if let Some(candidates) = response_body.candidates {
            if let Some(first) = candidates.first() {
                if let Some(part) = first.content.parts.first() {
                    if let Some(text) = &part.text {
                        return Ok(text.clone());
                    }
                }
            }
        }

        Err("No content returned".to_string())
    }
}
