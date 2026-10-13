//! CORS (Fetch §3.2, §4.8–§4.10) and `Cross-Origin-Resource-Policy` (Fetch §3.6, "cross-origin resource
//! policy check").
//!
//! - the safelists: CORS-safelisted methods (GET/HEAD/POST), CORS-safelisted request-headers (Accept,
//!   Accept-Language, Content-Language, Content-Type with its three MIME essences, a simple Range; every value
//!   ≤ 128 bytes and the safelisted values together ≤ 1024), CORS-safelisted response-header names;
//! - the forbidden request-headers a page may never set (Fetch §2.2.2) and the forbidden response-header
//!   names script never sees (`Set-Cookie`, `Set-Cookie2`);
//! - [`needs_preflight`], the preflight's own headers, [`check_preflight`] and the preflight cache
//!   (keyed by origin, URL and credentials; `Access-Control-Max-Age`, default 5 s, capped at 7200 s);
//! - [`cors_check`] (`Access-Control-Allow-Origin` / `-Credentials`) and [`filter_response_headers`] for the
//!   `cors` filtered response (`Access-Control-Expose-Headers`, `*` when not credentialed);
//! - [`corp_check`] for `no-cors` cross-origin responses.
//!
//! Header lists here are `(lowercase name, value)` pairs.

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub type HeaderList = Vec<(String, String)>;

pub fn get<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// Fetch §2.2.1 CORS-safelisted method.
pub fn is_safelisted_method(m: &str) -> bool {
    matches!(m, "GET" | "HEAD" | "POST")
}

/// Fetch §2.2.2 forbidden request-header (name, value): the page may not set it.
pub fn is_forbidden_request_header(name: &str, value: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const FORBIDDEN: &[&str] = &[
        "accept-charset", "accept-encoding", "access-control-request-headers", "access-control-request-method",
        "connection", "content-length", "cookie", "cookie2", "date", "dnt", "expect", "host", "keep-alive", "origin",
        "referer", "set-cookie", "te", "trailer", "transfer-encoding", "upgrade", "via",
    ];
    if FORBIDDEN.contains(&n.as_str()) || n.starts_with("proxy-") || n.starts_with("sec-") {
        return true;
    }
    if matches!(n.as_str(), "x-http-method" | "x-http-method-override" | "x-method-override") {
        return value.split(',').any(|m| matches!(m.trim().to_ascii_uppercase().as_str(), "CONNECT" | "TRACE" | "TRACK"));
    }
    false
}

/// Fetch §2.2.2 forbidden response-header name.
pub fn is_forbidden_response_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("set-cookie") || name.eq_ignore_ascii_case("set-cookie2")
}

fn is_cors_unsafe_byte(b: u8) -> bool {
    (b < 0x20 && b != 0x09) || matches!(b, b'"' | b'(' | b')' | b':' | b'<' | b'>' | b'?' | b'@' | b'[' | b'\\' | b']' | b'{' | b'}' | 0x7F)
}

/// Fetch §2.2.2 CORS-safelisted request-header (name, value).
pub fn is_safelisted_request_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.bytes().any(is_cors_unsafe_byte),
        "accept-language" | "content-language" => {
            value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'*' | b',' | b'-' | b'.' | b';' | b'='))
        }
        "content-type" => {
            if value.bytes().any(is_cors_unsafe_byte) {
                return false;
            }
            let essence = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            matches!(essence.as_str(), "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain")
        }
        "range" => {
            let Some(r) = value.strip_prefix("bytes=") else { return false };
            let Some((a, b)) = r.split_once('-') else { return false };
            !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
                && (b.is_empty() || a.parse::<u64>().ok() <= b.parse::<u64>().ok())
        }
        _ => false,
    }
}

/// Fetch §2.2.2 "CORS-unsafe request-header names": sorted, lowercase, unique.
pub fn unsafe_header_names(h: &[(String, String)]) -> Vec<String> {
    let mut unsafe_names = Vec::new();
    let mut safelisted_size = 0usize;
    for (k, v) in h {
        if is_safelisted_request_header(k, v) {
            safelisted_size += v.len();
        } else {
            unsafe_names.push(k.to_ascii_lowercase());
        }
    }
    if safelisted_size > 1024 {
        unsafe_names.extend(h.iter().filter(|(k, v)| is_safelisted_request_header(k, v)).map(|(k, _)| k.to_ascii_lowercase()));
    }
    unsafe_names.sort();
    unsafe_names.dedup();
    unsafe_names
}

/// Whether a `cors`-mode cross-origin request must be preflighted (Fetch §4.1 main fetch step 12 / §4.8).
pub fn needs_preflight(method: &str, h: &[(String, String)]) -> bool {
    !is_safelisted_method(method) || !unsafe_header_names(h).is_empty()
}

