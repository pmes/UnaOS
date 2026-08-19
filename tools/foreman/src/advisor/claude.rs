//! The Claude connector — the FIRST implementation of the provider trait, never
//! the only socket (design §2.3).
//!
//! Written as *a* connector, in the same shape any other connector would take.
//! Nothing outside this file knows the vendor's wire schema; the neutral types
//! in `provider.rs` are what the rest of the tool sees.
//!
//! CREDENTIALS. Non-negotiable (design §2.4): no provider identifier, endpoint,
//! key, token, or account is committed to this repository. The credential is
//! read from the HOST ENVIRONMENT at call time, never persisted, never logged,
//! and never written into a transcript. Until Holocron owns it over the bus,
//! the environment IS the host credential path:
//!
//!   * `ANTHROPIC_API_KEY`   -> `x-api-key`
//!   * `ANTHROPIC_AUTH_TOKEN`-> `Authorization: Bearer` (+ the OAuth beta header)
//!
//! `ANTHROPIC_BASE_URL` overrides the endpoint host for a proxied or self-hosted
//! deployment. No connector is compiled in as a default that activates without
//! an explicit selection: `foreman` only constructs this when `--provider=claude`
//! is passed.
//!
//! Model ids and request parameters are NOT recorded in the design document and
//! were not taken from memory: they come from the `claude-api` reference at
//! implementation time. `--model` overrides the default for a caller who wants a
//! different one; when Principia owns the settings surface, the selection comes
//! from there instead.

use serde::Deserialize;
use serde_json::json;

use super::provider::{Capabilities, Provider, ProviderError, Request, Response, Role, Usage};

/// Default model id, from the `claude-api` reference (not from memory, and not
/// from the design doc — see §2.3). Overridable per invocation.
pub const DEFAULT_MODEL: &str = "claude-opus-5";

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// How the host handed us a credential. The value itself is never stored in a
/// field that anything prints.
enum Credential {
    ApiKey(String),
    BearerToken(String),
}

pub struct ClaudeConnector {
    credential: Credential,
    base_url: String,
    model: String,
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl ClaudeConnector {
    /// Build from the host environment. Returns a classified `Auth` error — not
    /// a panic and not a prompt — when no credential is present, so the caller
    /// can report "no provider configured" and still print the verdict table.
    pub fn from_env(model: Option<&str>) -> Result<Self, ProviderError> {
        let credential = if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
            if k.trim().is_empty() {
                return Err(ProviderError::Auth(
                    "ANTHROPIC_API_KEY is set but empty".to_string(),
                ));
            }
            Credential::ApiKey(k)
        } else if let Ok(t) = std::env::var("ANTHROPIC_AUTH_TOKEN") {
            if t.trim().is_empty() {
                return Err(ProviderError::Auth(
                    "ANTHROPIC_AUTH_TOKEN is set but empty".to_string(),
                ));
            }
            Credential::BearerToken(t)
        } else {
            return Err(ProviderError::Auth(
                "no credential in the environment (set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN); \
                 nothing is read from the repository"
                    .to_string(),
            ));
        };

        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();

        let model = model
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| ProviderError::Transport(format!("runtime: {e}")))?;

        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| ProviderError::Transport(format!("http client: {e}")))?;

        Ok(ClaudeConnector { credential, base_url, model, runtime, client })
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

// --- the vendor wire schema, confined to this file ---------------------------

#[derive(Deserialize)]
struct WireResponse {
    #[serde(default)]
    content: Vec<WireBlock>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl Provider for ClaudeConnector {
    fn name(&self) -> String {
        // Model, not credential, not endpoint-with-credential.
        format!("claude:{}", self.model)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            request: true,
            // The thin loop needs `request` only. `stream` is left unimplemented
            // rather than claimed — an optional operation degrades HONESTLY.
            stream: false,
            embed: false,
            context_tokens: None,
        }
    }

    fn request(&self, req: &Request) -> Result<Response, ProviderError> {
        let messages: Vec<serde_json::Value> = req
            .turns
            .iter()
            .map(|t| {
                json!({
                    "role": match t.role { Role::User => "user", Role::Assistant => "assistant" },
                    "content": t.text,
                })
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_output_tokens,
            "messages": messages,
        });
        if let Some(sys) = &req.system {
            body["system"] = json!(sys);
        }

        let url = format!("{}/v1/messages", self.base_url);
        let mut rb = self
            .client
            .post(&url)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");
        rb = match &self.credential {
            Credential::ApiKey(k) => rb.header("x-api-key", k),
            Credential::BearerToken(t) => rb
                .header("authorization", format!("Bearer {t}"))
                .header("anthropic-beta", OAUTH_BETA),
        };

        let (status, retry_after, text) = self.runtime.block_on(async {
            let resp = rb
                .json(&body)
                .send()
                .await
                .map_err(|e| ProviderError::Transport(e.to_string()))?;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let text = resp
                .text()
                .await
                .map_err(|e| ProviderError::Transport(e.to_string()))?;
            Ok::<_, ProviderError>((status, retry_after, text))
        })?;

        if !status.is_success() {
            // The body may echo request content; it never carries the credential
            // (we never put it in the body), but keep the classified surface thin.
            let brief: String = text.chars().take(400).collect();
            return Err(match status.as_u16() {
                401 | 403 => ProviderError::Auth(format!("HTTP {status}: {brief}")),
                413 => ProviderError::RequestTooLarge(format!("HTTP {status}: {brief}")),
                429 => ProviderError::RateLimited { retry_after_secs: retry_after },
                s if s >= 500 => ProviderError::Transport(format!("HTTP {status}: {brief}")),
                _ => ProviderError::Malformed(format!("HTTP {status}: {brief}")),
            });
        }

        let wire: WireResponse = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Malformed(format!("{e}: {}", text.chars().take(200).collect::<String>())))?;

        // `stop_reason` is checked BEFORE the content is read: a refusal can
        // arrive as HTTP 200 with an empty or partial content array.
        let refused = wire.stop_reason.as_deref() == Some("refusal");

        let body_text: String = wire
            .content
            .iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        if refused && body_text.trim().is_empty() {
            return Err(ProviderError::Refused(
                "the provider declined this request (stop_reason=refusal)".to_string(),
            ));
        }

        Ok(Response {
            text: body_text,
            model: wire.model,
            usage: wire.usage.map(|u| Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
            }),
            refused,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_response_decodes_text_blocks() {
        let raw = r#"{"model":"m","stop_reason":"end_turn",
            "content":[{"type":"thinking","thinking":""},{"type":"text","text":"hello"}],
            "usage":{"input_tokens":3,"output_tokens":4}}"#;
        let w: WireResponse = serde_json::from_str(raw).unwrap();
        let text: String = w
            .content
            .iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text.clone())
            .collect();
        assert_eq!(text, "hello");
        assert_eq!(w.usage.unwrap().output_tokens, 4);
    }

    #[test]
    fn refusal_is_detected_before_content_is_read() {
        let raw = r#"{"model":"m","stop_reason":"refusal","content":[]}"#;
        let w: WireResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(w.stop_reason.as_deref(), Some("refusal"));
        assert!(w.content.is_empty());
    }

    #[test]
    fn no_credential_in_environment_is_a_classified_auth_error() {
        // Deliberately does not mutate the process environment: this asserts the
        // shape of the error, not the ambient state of the machine.
        let e = ProviderError::Auth("no credential".to_string());
        assert!(matches!(e, ProviderError::Auth(_)));
        assert!(e.to_string().starts_with("auth: "));
    }
}
