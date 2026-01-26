use gneiss_pal::{WaylandApp, AppHandler, MOONSTONE_PURPLE};

struct MoonstoneApp;

impl AppHandler for MoonstoneApp {
    fn update(&mut self) {}

    fn draw(&mut self, buffer: &mut [u32], _width: u32, _height: u32) {
        for pixel in buffer.iter_mut() {
            *pixel = MOONSTONE_PURPLE;
        }
    }
}

fn main() {
    println!("Initializing Moonstone check...");
    let app = WaylandApp::new().expect("Failed to init");
    if let Err(e) = app.run_window(MoonstoneApp) {
        eprintln!("Error: {}", e);
    }
    println!("Check complete.");
}
