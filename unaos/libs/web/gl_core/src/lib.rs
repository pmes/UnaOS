// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether (CODEX §2, Web) — seam: shared-core (a `no_std` lib both rings link; AETHER.ELF later).
//!
//! WEBGL1 (LEDGER SR136): a software OpenGL ES 2.0 pipeline with WebGL 1.0's rules, written from the
//! specifications — the OpenGL ES 2.0.25 Full Specification ("ES 2.0 §n"), the OpenGL ES Shading Language
//! 1.00.17 ("GLSL ES §n") and the WebGL 1.0 Specification (cited as WebGL 1.0 "<section title>"). No system GL, no ANGLE (R83).
//!
//! * [`consts`] — the WebGL 1.0 IDL constant table; [`context`] — the state machine and its error semantics;
//!   [`object`] / [`texture`] / [`framebuffer`] — the objects, completeness.
//! * [`glsl`] — GLSL ES 1.00: preprocessor, parser, semantics; [`program`] — shader and program objects.
//!
//! `no_std` + `alloc`, no dependencies, no `unsafe`.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod consts;
pub mod context;
pub mod fmath;
pub mod framebuffer;
pub mod glsl;
pub mod object;
pub mod program;
pub mod texture;

pub use context::{ActiveInfo, Attributes, Context, DrawStats, ObjKind, Param, PrecisionFormat, UniformLoc};
