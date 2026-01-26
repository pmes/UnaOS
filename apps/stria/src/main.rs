mod engine;
mod ui;

use gneiss_pal::{WaylandApp, WindowEvent, KeyCode};

fn main() {
    println!(":: Stria Media System v0.1 ::");

    // 1. Ignite the Engine (The Stevedore)
    let cores = num_cpus::get();
    println!(":: ENGINE ONLINE ({} Cores Ready) ::", cores);

    // 2. Launch the Interface (The Gneiss PAL)
    println!(":: LINKING GNEISS PAL... ::");
    let mut app = WaylandApp::new()
        .expect("Failed to initialize Wayland connection");

    // 3. Open the Viewport
    let _window = app.create_window(1280, 720, "Stria [Engine: Idle]")
        .expect("Failed to create window");

    println!(":: SYSTEM ONLINE. PRESS ESC TO SHUTDOWN. ::");

    // 4. Enter the Event Loop
    app.run(move |event| {
        match event {
            WindowEvent::CloseRequested => {
                println!(":: SHUTDOWN SEQUENCE ::");
                std::process::exit(0);
            }
            WindowEvent::KeyboardInput { key: KeyCode::Escape, .. } => {
                println!(":: EMERGENCY STOP ::");
                std::process::exit(0);
            }
            _ => {}
        }
    });
}
