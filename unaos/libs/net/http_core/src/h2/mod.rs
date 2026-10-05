//! HTTP/2 (feature `h2`): framing (RFC 9113 §4–§6), HPACK (RFC 7541), and a client connection with streams,
//! flow control and settings. The host transport negotiates it by ALPN `h2` when `AgentConfig::alpn` offers it.

pub mod client;
pub mod frame;
pub mod hpack;
mod huffman_table;

pub use client::H2Conn;
