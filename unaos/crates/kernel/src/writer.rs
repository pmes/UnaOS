use lazy_static::lazy_static;
use spin::Mutex;

// STUB: We are disabling FrameBuffer graphics for the Bootloader 0.9 downgrade.
// We will rely on Serial output for the first successful boot.

lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {});
}

pub struct Writer;

impl Writer {
    // Stub init function that does nothing
    // We accept arguments that match main.rs but ignore them
    pub fn init(&mut self, _buffer: &'static mut [u8], _info: impl AnyIgnore) {
        // No-op
    }

    pub fn width(&self) -> usize {
        0
    }
    pub fn height(&self) -> usize {
        0
    }
    pub fn write_pixel(&mut self, _x: usize, _y: usize, _color: u32) {}
}

// Helper to swallow the type mismatch in main.rs without editing main.rs again
pub trait AnyIgnore {}
impl<T> AnyIgnore for T {}
