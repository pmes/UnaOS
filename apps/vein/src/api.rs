use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::env;
use std::time::Duration;
use log::{info, error};
use tokio::time::sleep;

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Content {
    pub role: String,
    pub parts: Vec<Part>,
}

// MODIFIED: Enum to support Text and Inline Data (Images)
#[derive(Serialize, Clone, Debug)]
#[serde(untagged)] // Critical: Serialize variants directly without wrapping
pub enum Part {
    Text { text: String },
    InlineData { inline_data: InlineData },
}

// Helper to construct variants easily
impl Part {
    pub fn text(t: String) -> Self {
        Part::Text { text: t }
    }

    pub fn image(mime_type: String, data: String) -> Self {
        Part::InlineData {
            inline_data: InlineData {
                mime_type,
                data,
            },
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct InlineData {
    pub mime_type: String,
    pub data: String, // Base64 encoded string
}

#[derive(Deserialize, Debug)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Deserialize, Debug)]
struct Candidate {
    content: ContentResponse,
    finish_reason: Option<String>,
    safety_ratings: Option<Vec<SafetyRating>>,
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
    block_reason: Option<String>,
    safety_ratings: Option<Vec<SafetyRating>>,
}

#[derive(Deserialize, Debug)]
struct SafetyRating {
    category: String,
    probability: String,
}

pub struct GeminiClient {
    client: Client,
    access_token: String,
    model_url: String,
}

impl GeminiClient {
    pub async fn new() -> Result<Self, String> {
        // Vertex AI requires Project ID and Location
        let project_id = env::var("GOOGLE_CLOUD_PROJECT_ID")
            .or_else(|_| env::var("PROJECT_ID"))
            .map_err(|_| "GOOGLE_CLOUD_PROJECT_ID (or PROJECT_ID) not set".to_string())?;

        let location = env::var("GOOGLE_CLOUD_REGION")
            .or_else(|_| env::var("REGION"))
            .unwrap_or_else(|_| "us-central1".to_string());

        let model_name = env::var("GEMINI_MODEL_NAME").unwrap_or_else(|_| "gemini-3-pro-preview".to_string());

        // Vertex AI Endpoint
        let model_url = format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
            location, project_id, location, model_name
        );

        // Authentication Setup - Bypass using gcloud CLI
        let token_command = std::process::Command::new("gcloud")
            .arg("auth")
            .arg("print-access-token")
            .output()
            .map_err(|e| format!("Failed to execute 'gcloud auth print-access-token': {}", e))?;

        if !token_command.status.success() {
            let stderr = String::from_utf8_lossy(&token_command.stderr);
            return Err(format!("'gcloud auth print-access-token' failed: {}", stderr));
        }
        let access_token = String::from_utf8_lossy(&token_command.stdout).trim().to_string();

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .connection_verbose(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            client,
            access_token,
            model_url,
        })
    }

    pub async fn generate_content(&self, history: &[Content]) -> Result<String, String> {
        const MAX_RETRIES: u32 = 3;
        let mut attempt = 0;

        loop {
            attempt += 1;
            info!("Sending request to Vertex AI (Attempt {}/{}) using model: {}", attempt, MAX_RETRIES, self.model_url);

            let request_body = GenerateContentRequest {
                contents: history.to_vec(),
            };

            let response_result = self.client.post(&self.model_url)
                .bearer_auth(&self.access_token)
                .json(&request_body)
                .send()
                .await;

            let response = match response_result {
                Ok(r) => r,
                Err(e) => {
                    error!("HTTP Request failed: {}", e);
                    if attempt >= MAX_RETRIES {
                        return Err(format!("API Request failed after {} retries: {}", MAX_RETRIES, e));
                    }
                    sleep(Duration::from_secs(2 * attempt as u64)).await;
                    continue;
                }
            };

            let status = response.status();

            if status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 503 {
                let text = response.text().await.unwrap_or_default();
                error!("API Retryable Error {}: {}", status, text);

                if attempt >= MAX_RETRIES {
                    return Err(format!("API Error {} Not Retried: {}", status, text));
                }

                sleep(Duration::from_secs(5 * attempt as u64)).await;
                continue;
            }

            if status.is_client_error() {
                let text = response.text().await.unwrap_or_default();
                error!("API Unrecoverable Error {}: {}", status, text);
                return Err(format!("API Error {}: {}", status, text));
            }

            let response_data: GenerateContentResponse = match response.json().await {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to parse JSON response: {}", e);
                    return Err(format!("Failed to parse JSON: {}", e));
                }
            };

            if let Some(feedback) = response_data.prompt_feedback {
                if let Some(reason) = feedback.block_reason {
                    return Err(format!("Prompt Blocked by API: {}", reason));
                }
            }

            if let Some(candidates) = response_data.candidates {
                if let Some(first_candidate) = candidates.first() {
                    if let Some(first_part) = first_candidate.content.parts.first() {
                        return Ok(first_part.text.clone());
                    }
                }
            }

            return Err("[SIGNAL LOST] - Unexpected response format.".to_string());
        }
    }
}
