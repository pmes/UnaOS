#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
use std::string::String;
#[cfg(feature = "std")]
use std::vec::Vec;

pub const MOONSTONE_PURPLE: u32 = 0x2C003E;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Enter,
    Backspace,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Quit,
    Timer,
    KeyDown(KeyCode),
    Char(char),
    Mouse { x: i32, y: i32 },

    // Command Deck Events
    Input(String),         // The "Intelligent" full message
    TemplateAction(usize), // Clicking a button in the Right Pane
    NavSelect(usize),      // Clicking a row in the Left Pane

    None,
    Unknown,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ViewMode {
    Comms,
    Wolfpack,
}

#[derive(Clone, Debug)]
pub struct DashboardState {
    pub mode: ViewMode,
    // Left Pane
    pub nav_items: Vec<String>,
    pub active_nav_index: usize,
    // Center Pane (Comms)
    pub console_output: String,
    // Right Pane
    pub actions: Vec<String>,
}

// THE KERNEL INTERFACE
pub trait GneissPal {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32);
    fn poll_event(&mut self) -> Event;
    fn render(&mut self);
    fn width(&self) -> u32;
    fn height(&self) -> u32;

    fn clear_screen(&mut self, color: u32) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                self.draw_pixel(x, y, color);
            }
        }
    }

    fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for row in 0..h {
            for col in 0..w {
                self.draw_pixel((x + col) as u32, (y + row) as u32, color);
            }
        }
    }

    fn draw_text(&mut self, _x: usize, _y: usize, _text: &str, _color: u32) {
    }
}

pub trait AppHandler {
    fn handle_event(&mut self, event: Event);
    fn view(&self) -> DashboardState;
}

#[cfg(feature = "std")]
pub mod backend;

#[cfg(feature = "std")]
pub mod text;

#[cfg(feature = "std")]
pub use backend::Backend; // Renamed from EventLoop in "The Great Evolution"

// Compatibility alias if needed, though vein uses EventLoop now.
#[cfg(feature = "std")]
pub use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
