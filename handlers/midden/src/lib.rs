use gtk4::prelude::*;
use gtk4::{ScrolledWindow, TextBuffer, TextView, Widget};
use anyhow::Result;
use bandy::{SMessage, BandyMember};
use unafs::FileSystem;

pub fn create_view() -> (Widget, TextBuffer) {
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    let view = TextView::builder().monospace(true).editable(false).build();
    view.add_css_class("console");

    let buffer = view.buffer();
    scroll.set_child(Some(&view));

    (scroll.upcast::<Widget>(), buffer)
}

pub struct Midden {
    // In a real scenario, this might be an Arc<Mutex<FileSystem>>
    // or a channel to the FS actor. For now, we simulate the connection.
    _fs_handle: String,
    current_path: String,
}

impl Default for Midden {
    fn default() -> Self {
        Self::new()
    }
}

impl Midden {
    pub fn new() -> Self {
        Self {
            _fs_handle: "mount:0".to_string(),
            current_path: "/".to_string(),
        }
    }

    /// The core loop: Input -> Logic -> Output (via Bandy)
    pub fn execute(&mut self, command: &str) -> Result<SMessage> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(SMessage::NoOp);
        }

        match parts[0] {
            "ls" => self.list_files(),
            "pwd" => self.print_cwd(),
            "touch" => self.touch_file(parts.get(1)),
            "help" => Ok(SMessage::TerminalOutput("Available: ls, pwd, touch, help".to_string())),
            _ => Ok(SMessage::TerminalOutput(format!("Unknown command: {}", parts[0]))),
        }
    }

    fn list_files(&self) -> Result<SMessage> {
        // TODO: Hook into libs/unafs here
        // For now, we simulate the response
        Ok(SMessage::TerminalOutput(format!("Listing contents of {}... [STUB]", self.current_path)))
    }

    fn print_cwd(&self) -> Result<SMessage> {
        Ok(SMessage::TerminalOutput(self.current_path.clone()))
    }

    fn touch_file(&self, filename: Option<&&str>) -> Result<SMessage> {
        match filename {
            Some(name) => Ok(SMessage::FileSystemEvent(format!("Creating file: {}", name))),
            None => Ok(SMessage::TerminalError("Usage: touch <filename>".to_string())),
        }
    }
}

// Ensure Midden behaves like a nervous system member
impl BandyMember for Midden {
    fn publish(&self, topic: &str, msg: SMessage) -> Result<()> {
        // In the future, Midden will broadcast shell events here
        println!("[MIDDEN] {} -> {:?}", topic, msg);
        Ok(())
    }
}
