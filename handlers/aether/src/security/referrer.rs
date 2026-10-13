//! Referrer Policy (W3C Referrer Policy §3, §8): the eight policies plus the empty string (which means the
//! default, `strict-origin-when-cross-origin`), the `Referrer-Policy` header and `<meta name="referrer">`
//! parsers, and "determine request's referrer" (§8.3) with "strip url for use as a referrer" (§8.4).
//!
//! Every fetch site asks [`determine`]: `fetch()`/XHR (api::fetch, per hop — a redirect re-asks it against
//! the new target), script and module loads (js::host), and the navigation sites through
//! [`super::navigation_headers`].

/// A referrer policy (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferrerPolicy {
    /// The empty string: "use the default" (§3.10) — `strict-origin-when-cross-origin`.
    #[default]
    Empty,
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// One policy token (§4.1, ASCII case-insensitive); `None` for an unknown token.
    pub fn parse_token(s: &str) -> Option<ReferrerPolicy> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" => ReferrerPolicy::Empty,
            "no-referrer" => ReferrerPolicy::NoReferrer,
            "no-referrer-when-downgrade" => ReferrerPolicy::NoReferrerWhenDowngrade,
            "same-origin" => ReferrerPolicy::SameOrigin,
            "origin" => ReferrerPolicy::Origin,
            "strict-origin" => ReferrerPolicy::StrictOrigin,
            "origin-when-cross-origin" => ReferrerPolicy::OriginWhenCrossOrigin,
            "strict-origin-when-cross-origin" => ReferrerPolicy::StrictOriginWhenCrossOrigin,
            "unsafe-url" => ReferrerPolicy::UnsafeUrl,
            _ => return None,
        })
    }

    /// §8.1 "parse a referrer policy from a Referrer-Policy header": the LAST recognised token of the
    /// comma-separated list wins (so a new policy can be listed after a fallback older agents know).
    pub fn parse_header(value: &str) -> ReferrerPolicy {
        let mut out = ReferrerPolicy::Empty;
        for t in value.split(',') {
            if let Some(p) = ReferrerPolicy::parse_token(t) {
                if p != ReferrerPolicy::Empty {
                    out = p;
                }
            }
        }
        out
    }

    /// HTML §4.2.5.1 `<meta name="referrer" content>`: a token, or one of the legacy keywords.
    pub fn parse_meta(value: &str) -> Option<ReferrerPolicy> {
        match value.trim().to_ascii_lowercase().as_str() {
            "never" => Some(ReferrerPolicy::NoReferrer),
            "default" => Some(ReferrerPolicy::StrictOriginWhenCrossOrigin),
            "always" => Some(ReferrerPolicy::UnsafeUrl),
            "origin-when-crossorigin" => Some(ReferrerPolicy::OriginWhenCrossOrigin),
            v => ReferrerPolicy::parse_token(v).filter(|p| *p != ReferrerPolicy::Empty),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReferrerPolicy::Empty => "",
            ReferrerPolicy::NoReferrer => "no-referrer",
            ReferrerPolicy::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            ReferrerPolicy::SameOrigin => "same-origin",
            ReferrerPolicy::Origin => "origin",
            ReferrerPolicy::StrictOrigin => "strict-origin",
            ReferrerPolicy::OriginWhenCrossOrigin => "origin-when-cross-origin",
            ReferrerPolicy::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            ReferrerPolicy::UnsafeUrl => "unsafe-url",
        }
    }

    /// The policy in force: the empty string resolves to the default.
    pub fn effective(self) -> ReferrerPolicy {
        if self == ReferrerPolicy::Empty { ReferrerPolicy::StrictOriginWhenCrossOrigin } else { self }
    }
}

