#[derive(Debug)]
pub enum LuxError {
    /// Not enough data to parse or access slice
    BufferTooSmall,
    /// The file does not appear to be a valid TIFF/ARW
    InvalidMagic,
    /// Unrecognized or unsupported endianness indicator
    UnsupportedEndianness,
    /// Could not find the required tags or directories
    MissingData,
    /// Compression type is not supported yet
    UnsupportedCompression(u16),
    /// CFA pattern not supported
    UnsupportedCFA,
    /// Data is corrupt
    CorruptData,
}

impl std::fmt::Display for LuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LuxError::BufferTooSmall => write!(f, "Buffer is too small"),
            LuxError::InvalidMagic => write!(f, "Invalid magic number, expected TIFF"),
            LuxError::UnsupportedEndianness => write!(f, "Unsupported endianness"),
            LuxError::MissingData => write!(f, "Missing required tags or data"),
            LuxError::UnsupportedCompression(c) => write!(f, "Unsupported compression scheme: {}", c),
            LuxError::UnsupportedCFA => write!(f, "Unsupported CFA pattern"),
            LuxError::CorruptData => write!(f, "Data is corrupt"),
        }
    }
}

impl std::error::Error for LuxError {}
