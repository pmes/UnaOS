use gneiss_pal::backend;

fn main() {
    println!("--------------------------------");
    println!(":: GNEISS PAL // CONTROL DECK ::");
    println!("--------------------------------");
    println!("STATUS:   ONLINE");
    println!("SYSTEM:   HOSTED (Linux)");
    println!("--------------------------------");

    // Initialize the backend systems
    backend::init();
}
