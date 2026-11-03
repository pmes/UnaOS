//! CHARTER: Vug (CODEX §2, 3D) — shared-core: step_core, the STEP exchange of the CAD program (docs/dev/USERLAND/VUG.md
//! principle 6; the sheet docs/dev/blueprint/vug.md BP-vug-6).
//!
//! VUGSTEP (LEDGER SR482) builds, from the public standards only (no vendor SDK, no code copied):
//! - [`p21`]: ISO 10303-21, the clear-text encoding of the exchange structure — the HEADER section (FILE_DESCRIPTION,
//!   FILE_NAME, FILE_SCHEMA), the DATA section of `#n = ENTITY(...);` simple and `#n = (A(...) B(...));` complex
//!   instances, typed parameters, lists, `$` and `*`, strings with their `\X2\` / `\X4\` / `\X\` / `\S\` escapes —
//!   into an instance table, and the writer's REAL and STRING spellings;
//! - [`read`]: AP214 (ISO 10303-214, the `automotive_design` schema) advanced B-rep shape representations into
//!   brep_core `Brep`s, scaled by the file's length unit to millimetres;
//! - [`write`]: brep_core `Brep`s into an AP214 file, deterministic (instances numbered in arena order: the same
//!   Brep writes the same bytes).
//!
//! Refuse, don't guess: every input this crate cannot read exactly is a named [`Refusal`] — a syntax error by line and
//! column, an entity the subset lacks by its name and instance number — never a silent skip. Coordinates are f64
//! (BP-vug-1); every comparison takes the caller's `Tol` (BP-vug-2); the crate is pure (BP-vug-4).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod p21;
mod read;
mod write;

pub use read::read;
pub use write::{Header, write};

use alloc::string::String;

/// Every way step_core says no. A position is the 1-based line and column in the text.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// The text breaks the Part 21 grammar here (`what` names the expectation).
    Syntax { line: u32, col: u32, what: &'static str },
    /// A string opened here never closes.
    UnterminatedString { line: u32, col: u32 },
    /// An instance number defined twice (the second definition's position).
    DuplicateInstance { instance: u64, line: u32, col: u32 },
    /// A reference to an instance the file never defines (the reference's position).
    Undefined { instance: u64, line: u32, col: u32 },
    /// An entity name neither in the read subset nor in the table of known AP214 entities it lacks.
    UnknownEntity { entity: String, instance: u64, line: u32, col: u32 },
    /// An entity with the wrong number of parameters.
    ParamCount { entity: String, instance: u64, line: u32, col: u32, expected: usize, found: usize },
    /// A parameter of the wrong type (`index` is 0-based; `want` names the type).
    ParamType { entity: String, instance: u64, line: u32, col: u32, index: usize, want: &'static str },
    /// A known AP214 entity the read subset lacks (B_SPLINE_SURFACE_WITH_KNOTS, BREP_WITH_VOIDS, ...).
    Unsupported { entity: String, instance: u64 },
    /// The unit context cannot be read (named).
    Units { instance: u64, what: &'static str },
    /// The file holds no ADVANCED_BREP_SHAPE_REPRESENTATION.
    NoShape,
    /// brep_core refused the geometry an instance describes (a degenerate placement, a bad B-spline).
    Geometry { instance: u64, refusal: brep_core::Refusal },
    /// The writer cannot represent this Brep honestly (named).
    Unwritable { what: &'static str },
}
