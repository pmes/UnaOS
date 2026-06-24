// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use serde_json::Value;

/// Takes a raw JSON string (e.g., from an external API or network request),
/// attempts to parse it to extract standard network fields, and returns a
/// human-readable formatted string suitable for display in the Network Inspector.
pub fn format_network_log(log: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<Value>(log) {
        let method = parsed.get("method").and_then(|v| v.as_str()).unwrap_or("UNKNOWN");
        let url = parsed.get("url").and_then(|v| v.as_str()).unwrap_or("UNKNOWN_URL");
        let status = parsed.get("status").and_then(|v| v.as_u64()).unwrap_or(0);
        let timestamp = parsed.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let latency = parsed.get("latency").and_then(|v| v.as_f64()).unwrap_or(0.0);

        if method != "UNKNOWN" || url != "UNKNOWN_URL" {
            format!("[{}] {} {} - {} ({}ms)\n{}",
                timestamp, method, url, status, latency,
                serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| log.to_string())
            )
        } else {
            serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| log.to_string())
        }
    } else {
        // If it's not valid JSON or we can't parse it, just return the raw string
        log.to_string()
    }
}