/// The preflight request's extra headers (Fetch §4.9 steps 1–6): the method, the unsafe header names.
pub fn preflight_headers(method: &str, h: &[(String, String)]) -> HeaderList {
    let mut out = vec![("accept".to_string(), "*/*".to_string()), ("access-control-request-method".to_string(), method.to_string())];
    let names = unsafe_header_names(h);
    if !names.is_empty() {
        out.push(("access-control-request-headers".to_string(), names.join(",")));
    }
    out
}

/// Fetch §4.10 CORS check: `Ok` when `Access-Control-Allow-Origin` (and, credentialed, `-Credentials: true`)
/// admits `origin`.
pub fn cors_check(h: &[(String, String)], origin: &str, include_credentials: bool) -> Result<(), String> {
    let acao: Vec<&str> = h.iter().filter(|(k, _)| k.eq_ignore_ascii_case("access-control-allow-origin")).map(|(_, v)| v.trim()).collect();
    let Some(&allow) = acao.first() else {
        return Err(format!("No 'Access-Control-Allow-Origin' header is present on the requested resource (origin '{origin}')"));
    };
    if acao.len() > 1 || allow.contains(',') {
        return Err("The 'Access-Control-Allow-Origin' header contains multiple values, but only one is allowed".into());
    }
    if !include_credentials && allow == "*" {
        return Ok(());
    }
    if allow != origin {
        return Err(if allow == "*" {
            "The value of the 'Access-Control-Allow-Origin' header must not be the wildcard '*' when the request's credentials mode is 'include'".into()
        } else {
            format!("The 'Access-Control-Allow-Origin' header has a value '{allow}' that is not equal to the supplied origin '{origin}'")
        });
    }
    if !include_credentials {
        return Ok(());
    }
    if get(h, "access-control-allow-credentials").map(str::trim) != Some("true") {
        return Err("The value of the 'Access-Control-Allow-Credentials' header is not 'true', which is required when the request's credentials mode is 'include'".into());
    }
    Ok(())
}

fn list(h: &[(String, String)], name: &str) -> Vec<String> {
    h.iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .flat_map(|(_, v)| v.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect::<Vec<_>>())
        .collect()
}

/// One preflight-cache entry.
struct CacheEntry {
    key: (String, String, bool),
    methods: Vec<String>,
    headers: Vec<String>,
    expires: Instant,
}

fn cache() -> &'static Mutex<Vec<CacheEntry>> {
    static C: std::sync::OnceLock<Mutex<Vec<CacheEntry>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// Fetch §4.8 step 2: does a live cache entry cover this method and these headers?
pub fn preflight_cached(origin: &str, url: &str, include: bool, method: &str, h: &[(String, String)]) -> bool {
    let now = Instant::now();
    let Ok(mut c) = cache().lock() else { return false };
    c.retain(|e| e.expires > now);
    let key = (origin.to_string(), url.to_string(), include);
    let Some(e) = c.iter().find(|e| e.key == key) else { return false };
    let method_ok = is_safelisted_method(method) || e.methods.iter().any(|m| m == method || (!include && m == "*"));
    let headers_ok = unsafe_header_names(h).iter().all(|n| e.headers.iter().any(|x| x.eq_ignore_ascii_case(n) || (!include && x == "*" && n != "authorization")));
    method_ok && headers_ok
}

/// Fetch §4.9 steps 7–… on the preflight's answer: an ok status, the CORS check, then the method and
/// header lists; on success the result is cached for `Access-Control-Max-Age` seconds.
pub fn check_preflight(status: u16, h: &[(String, String)], origin: &str, url: &str, include: bool, method: &str, req_headers: &[(String, String)]) -> Result<(), String> {
    cors_check(h, origin, include).map_err(|e| format!("Response to preflight request doesn't pass access control check: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("Response to preflight request doesn't pass access control check: it does not have HTTP ok status ({status})"));
    }
    let methods = list(h, "access-control-allow-methods");
    let headers = list(h, "access-control-allow-headers");
    if !methods.iter().any(|m| m == method) && !is_safelisted_method(method) && (include || !methods.iter().any(|m| m == "*")) {
        return Err(format!("Method {method} is not allowed by Access-Control-Allow-Methods in preflight response"));
    }
    for n in unsafe_header_names(req_headers) {
        let listed = headers.iter().any(|x| x.eq_ignore_ascii_case(&n));
        if listed {
            continue;
        }
        if n == "authorization" || include || !headers.iter().any(|x| x == "*") {
            return Err(format!("Request header field {n} is not allowed by Access-Control-Allow-Headers in preflight response"));
        }
    }
    let max_age = get(h, "access-control-max-age").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(5).min(7200);
    if max_age > 0 {
        if let Ok(mut c) = cache().lock() {
            let key = (origin.to_string(), url.to_string(), include);
            c.retain(|e| e.key != key);
            if c.len() > 512 {
                c.remove(0);
            }
            c.push(CacheEntry { key, methods, headers, expires: Instant::now() + Duration::from_secs(max_age) });
        }
    }
    Ok(())
}

/// Fetch §2.2.6 CORS-safelisted response-header name.
pub fn is_safelisted_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cache-control" | "content-language" | "content-length" | "content-type" | "expires" | "last-modified" | "pragma"
    )
}

