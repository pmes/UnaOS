use gneiss_pal::WaylandApp;

fn main() {
    println!(":: VEIN :: Initializing Graphical Interface via Gneiss PAL...");

    let app = WaylandApp::new().expect("Failed to initialize PAL");

    // This will open the window and block until closed
    if let Err(e) = app.run_window() {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Terminated.");
}
