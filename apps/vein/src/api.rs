use log::{error, info};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::env;
use std::time::Duration;

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
pub struct Part {
    pub text: String,
}

#[derive(Deserialize, Debug)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Deserialize, Debug)]
struct Candidate {
    pub content: Option<ContentResponse>, // Made Optional
    pub finish_reason: Option<String>,
    pub safety_ratings: Option<Vec<SafetyRating>>,
}

#[derive(Deserialize, Debug)]
struct ContentResponse {
    pub parts: Option<Vec<PartResponse>>, // Made Optional
}

#[derive(Deserialize, Debug)]
struct PartResponse {
    pub text: String,
}

#[derive(Deserialize, Debug)]
struct PromptFeedback {
    pub block_reason: Option<String>,
    pub safety_ratings: Option<Vec<SafetyRating>>,
}

#[derive(Deserialize, Debug)]
struct SafetyRating {
    pub category: String,
    pub probability: String,
}

pub struct GeminiClient {
    client: Client,
    api_key: String,
    model_url: String,
}

impl GeminiClient {
    pub fn new() -> Result<Self, String> {
        let api_key =
            env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set".to_string())?;

        let model_name =
            env::var("GEMINI_MODEL_NAME").unwrap_or_else(|_| "gemini-2.5-flash".to_string());
        let model_url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_name, api_key
        );

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .connection_verbose(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            client,
            api_key,
            model_url,
        })
    }

    pub async fn generate_content(&self, history: &[Content]) -> Result<String, String> {
        info!(
            "Sending request to Gemini API. History length: {}",
            history.len()
        );

        let request_body = GenerateContentRequest {
            contents: history.to_vec(),
        };

        let response = self
            .client
            .post(&self.model_url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                error!("HTTP Request failed: {}", e);
                format!("API Request failed: {}", e)
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("API Error {}: {}", status, text);
            return Err(format!("API Error {}: {}", status, text));
        }

        let response_data: GenerateContentResponse = response.json().await.map_err(|e| {
            error!("Failed to parse JSON response: {}", e);
            format!("Failed to parse JSON: {}", e)
        })?;

        // Extract text safely and provide more detailed error messages
        if let Some(feedback) = response_data.prompt_feedback {
            if let Some(reason) = feedback.block_reason {
                error!("Gemini API blocked prompt: {}", reason);
                return Err(format!("Prompt Blocked by API: {}", reason));
            }
            if let Some(safety_ratings) = feedback.safety_ratings {
                error!("Prompt Safety Ratings: {:?}", safety_ratings);
            }
        }

        if let Some(candidates) = response_data.candidates {
            if let Some(first_candidate) = candidates.first() {
                if let Some(reason) = &first_candidate.finish_reason {
                    info!("Gemini generation finished with reason: {}", reason);
                    if reason != "STOP" {
                        error!("Gemini API generation finished unexpectedly: {}", reason);
                    }
                }
                // Safely access content and parts
                if let Some(content_response) = &first_candidate.content {
                    if let Some(parts) = &content_response.parts {
                        if let Some(first_part) = parts.first() {
                            return Ok(first_part.text.clone());
                        }
                    }
                }
            }
        }

        error!("Gemini API returned an unexpected response format, no valid candidate text found.");
        Err("[SIGNAL LOST] - Unexpected response format or empty generation. Check logs for details.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // For async tests, ensure tokio test feature is enabled in Cargo.toml if running tests
    // For this specific module, basic tests don't strictly need tokio runtime if not making real API calls

    #[test]
    fn test_request_serialization() {
        let request = GenerateContentRequest {
            contents: vec![Content {
                role: "user".to_string(),
                parts: vec![Part {
                    text: "Hello".to_string(),
                }],
            }],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"contents":[{"role":"user","parts":[{"text":"Hello"}]}]}"#
        );
    }

    #[test]
    fn test_response_deserialization_with_all_fields() {
        let json = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "Hello Vein"
                            }
                        ]
                    },
                    "finish_reason": "STOP",
                    "safety_ratings": [
                        {"category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "probability": "NEGLIGIBLE"}
                    ]
                }
            ],
            "prompt_feedback": {
                "block_reason": "SAFETY",
                "safety_ratings": [
                    {"category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "probability": "NEGLIGIBLE"}
                ]
            }
        }"#;
        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.candidates.as_ref().unwrap()[0]
                .content
                .as_ref()
                .unwrap()
                .parts
                .as_ref()
                .unwrap()[0]
                .text,
            "Hello Vein"
        );
        assert_eq!(
            response.prompt_feedback.unwrap().block_reason.unwrap(),
            "SAFETY"
        );
    }

    #[test]
    fn test_response_deserialization_missing_parts() {
        let json = r#"{
            "candidates": [
                {
                    "content": {},
                    "finish_reason": "SAFETY",
                    "safety_ratings": []
                }
            ]
        }"#;
        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        assert!(response.candidates.as_ref().unwrap()[0]
            .content
            .as_ref()
            .unwrap()
            .parts
            .is_none());
    }

    // Example of an async integration test - requires GEMINI_API_KEY to be set
    #[tokio::test]
    #[ignore = "Requires GEMINI_API_KEY env var and live API call"]
    async fn test_gemini_api_call() {
        dotenvy::dotenv().ok();
        let client = GeminiClient::new().expect("Failed to create GeminiClient");
        let history = vec![Content {
            role: "user".to_string(),
            parts: vec![Part {
                text: "What is your purpose?".to_string(),
            }],
        }];
        let response = client.generate_content(&history).await;
        println!("Gemini API response: {:?}", response);
        assert!(response.is_ok());
        assert!(!response.unwrap().is_empty());
    }
}
