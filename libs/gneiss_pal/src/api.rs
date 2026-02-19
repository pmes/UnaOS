use log::{error, info};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::process::Command;

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig, // Added for control
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
    Text {
        text: String,
    },
    FileData {
        #[serde(rename = "fileData")]
        file_data: FileData,
    },
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
    #[serde(rename = "promptFeedback")]
    prompt_feedback: Option<PromptFeedback>, // Note camelCase
}

#[derive(Deserialize, Debug)]
struct Candidate {
    content: Option<ContentResponse>,
    #[serde(rename = "finishReason")]
    _finish_reason: Option<String>,
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
    #[serde(rename = "blockReason")]
    block_reason: Option<String>,
}

pub struct GeminiClient {
    client: Client,
    model_url: String,
    token: String, // Added to store the active gcloud token
}

impl GeminiClient {
    pub async fn new() -> Result<Self, String> {
        // 1. Simple Auth broken. Fetch Bearer Token dynamically via gcloud ADC
        let output = Command::new("gcloud")
            .args(["auth", "application-default", "print-access-token"])
            .output()
            .map_err(|e| format!("Failed to execute gcloud for token: {}", e))?;

        if !output.status.success() {
            return Err("Failed to retrieve gcloud access token. Ensure gcloud ADC is configured.".to_string());
        }

        let token = String::from_utf8(output.stdout)
            .map_err(|_| "Invalid UTF-8 in gcloud token".to_string())?
            .trim()
            .to_string();

        // 2. Hardcode to Experimental as requested
        let model_name = "gemini-3-pro-preview";

        // 3. Pure Vertex URL (No API key appended)
        let model_url = format!(
            "https://aiplatform.googleapis.com/v1/projects/unauploads-1769528906/locations/global/publishers/google/models/{}:generateContent",
            model_name
        );

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        info!("System Ignited. Target: {} (Vertex API)", model_name);

        Ok(Self { client, model_url, token })
    }

    pub async fn generate_content(&self, history: &[Content]) -> Result<String, String> {
        let request_body = GenerateContentRequest {
            contents: history.to_vec(),
            generation_config: GenerationConfig {
                temperature: 0.4, // shoot down the middle
            },
        };

        info!("Transmitting to Neural Core...");

        let response = self
            .client
            .post(&self.model_url)
            .bearer_auth(&self.token) // Injects the exact header Vertex demands
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

        if let Some(feedback) = data.prompt_feedback {
            if let Some(reason) = feedback.block_reason {
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

    pub async fn list_vertex_models(&self) -> Result<String, String> {
        Ok("Model listing bypass engaged. Hardcoded to gemini-3-pro-preview.".to_string())
    }
}
