// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The shared backoff every provider sends through (VEINPROV, B303).
//!
//! This is the policy Vein's `SynapticRetry` (handlers/vein/src/synapse.rs)
//! always meant — exponential backoff with jitter on 429 and 5xx — lifted here
//! so every [`super::ModelProvider`] uses the same one; `SynapticRetry` now
//! delegates to [`send_with_backoff`].
//!
//! Classification: 408/409/429/5xx and connection errors are retryable;
//! everything else that is not a success is final. A 429 (or 503) carrying
//! `retry-after: <seconds>` waits that long instead of the computed backoff.

use std::time::Duration;

use reqwest::{RequestBuilder, Response};

use super::provider::ProviderError;

/// How hard to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// The first backoff; doubled per retry, plus up to 250 ms of jitter.
    pub base: Duration,
    /// Cap on any single wait, including an honoured `retry-after`.
    pub max_wait: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy { max_retries: 5, base: Duration::from_millis(1000), max_wait: Duration::from_secs(60) }
    }
}

impl RetryPolicy {
    /// A policy for tests: same shape, millisecond waits.
    pub fn fast() -> Self {
        RetryPolicy { max_retries: 3, base: Duration::from_millis(1), max_wait: Duration::from_millis(50) }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let jitter_cap = if self.base >= Duration::from_millis(250) { 250 } else { 1 };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let jitter = Duration::from_millis((nanos % jitter_cap) as u64);
        let exp = self.base.saturating_mul(1u32 << attempt.min(16));
        (exp + jitter).min(self.max_wait)
    }
}

/// Whether an HTTP status is worth retrying.
pub fn is_retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 429) || (500..600).contains(&status)
}

/// The `retry-after` header as a wait, when it is a number of seconds.
pub fn retry_after(res: &Response) -> Option<Duration> {
    res.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

/// Send `template` (re-cloned per attempt), retrying retryable statuses and
/// connection errors under `policy`. Answers the last response, whatever its
/// status, once the budget is spent — the caller classifies it. A connection
/// error on the last attempt is returned as the `Err`.
pub async fn send_with_backoff(template: RequestBuilder, policy: &RetryPolicy) -> reqwest::Result<Response> {
    let mut attempt = 0u32;
    loop {
        let req = match template.try_clone() {
            Some(r) => r,
            // A streaming body cannot be cloned: one shot, no retry.
            None => return template.send().await,
        };
        match req.send().await {
            Ok(res) => {
                let status = res.status().as_u16();
                if !is_retryable(status) || attempt >= policy.max_retries {
                    return Ok(res);
                }
                let wait = retry_after(&res).map(|w| w.min(policy.max_wait)).unwrap_or_else(|| policy.backoff(attempt));
                log::warn!("provider answered {status}; retry {} in {:?}", attempt + 1, wait);
                tokio::time::sleep(wait).await;
            }
            Err(e) => {
                if attempt >= policy.max_retries {
                    return Err(e);
                }
                let wait = policy.backoff(attempt);
                log::warn!("provider connection failed ({e}); retry {} in {:?}", attempt + 1, wait);
                tokio::time::sleep(wait).await;
            }
        }
        attempt += 1;
    }
}

/// [`send_with_backoff`], then classify: a success answers the response; a
/// non-success answers [`ProviderError::Request`] (final) or
/// [`ProviderError::Retryable`] (budget spent) carrying the body text.
pub async fn send_classified(template: RequestBuilder, policy: &RetryPolicy) -> Result<Response, ProviderError> {
    let res = send_with_backoff(template, policy)
        .await
        .map_err(|e| ProviderError::Retryable { status: 0, message: e.to_string() })?;
    let status = res.status().as_u16();
    if res.status().is_success() {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    let message = error_message(&body);
    if is_retryable(status) {
        Err(ProviderError::Retryable { status, message })
    } else {
        Err(ProviderError::Request { status, message })
    }
}

/// Pull `error.message` out of a JSON error body (both Anthropic and Google
/// shape it that way); otherwise the body itself, trimmed.
pub fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(m) = v.pointer("/error/message").and_then(|m| m.as_str())
    {
        return m.to_string();
    }
    body.trim().chars().take(2000).collect()
}
