//! The provider seam (design §2.2).
//!
//! SCAFFOLDING NOTE — read before extending.
//! =========================================
//! Properly, this trait belongs at the `gneiss_pal`/Vein boundary, with the
//! existing Vertex `ResilientClient` retrofitted as a second implementation, and
//! the CLI reaching Vein over the Bandy bus so Vein resolves Principia's
//! provider selection and Holocron's credential. Design §6 names both the trait
//! placement (a shared-crate decision, raised for the integrator) and the
//! direct-call shortcut past the bus as OPEN calls, and requires that a thin
//! loop taking the shortcut label it as scaffolding. This module is that label:
//!
//!   * the trait and its neutral types live HERE, inside `tools/foreman`, only
//!     so the thin loop can exist without touching a shared crate outside this
//!     track's lane;
//!   * they migrate into the Vein-owned crate (or `gneiss_pal`, per the
//!     integrator's ruling) at the Vein integration rung, at which point
//!     `foreman` deletes this file and depends on the shared trait;
//!   * the Claude connector moves with it, unchanged in shape — it is written as
//!     *a* connector, in the form any other connector would take.
//!
//! Design constraints honoured here:
//!   * provider-neutral types — no vendor wire schema is re-exported;
//!   * retry stays ABOVE the trait (this thin loop takes exactly one
//!     round-trip, so it has no retry at all yet);
//!   * optional operations degrade honestly through `capabilities()`;
//!   * errors are CLASSIFIED, not stringly-typed.

use std::fmt;

/// Who authored a turn. Provider-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One turn of a conversation. The thin loop only ever sends one.
#[derive(Debug, Clone)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// A provider-neutral request.
#[derive(Debug, Clone)]
pub struct Request {
    /// Operator framing. Not a place for credentials, ever.
    pub system: Option<String>,
    pub turns: Vec<Turn>,
    pub max_output_tokens: u32,
}

/// A provider-neutral response.
#[derive(Debug, Clone)]
pub struct Response {
    pub text: String,
    /// The model that actually served the response, as the provider reported it.
    pub model: Option<String>,
    pub usage: Option<Usage>,
    /// True when the provider declined rather than answered. A refusal is data,
    /// not an error — the loop reports it and stops.
    pub refused: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// What a connector can do, so callers degrade instead of failing.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub request: bool,
    pub stream: bool,
    pub embed: bool,
    /// Context limit in tokens, when the provider states one.
    pub context_tokens: Option<u64>,
}

/// Classified provider errors — `SynapticRetry` and the diagnosis loop decide
/// without parsing prose.
#[derive(Debug)]
pub enum ProviderError {
    /// No credential, or the credential was rejected.
    Auth(String),
    /// Rate limited / told to back off.
    RateLimited { retry_after_secs: Option<u64> },
    /// The request was too large for the provider.
    RequestTooLarge(String),
    /// Network / transport failure below the API.
    Transport(String),
    /// The provider answered, and the answer was "no".
    Refused(String),
    /// The provider answered something this connector could not translate.
    Malformed(String),
    /// The operation is not supported by this connector.
    Unsupported(&'static str),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Auth(m) => write!(f, "auth: {m}"),
            ProviderError::RateLimited { retry_after_secs: Some(s) } => {
                write!(f, "rate-limited (retry after {s}s)")
            }
            ProviderError::RateLimited { retry_after_secs: None } => write!(f, "rate-limited"),
            ProviderError::RequestTooLarge(m) => write!(f, "request too large: {m}"),
            ProviderError::Transport(m) => write!(f, "transport: {m}"),
            ProviderError::Refused(m) => write!(f, "provider refused: {m}"),
            ProviderError::Malformed(m) => write!(f, "malformed provider response: {m}"),
            ProviderError::Unsupported(op) => write!(f, "operation not supported by this provider: {op}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// The seam. Object-safe and synchronous on purpose: a connector that needs an
/// async transport owns its own runtime, so the trait does not force a runtime
/// on the kernel-adjacent callers that will link it later.
pub trait Provider: Send + Sync {
    /// Stable identifier for the transcript. Never a credential or an endpoint
    /// carrying one.
    fn name(&self) -> String;

    fn capabilities(&self) -> Capabilities;

    /// One prompt -> one complete response. The diagnosis loop's only
    /// requirement.
    fn request(&self, req: &Request) -> Result<Response, ProviderError>;

    /// Incremental response. Optional per provider; the thin loop never calls it.
    fn stream(
        &self,
        _req: &Request,
        _on_chunk: &mut dyn FnMut(&str),
    ) -> Result<Response, ProviderError> {
        Err(ProviderError::Unsupported("stream"))
    }
}

/// The `--provider=none` build: no provider is configured, and that is a VALID
/// configuration. The deterministic half of the tool never depends on the AI
/// half (design §2.4), so this exists to make the absence explicit rather than
/// to make a call.
pub struct NoProvider;

impl Provider for NoProvider {
    fn name(&self) -> String {
        "none".to_string()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { request: false, stream: false, embed: false, context_tokens: None }
    }

    fn request(&self, _req: &Request) -> Result<Response, ProviderError> {
        Err(ProviderError::Unsupported("request (no provider configured)"))
    }
}