/// §8.4 "strip url for use as a referrer": no referrer for a local scheme, never credentials or a fragment;
/// `origin_only` also drops the path and query (leaving `scheme://host[:port]/`).
pub fn strip(url: &str, origin_only: bool) -> Option<String> {
    let mut u = url::Url::parse(url).ok()?;
    if matches!(u.scheme(), "about" | "blob" | "data" | "javascript" | "file") || u.cannot_be_a_base() {
        return None;
    }
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.set_fragment(None);
    if origin_only {
        u.set_path("/");
        u.set_query(None);
    }
    Some(u.to_string())
}

/// §8.3 steps 7–11: the referrer a request from `referrer` to `target` carries under `policy`; `None` = no
/// `Referer` header. A full URL longer than 4096 bytes falls back to the origin (step 7).
pub fn determine(policy: ReferrerPolicy, referrer: &str, target: &str) -> Option<String> {
    let full = strip(referrer, false)?;
    let origin = strip(referrer, true)?;
    let full = if full.len() > 4096 { origin.clone() } else { full };
    let same = super::origin::Origin::of(referrer).same_origin(&super::origin::Origin::of(target));
    let downgrade = super::origin::is_potentially_trustworthy_url(referrer) && !super::origin::is_potentially_trustworthy_url(target);
    match policy.effective() {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::Origin => Some(origin),
        ReferrerPolicy::UnsafeUrl => Some(full),
        ReferrerPolicy::StrictOrigin => (!downgrade).then_some(origin),
        ReferrerPolicy::StrictOriginWhenCrossOrigin | ReferrerPolicy::Empty => {
            if same {
                Some(full)
            } else if downgrade {
                None
            } else {
                Some(origin)
            }
        }
        ReferrerPolicy::SameOrigin => same.then_some(full),
        ReferrerPolicy::OriginWhenCrossOrigin => Some(if same { full } else { origin }),
        ReferrerPolicy::NoReferrerWhenDowngrade => (!downgrade).then_some(full),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eight_policies() {
        let r = "https://a.example/page?q=1#frag";
        let same = "https://a.example/x";
        let cross = "https://b.example/x";
        let down = "http://b.example/x";
        use ReferrerPolicy::*;
        assert_eq!(determine(NoReferrer, r, same), None);
        assert_eq!(determine(UnsafeUrl, r, down).as_deref(), Some("https://a.example/page?q=1"));
        assert_eq!(determine(Origin, r, same).as_deref(), Some("https://a.example/"));
        assert_eq!(determine(StrictOrigin, r, down), None);
        assert_eq!(determine(StrictOrigin, r, cross).as_deref(), Some("https://a.example/"));
        assert_eq!(determine(Empty, r, same).as_deref(), Some("https://a.example/page?q=1"));
        assert_eq!(determine(StrictOriginWhenCrossOrigin, r, cross).as_deref(), Some("https://a.example/"));
        assert_eq!(determine(StrictOriginWhenCrossOrigin, r, down), None);
        assert_eq!(determine(SameOrigin, r, cross), None);
        assert_eq!(determine(OriginWhenCrossOrigin, r, cross).as_deref(), Some("https://a.example/"));
        assert_eq!(determine(NoReferrerWhenDowngrade, r, down), None);
        assert_eq!(determine(NoReferrerWhenDowngrade, r, cross).as_deref(), Some("https://a.example/page?q=1"));
        assert_eq!(determine(UnsafeUrl, "https://u:p@a.example/", same).as_deref(), Some("https://a.example/"));
        assert_eq!(determine(UnsafeUrl, "data:text/html,x", same), None);
    }

    #[test]
    fn parsing() {
        assert_eq!(ReferrerPolicy::parse_header("unsafe-url, bogus, no-referrer"), ReferrerPolicy::NoReferrer);
        assert_eq!(ReferrerPolicy::parse_header("no-referrer, nonsense"), ReferrerPolicy::NoReferrer);
        assert_eq!(ReferrerPolicy::parse_meta("never"), Some(ReferrerPolicy::NoReferrer));
        assert_eq!(ReferrerPolicy::parse_meta("bogus"), None);
    }
}
