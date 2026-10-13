//! The WHATWG URL Standard — re-exported from `html_core::url` (URLCORE, LEDGER SR87), the one URL parser in
//! the tree: the basic URL parser with every state and state override, hosts with full UTS #46 IDNA
//! (mapping table, NFC, CheckBidi, CheckJoiners), the URL API setters, origins, the percent-encode sets and
//! application/x-www-form-urlencoded. Every name this module exported before (Url, UrlError, Host, UrlPath,
//! EncodeSet, percent_encode, percent_decode, parse_host, domain_to_ascii, punycode_*, default_port,
//! is_special) keeps its meaning.

pub use html_core::url::*;