/// The headers script sees: a basic response drops the forbidden response-header names; a `cors` response
/// keeps only the safelisted names and those `Access-Control-Expose-Headers` lists (`*` = all, when not
/// credentialed); an opaque response has none.
pub fn filter_response_headers(h: &[(String, String)], kind: &str, include: bool) -> HeaderList {
    match kind {
        "opaque" | "opaqueredirect" => Vec::new(),
        "cors" => {
            let expose = list(h, "access-control-expose-headers");
            let all = !include && expose.iter().any(|e| e == "*");
            h.iter()
                .filter(|(k, _)| !is_forbidden_response_header(k))
                .filter(|(k, _)| all || is_safelisted_response_header(k) || expose.iter().any(|e| e.eq_ignore_ascii_case(k)))
                .cloned()
                .collect()
        }
        _ => h.iter().filter(|(k, _)| !is_forbidden_response_header(k)).cloned().collect(),
    }
}

/// Fetch §3.6 cross-origin resource policy check for a `no-cors` response from `response_url` to a
/// request from `client`: `Err(reason)` when `Cross-Origin-Resource-Policy` forbids it.
pub fn corp_check(client: &super::origin::Origin, response_url: &str, h: &[(String, String)]) -> Result<(), String> {
    let Some(policy) = get(h, "cross-origin-resource-policy") else { return Ok(()) };
    let policy = policy.trim();
    let target = super::origin::Origin::of(response_url);
    if client.same_origin(&target) {
        return Ok(());
    }
    match policy {
        "same-origin" => Err(format!("Cross-Origin-Resource-Policy: same-origin forbids {} from {}", target.serialize(), client.serialize())),
        "same-site" => {
            if client.scheme() == "https" && target.scheme() != "https" {
                return Err("Cross-Origin-Resource-Policy: same-site forbids a non-https response to an https document".into());
            }
            if client.schemelessly_same_site(&target) {
                Ok(())
            } else {
                Err(format!("Cross-Origin-Resource-Policy: same-site forbids {} from {}", target.site(), client.site()))
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> HeaderList {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn safelists_and_preflight() {
        assert!(!needs_preflight("GET", &h(&[("accept", "*/*"), ("content-type", "text/plain;charset=UTF-8")])));
        assert!(needs_preflight("PUT", &[]));
        assert!(needs_preflight("POST", &h(&[("content-type", "application/json")])));
        assert!(needs_preflight("GET", &h(&[("x-custom", "1")])));
        assert!(is_safelisted_request_header("range", "bytes=0-99"));
        assert!(!is_safelisted_request_header("range", "bytes=0-99,200-"));
        assert_eq!(preflight_headers("PUT", &h(&[("X-B", "1"), ("x-a", "2")]))[2].1, "x-a,x-b");
        assert!(is_forbidden_request_header("Sec-Fetch-Site", ""));
        assert!(is_forbidden_request_header("x-http-method-override", "TRACE"));
        assert!(!is_forbidden_request_header("x-http-method-override", "PATCH"));
    }

    #[test]
    fn checks() {
        let o = "https://a.example";
        assert!(cors_check(&h(&[("access-control-allow-origin", "*")]), o, false).is_ok());
        assert!(cors_check(&h(&[("access-control-allow-origin", "*")]), o, true).is_err());
        assert!(cors_check(&h(&[("access-control-allow-origin", o)]), o, true).is_err());
        assert!(cors_check(&h(&[("access-control-allow-origin", o), ("access-control-allow-credentials", "true")]), o, true).is_ok());
        assert!(cors_check(&[], o, false).is_err());
        let pre = h(&[("access-control-allow-origin", o), ("access-control-allow-methods", "PUT"), ("access-control-allow-headers", "x-a"), ("access-control-max-age", "0")]);
        assert!(check_preflight(200, &pre, o, "https://b.example/", false, "PUT", &h(&[("x-a", "1")])).is_ok());
        assert!(check_preflight(200, &pre, o, "https://b.example/", false, "DELETE", &[]).is_err());
        assert!(check_preflight(500, &pre, o, "https://b.example/", false, "PUT", &[]).is_err());
        let exposed = filter_response_headers(&h(&[("content-type", "a"), ("x-secret", "b"), ("set-cookie", "c")]), "cors", false);
        assert_eq!(exposed.len(), 1);
        let client = super::super::origin::Origin::of("https://a.example/");
        assert!(corp_check(&client, "https://b.example/i.png", &h(&[("cross-origin-resource-policy", "same-origin")])).is_err());
        assert!(corp_check(&client, "https://x.a.example/i.png", &h(&[("cross-origin-resource-policy", "same-site")])).is_ok());
    }
}
