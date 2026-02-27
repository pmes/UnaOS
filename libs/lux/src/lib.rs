pub mod error;
pub mod parser;

pub use error::LuxError;
pub use parser::parse_arw;

/// The final uncompressed, demosaiced image buffer.
/// Linear RGB (f32) structure, tightly packed.
pub struct RgbBuffer {
    /// Width of the image in pixels
    pub width: u32,
    /// Height of the image in pixels
    pub height: u32,
    /// Tightly packed linear RGB buffer (R, G, B, R, G, B...)
    pub pixels: Vec<f32>,
}
