//! Temporal (TC39 proposal-temporal, the 2025 specification text) — the pure layer: the ISO 8601 calendar
//! and exact arithmetic (`iso`), the ISO string grammar with RFC 9557 annotations (`parse`), time zones with
//! historical transitions (`zone`, `tzdata`), and the difference / relative-rounding engine (`diff`). The
//! JavaScript surface lands on top of these.

pub mod diff;
pub mod iso;
pub mod parse;
pub mod tzdata;
pub mod zone;
