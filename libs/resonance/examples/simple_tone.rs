use resonance::{AudioEngine, create_test_graph};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), anyhow::Error> {
    println!("Initializing Resonance Audio Engine...");

    // Create the test graph (Osc -> Gain)
    let graph = create_test_graph();

    // Start the engine
    // This moves the graph into the audio thread.
    let _engine = AudioEngine::new(graph)?;

    println!("Audio Engine started. Playing 440Hz tone...");
    println!("Press Ctrl+C to stop.");

    // Keep the main thread alive to let the audio stream run.
    // In a real app, this would be the main event loop.
    thread::sleep(Duration::from_secs(5));

    println!("Stopping audio...");
    Ok(())
}
