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

//! Gneiss's GitHub client (the forge). HTTPCORE (SR51): `octocrab` is gone — the three REST endpoints Gneiss
//! uses are spoken directly over [`crate::api::http`] (UnaOS's own HTTP + TLS):
//!
//! * `GET /user` — who the token belongs to,
//! * `GET /user/repos` — the first page of the caller's repositories (what octocrab's `send()` returned),
//! * `GET /repos/{owner}/{repo}/contents/{path}?ref=` — one file, base64-decoded (RFC 4648 §4).
//!
//! Headers per GitHub's REST docs: `Authorization: Bearer`, `Accept: application/vnd.github+json`,
//! `X-GitHub-Api-Version: 2022-11-28` and a `User-Agent` (GitHub refuses requests without one).

use std::env;

use serde_json::Value;

use crate::api::http::{Client, Url};

pub const GITHUB_API: &str = "https://api.github.com";
pub const GITHUB_API_VERSION: &str = "2022-11-28";

pub struct ForgeClient {
    http: Client,
    token: String,
    base: String,
}

impl ForgeClient {
    pub fn new() -> Result<Self, String> {
        let token = env::var("GITHUB_TOKEN").map_err(|_| "GITHUB_TOKEN not set".to_string())?;
        Self::with_token(token, GITHUB_API)
    }

    /// A client for `base` (the GitHub API root; a test server in tests).
    pub fn with_token(token: String, base: &str) -> Result<Self, String> {
        let http = Client::builder()
            .user_agent("UnaOS-gneiss-forge")
            .build()
            .map_err(|e| format!("Failed to build the forge client: {e}"))?;
        Ok(Self { http, token, base: base.trim_end_matches('/').to_string() })
    }

    async fn get_json(&self, path_and_query: &str) -> Result<Value, String> {
        let res = self
            .http
            .get(format!("{}{}", self.base, path_and_query))
            .bearer_auth(&self.token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", GITHUB_API_VERSION)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = res.status();
        let body = res.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
                .unwrap_or(body);
            return Err(format!("GitHub {status}: {msg}"));
        }
        serde_json::from_str(&body).map_err(|e| format!("GitHub answered non-JSON: {e}"))
    }

    pub async fn get_user_info(&self) -> Result<String, String> {
        match self.get_json("/user").await {
            Ok(v) => Ok(format!("Logged in as: {}", v.get("login").and_then(|l| l.as_str()).unwrap_or("?"))),
            Err(e) => Err(format!("Failed to fetch user info: {e}")),
        }
    }

    #[allow(dead_code)]
    pub async fn list_repos(&self) -> Result<Vec<String>, String> {
        let v = self.get_json("/user/repos").await.map_err(|e| format!("Failed to list repos: {e}"))?;
        Ok(v.as_array()
            .map(|a| a.iter().filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(str::to_string)).collect())
            .unwrap_or_default())
    }

    /// One file's text (lossy UTF-8). A directory listing or a missing file is an error.
    pub async fn get_file_content(&self, owner: &str, repo: &str, path: &str, branch: Option<&str>) -> Result<String, String> {
        // Each component is percent-encoded with the WHATWG path set; '/' inside `path` separates segments.
        let seg = |s: &str| http_core::url::percent_encode(s, http_core::url::EncodeSet::Component);
        let mut p = format!("/repos/{}/{}/contents/{}", seg(owner), seg(repo), path.split('/').map(seg).collect::<Vec<_>>().join("/"));
        if let Some(b) = branch {
            p.push_str("?ref=");
            p.push_str(&seg(b));
        }
        let _ = Url::parse(&format!("{}{}", self.base, p)).map_err(|e| e.to_string())?;
        let v = self.get_json(&p).await.map_err(|e| format!("Failed to fetch file content: {e}"))?;
        if v.is_array() {
            return Err("File not found or empty (that path is a directory)".to_string());
        }
        let content = v.get("content").and_then(|c| c.as_str()).ok_or("No content in file response")?;
        if v.get("encoding").and_then(|e| e.as_str()).unwrap_or("base64") != "base64" {
            return Ok(content.to_string());
        }
        let bytes = base64_decode(content).ok_or("file content is not valid base64")?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// RFC 4648 §4 base64, whitespace (GitHub wraps at 60 columns) skipped, padding optional.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn test_client_creation_without_token() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("GITHUB_TOKEN") };
        let client = ForgeClient::new();
        assert!(client.is_err());
    }

    #[test]
    fn rfc4648_vectors() {
        // RFC 4648 §10.
        for (enc, dec) in [("", ""), ("Zg==", "f"), ("Zm8=", "fo"), ("Zm9v", "foo"), ("Zm9vYg==", "foob"), ("Zm9vYmE=", "fooba"), ("Zm9v\nYmFy", "foobar")] {
            assert_eq!(base64_decode(enc).unwrap(), dec.as_bytes());
        }
        assert!(base64_decode("Zm9v!").is_none());
    }

    /// The three endpoints against a local GitHub-shaped server: paths, the auth/accept/version headers, and
    /// the base64 file decoded.
    #[tokio::test]
    async fn rest_endpoints_against_a_mock() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let mut s = stream.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap() == 0 {
                        return;
                    }
                    if l == "\r\n" {
                        break;
                    }
                    head.push_str(&l);
                }
                let path = head.split_whitespace().nth(1).unwrap().to_string();
                seen2.lock().unwrap().push(head.to_ascii_lowercase());
                let body = match path.as_str() {
                    "/user" => r#"{"login":"una"}"#.to_string(),
                    "/user/repos" => r#"[{"name":"UnaOS"},{"name":"notes"}]"#.to_string(),
                    p if p.starts_with("/repos/o/r/contents/docs/a%20b.md?ref=main") => {
                        r#"{"type":"file","encoding":"base64","content":"aGVsbG8g\nZm9yZ2UK"}"#.to_string()
                    }
                    _ => r#"{"message":"Not Found"}"#.to_string(),
                };
                write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let f = ForgeClient::with_token("ghp_test".into(), &format!("http://127.0.0.1:{port}")).unwrap();
        assert_eq!(f.get_user_info().await.unwrap(), "Logged in as: una");
        assert_eq!(f.list_repos().await.unwrap(), vec!["UnaOS".to_string(), "notes".to_string()]);
        assert_eq!(f.get_file_content("o", "r", "docs/a b.md", Some("main")).await.unwrap(), "hello forge\n");
        for h in seen.lock().unwrap().iter() {
            assert!(h.contains("authorization: bearer ghp_test"), "{h}");
            assert!(h.contains("accept: application/vnd.github+json"), "{h}");
            assert!(h.contains("x-github-api-version: 2022-11-28"), "{h}");
            assert!(h.contains("user-agent: unaos-gneiss-forge"), "{h}");
        }
    }
}
