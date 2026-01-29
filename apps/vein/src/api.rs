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

// Response structs for streamGenerateContent (returns an array of chunks usually)
// But wait, the standard stream API returns a stream of JSON objects.
// However, reqwest's `json()` reads the whole body.
// If the endpoint is `streamGenerateContent`, the response body is a stream of JSON objects like `[{...}, {...}]` or line-delimited?
// Google Vertex AI `streamGenerateContent` returns a JSON array `[...]`.
#[derive(Deserialize, Debug)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Deserialize, Debug)]
struct Candidate {
    content: Option<ContentResponse>, // Make optional as it might be missing in some chunks
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
        let project_id = env::var("GOOGLE_CLOUD_PROJECT_ID")
            .or_else(|_| env::var("PROJECT_ID"))
            .map_err(|_| "GOOGLE_CLOUD_PROJECT_ID (or PROJECT_ID) not set".to_string())?;

        let location = env::var("GOOGLE_CLOUD_REGION")
            .or_else(|_| env::var("REGION"))
            .unwrap_or_else(|_| "us-central1".to_string());

        let model_name = env::var("GEMINI_MODEL_NAME").unwrap_or_else(|_| "gemini-3-pro-preview".to_string());

        // Use streamGenerateContent on global endpoint
        let model_url = format!(
            "https://aiplatform.googleapis.com/v1/projects/{}/locations/global/publishers/google/models/{}:streamGenerateContent",
            project_id, model_name
        );

        // Authentication via gcloud CLI (application-default)
        let token_command = std::process::Command::new("gcloud")
            .arg("auth")
            .arg("application-default")
            .arg("print-access-token")
            .output()
            .map_err(|e| format!("Failed to execute 'gcloud auth application-default print-access-token': {}", e))?;

        if !token_command.status.success() {
            let stderr = String::from_utf8_lossy(&token_command.stderr);
            return Err(format!("'gcloud auth application-default print-access-token' failed: {}", stderr));
        }
        let access_token = String::from_utf8_lossy(&token_command.stdout).trim().to_string();

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(120)) // Increased timeout for streaming
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

            // For streamGenerateContent, the response is a JSON array of chunks.
            // We parse it as a Vec<GenerateContentResponse>.
            let response_chunks: Vec<GenerateContentResponse> = match response.json().await {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to parse JSON stream response: {}", e);
                    return Err(format!("Failed to parse JSON: {}", e));
                }
            };

            let mut full_text = String::new();

            for chunk in response_chunks {
                 if let Some(feedback) = chunk.prompt_feedback {
                    if let Some(reason) = feedback.block_reason {
                        return Err(format!("Prompt Blocked by API: {}", reason));
                    }
                }

                if let Some(candidates) = chunk.candidates {
                    if let Some(first_candidate) = candidates.first() {
                         if let Some(content) = &first_candidate.content {
                             for part in &content.parts {
                                 full_text.push_str(&part.text);
                             }
                         }
                    }
                }
            }

            if full_text.is_empty() {
                 return Err("[SIGNAL LOST] - Empty response from model.".to_string());
            }

            return Ok(full_text);
        }
    }

    pub async fn list_vertex_models(&self) -> Result<String, String> {
        let location = env::var("GOOGLE_CLOUD_REGION")
            .or_else(|_| env::var("REGION"))
            .unwrap_or_else(|_| "us-central1".to_string());

        let project_id = env::var("GOOGLE_CLOUD_PROJECT_ID")
            .or_else(|_| env::var("PROJECT_ID"))
            .map_err(|_| "GOOGLE_CLOUD_PROJECT_ID (or PROJECT_ID) not set".to_string())?;

        let url = format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/global/publishers/google/models",
            location, project_id
        );

        info!("Requesting Model List from: {}", url);

        let response = self.client.get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Model List Error {}: {}", status, text));
        }

        response.text().await.map_err(|e| format!("Failed to read response body: {}", e))
    }
}
