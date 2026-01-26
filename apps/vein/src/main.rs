use gneiss_pal::{WaylandApp, AppHandler, text};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use dotenvy::dotenv;

struct State {
    status_text: String,
    #[allow(dead_code)]
    network_initialized: bool,
}

struct VeinApp {
    state: Arc<Mutex<State>>,
}

impl AppHandler for VeinApp {
    fn update(&mut self) {
        // Future: Poll for specific updates if needed
    }

    fn draw(&mut self, buffer: &mut [u32], width: u32, height: u32) {
        // Fill Background with Una Blue
        // 0xFF00AAFF is ARGB (assuming softbuffer uses 0RGB or ARGB, usually top byte ignored or alpha)
        // User specified 0x00aaff.
        let bg_color = 0x00aaff;
        for pixel in buffer.iter_mut() {
            *pixel = bg_color;
        }

        let state = self.state.lock().unwrap();

        // Title
        text::draw_text(
            buffer,
            width,
            height,
            "UnaOS Virtual Office: ONLINE",
            50,
            50,
            0xFFFFFFFF, // White
        );

        // Network Status
        text::draw_text(
            buffer,
            width,
            height,
            &state.status_text,
            50,
            90, // A bit lower
            0xFFFFFFFF, // White
        );
    }
}

fn main() {
    // Load environment variables
    dotenv().ok();

    println!(":: VEIN :: Booting...");

    // Shared State
    let state = Arc::new(Mutex::new(State {
        status_text: "Initializing Network Stack...".to_string(),
        network_initialized: false,
    }));

    let state_for_bg = state.clone();

    // Spawn Background Async Runtime
    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");

        rt.block_on(async {
            println!(":: VEIN :: Async Core Starting...");

            // Initialize Reqwest Client
            let _client = reqwest::Client::new();

            // Simulate startup delay
            tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

            {
                let mut s = state_for_bg.lock().unwrap();
                s.status_text = "Network Stack Initialized".to_string();
                s.network_initialized = true;
            }
            println!(":: VEIN :: Network Stack Initialized");

            // Keep the runtime alive for future tasks
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });
    });

    // Start UI
    println!(":: VEIN :: Initializing Graphical Interface...");
    let app = WaylandApp::new().expect("Failed to initialize PAL");
    let handler = VeinApp { state };

    if let Err(e) = app.run_window(handler) {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Shutdown.");
}
