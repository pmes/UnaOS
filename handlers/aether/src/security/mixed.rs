//! Mixed Content (W3C Mixed Content Level 2) and Upgrade Insecure Requests (W3C UIR §4).
//!
//! Order per request (and per redirect hop): first the document's `upgrade-insecure-requests` directive
//! rewrites `http:`→`https:` / `ws:`→`wss:` (port 80 dropped); then, from a secure context, a request whose
//! URL is still not "a priori authenticated" is either UPGRADED (an upgradeable destination — image, audio,
//! video — Mixed Content L2 §4.1 autoupgrade) or BLOCKED (everything active: script, style, fetch/XHR, frame,
//! worker, font, object, …). `block-all-mixed-content` blocks the upgradeable kinds too (it is deprecated
//! in CSP3 but still honoured here, as it is the stricter reading).

/// The verdict for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Fetch `url` as given.
    Allow,
    /// Fetch this rewritten URL instead (UIR, or the passive autoupgrade).
    Upgrade(String),
    /// A network error; the reason names the directive or the rule.
    Block(&'static str),
}

/// Destinations Mixed Content L2 §3.1 lets the user agent upgrade rather than block.
pub fn is_upgradeable(destination: &str) -> bool {
    matches!(destination, "image" | "audio" | "video")
}

/// UIR §4.1: `http:`→`https:` and `ws:`→`wss:`, the default port dropped.
pub fn upgrade_url(url: &str) -> Option<String> {
    let mut u = url::Url::parse(url).ok()?;
    let to = match u.scheme() {
        "http" => "https",
        "ws" => "wss",
        _ => return None,
    };
    let port = u.port();
    u.set_scheme(to).ok()?;
    if port == Some(80) {
        let _ = u.set_port(None);
    }
    Some(u.to_string())
}

/// Mixed Content §4.3 "a priori authenticated URL": a potentially trustworthy URL.
fn a_priori_authenticated(url: &str) -> bool {
    super::origin::is_potentially_trustworthy_url(url)
}

/// The decision for a request to `request_url` of `destination` from the document at `client_url`.
pub fn check(client_url: &str, request_url: &str, destination: &str, upgrade_insecure: bool, block_all: bool) -> Verdict {
    let mut url = request_url.to_string();
    let mut upgraded = false;
    if upgrade_insecure {
        if let Some(u) = upgrade_url(&url) {
            url = u;
            upgraded = true;
        }
    }
    let secure_context = !client_url.is_empty() && super::origin::is_potentially_trustworthy_url(client_url) && !client_url.starts_with("data:");
    if secure_context && !a_priori_authenticated(&url) && (url.starts_with("http:") || url.starts_with("ws:")) {
        if block_all {
            return Verdict::Block("block-all-mixed-content");
        }
        if is_upgradeable(destination) {
            return match upgrade_url(&url) {
                Some(u) => Verdict::Upgrade(u),
                None => Verdict::Block("mixed-content"),
            };
        }
        return Verdict::Block("mixed-content");
    }
    if upgraded { Verdict::Upgrade(url) } else { Verdict::Allow }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_blocked_passive_upgraded() {
        let page = "https://a.example/";
        assert_eq!(check(page, "http://b.example/x.js", "script", false, false), Verdict::Block("mixed-content"));
        assert_eq!(check(page, "http://b.example/api", "", false, false), Verdict::Block("mixed-content"));
        assert_eq!(check(page, "http://b.example/i.png", "image", false, false), Verdict::Upgrade("https://b.example/i.png".into()));
        assert_eq!(check(page, "http://b.example/i.png", "image", false, true), Verdict::Block("block-all-mixed-content"));
        assert_eq!(check(page, "http://localhost:8000/x", "", false, false), Verdict::Allow);
        assert_eq!(check("http://a.example/", "http://b.example/x", "", false, false), Verdict::Allow);
        assert_eq!(check("http://a.example/", "http://b.example:80/x", "", true, false), Verdict::Upgrade("https://b.example/x".into()));
        assert_eq!(upgrade_url("ws://h:81/s").as_deref(), Some("wss://h:81/s"));
    }
}
