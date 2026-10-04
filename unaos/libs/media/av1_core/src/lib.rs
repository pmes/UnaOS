//! AVCODEC — a from-specification AV1 decoder, intra first (AVIF still images), in `no_std` + `alloc`.
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
//! | [`cdf`]       | §7.20 / §9.4 default CDF contexts (tables generated)         |
//! | [`decode`]    | §5.9–5.11 tile, partition, block, mode info, residual        |
//! | [`predict`]   | §7.11.2 intra, §7.11.4 palette, §7.11.5 CfL                   |
//! | [`transform`] | §7.12 dequant / reconstruct, §7.13 inverse transforms        |
//! | [`loopfilter`]| §7.14 deblocking                                             |
//! | [`cdef`]      | §7.15 CDEF                                                   |
//! | [`restoration`]| §7.17 loop restoration (Wiener, self-guided)               |
//! | [`image`]     | Y'CbCr → RGBA and the `decode_avif` entry point              |
//!
//! What is NOT decoded (owed, see docs/dev/evidence/media-1004/AVCODEC.md): inter frames
//! (motion vectors, reference frames, warped/global motion, OBMC, compound), intra block copy,
//! superres upscaling, film grain synthesis, AVIF grid/alpha/transform properties.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod avif;
pub mod bits;
pub mod obu;
pub mod tables;

pub use avif::{avif_payload, Obus};

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
