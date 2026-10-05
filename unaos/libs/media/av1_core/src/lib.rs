//! AVCODEC — a from-specification AV1 decoder (AVIF still images and AV1 video), in `no_std` + `alloc`.
//!
//! Source of truth: *AV1 Bitstream & Decoding Process Specification*, v1.0.0 with Errata 1
//! (AOMedia). Module names follow the spec's sections; function and variable names inside follow
//! the spec's pseudo-code so a reader can hold the two side by side.
//!
//! | module        | spec                                                        |
//! |---------------|-------------------------------------------------------------|
//! | [`bits`]      | §4.10 descriptors f(n) su(n) ns(n) le(n) leb128() uvlc()     |
//! | [`obu`]       | §5.3–5.9 OBU, sequence header, frame header, tile info       |
//! | [`avif`]      | AVIF / HEIF (ISO/IEC 23008-12) / ISOBMFF item extraction     |
//! | [`symbol`]    | §8.2 symbol decoder with CDF adaptation                      |
//! | [`cdf`]       | §6.8.2 every CDF array, load/save/frame-end update           |
//! | [`decode`]    | §5.9–5.11 tile, partition, block, mode info, residual        |
//! | [`modeinfo`]  | §5.11.18–33 inter frame mode info and its contexts           |
//! | [`mvpred`]    | §7.9 motion field estimation, §7.10 MV prediction            |
//! | [`inter`]     | §7.11.3 inter prediction (filters, scaling, warp, OBMC, masks) |
//! | [`refs`]      | §7.20 / §7.21 the reference frame store                      |
//! | [`predict`]   | §7.11.2 intra, §7.11.4 palette, §7.11.5 CfL                   |
//! | [`transform`] | §7.12 dequant / reconstruct, §7.13 inverse transforms        |
//! | [`loopfilter`]| §7.14 deblocking                                             |
//! | [`cdef`]      | §7.15 CDEF                                                   |
//! | [`restoration`]| §7.17 loop restoration (Wiener, self-guided)               |
//! | [`image`]     | Y'CbCr → RGBA and the `decode_avif` entry point              |
//!
//! What is NOT decoded yet (owed, see docs/dev/evidence/media-1004/AVCODEC2.md): superres
//! upscaling, film grain synthesis, large-scale tile, AVIF grid/alpha/layers/transform properties.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod avif;
pub mod bits;
pub mod cdef;
pub mod cdf;
pub mod decode;
pub mod image;
pub mod inter;
pub mod loopfilter;
pub mod modeinfo;
pub mod mvpred;
pub mod obu;
pub mod predict;
pub mod refs;
pub mod restoration;
pub mod symbol;
pub mod tables;
pub mod transform;

pub use avif::{avif_payload, Obus};
pub use image::{decode_avif, Image};

/// Every way a decode can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Ran out of bytes / bits.
    Truncated,
    /// The bitstream or container violates the specification.
    Invalid(&'static str),
    /// A valid feature this decoder does not implement yet (see the honest ceiling in the doc).
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => write!(f, "truncated data"),
            Error::Invalid(s) => write!(f, "invalid: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
