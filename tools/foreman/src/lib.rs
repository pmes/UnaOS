//! `foreman` — the SH-4 smart installer's THINNEST working loop.
//!
//! Design: `unaos/docs/dev/OS/10_INSTALL/vein-smart-installer.md` (§5).
//! Working name; OPEN for Peter's naming pass. Nothing downstream should depend
//! on the name.
//!
//! Four modules that carry no CLI assumptions, so the `UnaOS_Installer` vessel
//! can later link the same modules and supply its own front end (design §5.1):
//!
//! | module      | responsibility                                                |
//! |-------------|---------------------------------------------------------------|
//! | `capture`   | read a log as bytes; sanitize; yield lines with positions      |
//! | `verdict`   | parse a `.spec`; evaluate; produce a STRUCTURED result + render|
//! | `context`   | assemble the bounded diagnostic context under explicit budgets |
//! | `advisor`   | call the provider through the trait; parse the closed action set|
//!
//! `main` wires them and prints; `transcript` is a sink the caller passes in.
//!
//! What this loop is NOT: it opens no device, follows no log, injects nothing,
//! flashes nothing, and controls no power. Those are later rungs.

pub mod advisor;
pub mod capture;
pub mod context;
pub mod transcript;
pub mod verdict;
