use gneiss_pal::{GneissPal, HostPal, MOONSTONE_PURPLE, Event};

fn main() {
    // 1. Initialize the Body
    let width = 800;
    let height = 600;
    let mut pal = HostPal::new(width, height);

    println!("Initializing Moonstone check...");

    // 2. The Main Loop (The Heartbeat)
    loop {
        // A. Input
        if let Event::Quit = pal.poll_event() {
            break;
        }

        // B. Logic (Fill Screen with Moonstone Purple)
        for y in 0..height {
            for x in 0..width {
                pal.draw_pixel(x as u32, y as u32, MOONSTONE_PURPLE);
            }
        }

        // C. Render
        pal.render();
    }

    println!("Check complete.");
}
