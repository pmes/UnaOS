use log::{error, info, warn};
use reqwest::{Client, ClientBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::process::Command;

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct GenerationConfig {
    temperature: f32,
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
    prompt_feedback: Option<PromptFeedback>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct UsageMetadata {
    #[serde(rename = "promptTokenCount")]
    pub prompt_token_count: Option<i32>,
    #[serde(rename = "candidatesTokenCount")]
    pub candidates_token_count: Option<i32>,
    #[serde(rename = "totalTokenCount")]
    pub total_token_count: Option<i32>,
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

pub struct ResilientClient {
    client: Client,
    model_url: String,
    token: String,
}

impl ResilientClient {
    pub async fn new() -> Result<Self, String> {
        let token = Self::fetch_token()?;

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

    fn fetch_token() -> Result<String, String> {
        let output = Command::new("gcloud")
            .args(["auth", "application-default", "print-access-token"])
            .output()
            .map_err(|e| format!("Failed to execute gcloud for token: {}", e))?;

        if !output.status.success() {
            return Err("Failed to retrieve gcloud access token. Ensure gcloud ADC is configured.".to_string());
        }

        String::from_utf8(output.stdout)
            .map(|s| s.trim().to_string())
            .map_err(|_| "Invalid UTF-8 in gcloud token".to_string())
    }

    pub async fn refresh_token(&mut self) -> Result<(), String> {
        info!("Refreshing GCloud Token (Lazarus Protocol)...");
        match Self::fetch_token() {
            Ok(t) => {
                self.token = t;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub async fn generate_content(&mut self, history: &[Content]) -> Result<(String, Option<UsageMetadata>), String> {
        let request_body = GenerateContentRequest {
            contents: history.to_vec(),
            generation_config: GenerationConfig {
                temperature: 0.4,
            },
        };

        let mut attempts = 0;
        loop {
            attempts += 1;
            info!("Transmitting to Neural Core (Attempt {})...", attempts);

            let response = self
                .client
                .post(&self.model_url)
                .bearer_auth(&self.token)
                .json(&request_body)
                .send()
                .await
                .map_err(|e| format!("Transmission Failed: {}", e))?;

            if response.status() == StatusCode::UNAUTHORIZED {
                if attempts < 2 {
                    warn!("401 Unauthorized detected. Initiating Lazarus Protocol...");
                    if let Err(e) = self.refresh_token().await {
                        return Err(format!("Lazarus Protocol Failed: {}", e));
                    }
                    continue;
                } else {
                    return Err("Authentication Failed after retry.".to_string());
                }
            }

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
                        return Ok((full_text, data.usage_metadata));
                    }
                }
            }

            return Err("Neural Core returned silence (Empty Response).".to_string());
        }
    }

    pub async fn list_vertex_models(&self) -> Result<String, String> {
        Ok("Model listing bypass engaged. Hardcoded to gemini-3-pro-preview.".to_string())
    }

    pub async fn embed_content(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let url = "https://aiplatform.googleapis.com/v1/projects/unauploads-1769528906/locations/global/publishers/google/models/text-embedding-004:predict";

        let request_body = EmbedContentRequest {
            instances: vec![EmbedContentInstance {
                content: text.to_string(),
            }],
        };

        let mut attempts = 0;
        loop {
            attempts += 1;

            let response = self
                .client
                .post(url)
                .bearer_auth(&self.token)
                .json(&request_body)
                .send()
                .await
                .map_err(|e| format!("Embedding Transmission Failed: {}", e))?;

            if response.status() == StatusCode::UNAUTHORIZED {
                if attempts < 2 {
                    warn!("401 Unauthorized (Embedding). Initiating Lazarus Protocol...");
                    if let Err(e) = self.refresh_token().await {
                        return Err(format!("Lazarus Protocol Failed: {}", e));
                    }
                    continue;
                } else {
                    return Err("Authentication Failed after retry.".to_string());
                }
            }

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(format!("Embedding Failure {}: {}", status, text));
            }

            let data: EmbedContentResponse = response
                .json()
                .await
                .map_err(|e| format!("Failed to decode embedding: {}", e))?;

            if let Some(predictions) = data.predictions {
                if let Some(first) = predictions.first() {
                    return Ok(first.embeddings.values.clone());
                }
            }

            return Err("Neural Core returned no embedding.".to_string());
        }
    }
}

#[derive(Serialize)]
struct EmbedContentRequest {
    instances: Vec<EmbedContentInstance>,
}

#[derive(Serialize)]
struct EmbedContentInstance {
    content: String,
}

#[derive(Deserialize)]
struct EmbedContentResponse {
    predictions: Option<Vec<EmbedPrediction>>,
}

#[derive(Deserialize)]
struct EmbedPrediction {
    embeddings: EmbedValues,
}

#[derive(Deserialize)]
struct EmbedValues {
    values: Vec<f32>,
}
