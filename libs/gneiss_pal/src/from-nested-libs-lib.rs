#![no_std]

pub const MOONSTONE_PURPLE: u32 = 0x2C003E;

pub enum Event {
    Quit,
    Key(char),
    None,
}

pub trait GneissPal {
    /// The Sacred Command: Draw a pixel to the buffer.
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32);

    /// The Senses: Check for user input.
    fn poll_event(&mut self) -> Event;

    /// The Breath: Flush the buffer to the screen.
    fn render(&mut self);
}

#[cfg(feature = "std")]
pub mod host;

#[cfg(feature = "std")]
pub use host::HostPal;
