use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::env;
use std::time::Duration;
use log::{info, error};

// --- Data Structures ---

#[derive(Serialize)]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    pub generationConfig: GenerationConfig,
}

#[derive(Serialize)]
pub struct GenerationConfig {
    pub temperature: f32,
    pub maxOutputTokens: i32,
}

#[derive(Serialize, Clone, Debug)]
pub struct Content {
    pub role: String,
    pub parts: Vec<Part>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum Part {
    Text { text: String },
    FileData { file_data: FileData },
}

impl Part {
    pub fn text(t: String) -> Self {
        Part::Text { text: t }
    }
    pub fn file_data(mime_type: String, file_uri: String) -> Self {
        Part::FileData {
            file_data: FileData { mime_type, file_uri },
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct FileData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}

#[derive(Deserialize, Debug)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<Candidate>>,
    pub promptFeedback: Option<PromptFeedback>,
}

#[derive(Deserialize, Debug)]
pub struct Candidate {
    pub content: Option<ContentResponse>,
    pub finishReason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ContentResponse {
    pub parts: Vec<PartResponse>,
}

#[derive(Deserialize, Debug)]
pub struct PartResponse {
    pub text: String,
}

#[derive(Deserialize, Debug)]
pub struct PromptFeedback {
    pub blockReason: Option<String>,
}

// --- The Client ---

pub struct GeminiClient {
    client: Client,
    api_key: String,
    model_url: String,
}

impl GeminiClient {
    pub async fn new() -> Result<Self, String> {
        let api_key = env::var("GEMINI_API_KEY")
            .map_err(|_| "GEMINI_API_KEY not set in .env".to_string())?;

        // Hardcoded to Experimental as per original spec
        let model_name = "gemini-3-pro-preview";
        let model_url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_name, api_key
        );

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("Client Build Failed: {}", e))?;

        info!("System Ignited. Target: {}", model_name);

        Ok(Self { client, api_key, model_url })
    }

    pub async fn generate_content(&self, history: &[Content]) -> Result<String, String> {
        let request_body = GenerateContentRequest {
            contents: history.to_vec(),
            generationConfig: GenerationConfig {
                temperature: 0.9,
                maxOutputTokens: 8192,
            },
        };

        let response = self.client.post(&self.model_url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Transmission Failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("Hull Breach (API Error): {} - {}", status, text);
            return Err(format!("System Failure {}: {}", status, text));
        }

        let data: GenerateContentResponse = response.json().await
            .map_err(|e| format!("Decode Failed: {}", e))?;

        if let Some(feedback) = data.promptFeedback {
            if let Some(reason) = feedback.blockReason {
                return Err(format!("Safety Protocols Engaged: {}", reason));
            }
        }

        if let Some(candidates) = data.candidates {
            if let Some(first) = candidates.first() {
                if let Some(content) = &first.content {
                    let full_text: String = content.parts.iter().map(|p| p.text.as_str()).collect();
                    return Ok(full_text);
                }
            }
        }

        Err("Neural Core returned silence.".to_string())
    }
}
