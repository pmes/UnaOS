//! `multipart/form-data` (RFC 7578) — the encoder, with names and filenames escaped as the HTML Standard's
//! form-data encoding does (`"` → `%22`, CR → `%0D`, LF → `%0A`). The boundary is the caller's (no RNG in a
//! `no_std` core): it must not occur in any part.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

pub struct Form {
    boundary: String,
    parts: Vec<Part>,
}

fn esc(s: &str) -> String {
    s.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A")
}

impl Form {
    pub fn new(boundary: &str) -> Self {
        Form { boundary: String::from(boundary), parts: Vec::new() }
    }

    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.parts.push(Part { name: name.into(), filename: None, content_type: None, data: value.as_bytes().to_vec() });
        self
    }

    pub fn file(mut self, name: &str, filename: &str, content_type: &str, data: Vec<u8>) -> Self {
        self.parts.push(Part { name: name.into(), filename: Some(filename.into()), content_type: Some(content_type.into()), data });
        self
    }

    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    /// The `Content-Type` field value.
    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// The body (RFC 7578 §4 / RFC 2046 §5.1.1).
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        for p in &self.parts {
            o.extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
            let mut cd = format!("Content-Disposition: form-data; name=\"{}\"", esc(&p.name));
            if let Some(f) = &p.filename {
                cd.push_str(&format!("; filename=\"{}\"", esc(f)));
            }
            o.extend_from_slice(cd.as_bytes());
            o.extend_from_slice(b"\r\n");
            if let Some(ct) = &p.content_type {
                o.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
            }
            o.extend_from_slice(b"\r\n");
            o.extend_from_slice(&p.data);
            o.extend_from_slice(b"\r\n");
        }
        o.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        o
    }
}
