use std::any::Any;
use std::env;

/// Interface that the platform (frontend) must implement to expose capabilities to the Core.
pub trait Platform: Any {
    fn set_title(&self, title: &str);
    fn as_any(&self) -> &dyn Any;
}

/// Interface for plugins (like a Video Player) to hook into the application lifecycle.
pub trait Plugin {
    fn on_init(&mut self, platform: &dyn Platform);
    fn on_update(&mut self, platform: &dyn Platform);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// The Central Application logic that manages plugins and communicates with the Platform.
pub struct App {
    plugins: Vec<Box<dyn Plugin>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register_plugin<P: Plugin + 'static>(&mut self, plugin: P) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn init(&mut self, platform: &dyn Platform) {
        for plugin in &mut self.plugins {
            plugin.on_init(platform);
        }
    }

    pub fn update(&mut self, platform: &dyn Platform) {
        for plugin in &mut self.plugins {
            plugin.on_update(platform);
        }
    }
}

/// Helper function to parse CLI arguments for typical media apps.
/// Returns (video_path, debug_mode).
/// Skips the executable name.
/// Handles `--debug` flag.
/// Takes the first non-flag argument as the video path.
pub fn simple_arg_parse() -> (Option<String>, bool) {
    let args: Vec<String> = env::args().collect();
    let debug_mode = args.iter().any(|a| a == "--debug");

    let path = args.iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .cloned();

    (path, debug_mode)
}
