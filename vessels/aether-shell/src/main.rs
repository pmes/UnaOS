#[cfg(feature = "gtk")]
mod shell_gtk;

#[cfg(feature = "qt")]
mod shell_qt;

#[cfg(target_os = "macos")]
mod shell_macos;

fn main() {
    #[cfg(feature = "gtk")]
    shell_gtk::run();
    
    #[cfg(all(not(feature = "gtk"), feature = "qt"))]
    shell_qt::run();

    #[cfg(all(not(feature = "gtk"), not(feature = "qt"), target_os = "macos"))]
    shell_macos::run();
    
    #[cfg(all(not(feature = "gtk"), not(feature = "qt"), not(target_os = "macos")))]
    println!("No shell feature enabled. Compile with --features gtk or qt");
}
