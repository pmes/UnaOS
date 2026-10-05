//! HTMLCORE (LEDGER SR46) — UnaOS's own HTML parser: the WHATWG HTML tokenizer (§13.2.5), tree construction
//! (§13.2.6) into an arena DOM, the fragment parsing algorithm (§13.4) and the serializer (§13.3).
//!
//! `no_std` + `alloc`, zero dependencies, no `unsafe`. Written from the WHATWG HTML Living Standard; the named
//! character reference table is generated data (`src/entities.rs`, from the spec's entities.json).
//!
//! ```
//! use html_core::{parse_document, ParseOpts, serialize::{outer_html, SerializeOpts}};
//! let doc = parse_document("<p>Hello<b>world", ParseOpts::default());
//! let html = doc.document_element().unwrap();
//! assert_eq!(outer_html(&doc, html, SerializeOpts::default()),
//!            "<html><head></head><body><p>Hello<b>world</b></p></body></html>");
//! ```

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod dom;
pub mod entities;
pub mod serialize;
pub mod tokenizer;
pub mod tree_builder;

pub use dom::{Attribute, Document, Element, Namespace, Node, NodeData, NodeId, QuirksMode};
pub use tree_builder::{parse_document, parse_fragment, ParseOpts};
