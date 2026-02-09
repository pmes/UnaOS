use log::{error, info};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::env;
use std::time::Duration;

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
    generationConfig: GenerationConfig, // Added for control
}

#[derive(Serialize)]
struct GenerationConfig {
    temperature: f32, // The Slider
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
            file_data: FileData {
                mime_type,
                file_uri,
            },
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

// Standard Google AI Response (Not Vertex Stream)
#[derive(Deserialize, Debug)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    promptFeedback: Option<PromptFeedback>, // Note camelCase
}

#[derive(Deserialize, Debug)]
struct Candidate {
    content: Option<ContentResponse>,
    finishReason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ContentResponse {
    parts: Vec<PartResponse>,
}

#[derive(Deserialize, Debug)]
struct PartResponse {
    text: String,
}

#[derive(Deserialize, Debug)]
struct PromptFeedback {
    blockReason: Option<String>,
}

pub struct GeminiClient {
    client: Client,
    api_key: String,
    model_url: String,
}

impl GeminiClient {
    pub async fn new() -> Result<Self, String> {
        // 1. Get the Key (Simpler Auth)
        let api_key =
            env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set in .env".to_string())?;

        // 2. Hardcode to Experimental as requested
        let model_name = "gemini-3-pro-preview";

        // 3. Use the Developer API URL (Not Vertex)
        // using generateContent (Buffered) to avoid timeout/stream issues for now
        let model_url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_name, api_key
        );

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        info!("System Ignited. Target: {} (Developer API)", model_name);

        Ok(Self {
            client,
            api_key, // Kept in struct if we need it later, though it's in the URL
            model_url,
        })
    }

    pub async fn generate_content(&self, history: &[Content]) -> Result<String, String> {
        // NO RETRY LOOP. Raw connection.

        let request_body = GenerateContentRequest {
            contents: history.to_vec(),
            generationConfig: GenerationConfig {
                temperature: 0.9, // High creativity
            },
        };

        info!("Transmitting to Neural Core...");

        let response = self
            .client
            .post(&self.model_url)
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

        let data: GenerateContentResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to decode neural pattern: {}", e))?;

        if let Some(feedback) = data.promptFeedback {
            if let Some(reason) = feedback.blockReason {
                return Err(format!("Safety Protocols Engaged: {}", reason));
            }
        }

        if let Some(candidates) = data.candidates {
            if let Some(first) = candidates.first() {
                if let Some(content) = &first.content {
                    let mut full_text = String::new();
                    for part in &content.parts {
                        full_text.push_str(&part.text);
                    }
                    return Ok(full_text);
                }
            }
        }

        Err("Neural Core returned silence (Empty Response).".to_string())
    }

    // Stubbed out because we aren't using Vertex Token anymore
    pub async fn list_vertex_models(&self) -> Result<String, String> {
        Ok("Model listing unavailable in Experimental Key Mode.".to_string())
    }
}
