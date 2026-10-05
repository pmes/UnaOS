//! Redirects (RFC 9110 §15.4, with the method and header rules of the Fetch Standard §4.4 "HTTP-redirect
//! fetch", which is what every browser and `reqwest` implement).
//!
//! * 301/302: POST becomes GET (the historic behaviour §15.4.2/§15.4.3 permit); other methods are kept.
//! * 303: anything but GET/HEAD becomes GET (§15.4.4).
//! * 307/308: method and body are kept (§15.4.8/§15.4.9).
//! * A method change to GET drops the body and its request-content headers.
//! * Location is a URI-reference resolved against the current URL; a Location without a fragment inherits
//!   the current one (RFC 9110 §10.2.2).
//! * Leaving the origin strips credentials: Authorization, Proxy-Authorization, Cookie (the jar recomputes
//!   it) and the API-key headers UnaOS's providers use (x-api-key, x-goog-api-key).
//! * Only http/https targets are followed; at most [`MAX_REDIRECTS`] (Fetch: 20).

use alloc::string::String;

use crate::headers::Headers;
use crate::url::{Url, UrlError};

pub const MAX_REDIRECTS: usize = 20;

/// Headers that describe the request content (dropped when the body is).
pub const CONTENT_HEADERS: [&str; 5] = ["content-type", "content-length", "content-encoding", "content-language", "content-location"];

/// Credentials that never follow a redirect to another origin.
pub const CREDENTIAL_HEADERS: [&str; 5] = ["authorization", "proxy-authorization", "cookie", "x-api-key", "x-goog-api-key"];

pub fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectError {
    BadLocation(UrlError),
    /// A Location whose scheme is not http/https.
    UnsupportedScheme(String),
}

/// The next hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub url: Url,
    pub method: String,
    /// The request body (and its content headers) must be dropped.
    pub drop_body: bool,
    /// The target is another origin: credential headers were stripped.
    pub cross_origin: bool,
}

/// Compute the hop for a `status` + `location` answer to `method current`. `None`: not a redirect (or no
/// Location — the response is then the final answer, as Fetch says).
pub fn next_hop(status: u16, method: &str, current: &Url, location: Option<&str>) -> Option<Result<Hop, RedirectError>> {
    if !is_redirect(status) {
        return None;
    }
    let loc = location?;
    let mut url = match current.join(loc) {
        Ok(u) => u,
        Err(e) => return Some(Err(RedirectError::BadLocation(e))),
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return Some(Err(RedirectError::UnsupportedScheme(String::from(url.scheme()))));
    }
    if url.fragment().is_none() {
        if let Some(f) = current.fragment() {
            url.set_fragment(Some(f));
        }
    }
    let to_get = ((status == 301 || status == 302) && method.eq_ignore_ascii_case("POST"))
        || (status == 303 && !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD"));
    let method = if to_get { String::from("GET") } else { String::from(method) };
    let cross_origin = !current.same_origin(&url);
    Some(Ok(Hop { url, method, drop_body: to_get, cross_origin }))
}

/// Apply a hop's header rules to the request headers.
pub fn rewrite_headers(h: &mut Headers, hop: &Hop) {
    if hop.drop_body {
        for k in CONTENT_HEADERS {
            h.remove(k);
        }
    }
    if hop.cross_origin {
        for k in CREDENTIAL_HEADERS {
            h.remove(k);
        }
    }
}
