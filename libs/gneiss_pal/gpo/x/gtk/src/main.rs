use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use gneiss_pal::{App as CoreApp, Platform, Plugin};

struct GtkPlatform {
    window: adw::ApplicationWindow,
}

impl Platform for GtkPlatform {
    fn set_title(&self, title: &str) {
        self.window.set_title(Some(title));
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// Example Plugin
struct HelloPlugin;
impl Plugin for HelloPlugin {
    fn on_init(&mut self, platform: &dyn Platform) {
        platform.set_title("Hello from GTK Skeleton");
        println!("Plugin Initialized!");
    }
    fn on_update(&mut self, _platform: &dyn Platform) {}
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn main() {
    let app = adw::Application::builder()
        .application_id("com.template.gtk")
        .build();

    app.connect_activate(|app| {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Template App")
            .default_width(800)
            .default_height(600)
            .build();

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let label = gtk4::Label::new(Some("Universal App Bones - GTK"));
        label.set_vexpand(true);
        label.set_hexpand(true);
        content.append(&label);

        // Use set_content if available, or set_child for newer Adw
        window.set_content(Some(&content));
        window.present();

        // Initialize Core App and Platform
        let platform = GtkPlatform { window: window.clone() };
        let mut core_app = CoreApp::new();

        core_app.register_plugin(HelloPlugin);
        core_app.init(&platform);

        // Keep core_app alive? In a real app we might store it in a RefCell/Rc if needed for callbacks
        // For now, it lives as long as this closure scope (which runs once).
        // In a real loop, we'd move it to a struct attached to the window.
    });

    app.run_with_args::<&str>(&[]);
}
