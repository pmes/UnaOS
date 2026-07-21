#[cfg(any(target_os = "linux", target_os = "macos"))]
#[cfg(feature = "gtk")]
mod shell_gtk;

#[cfg(feature = "qt")]
mod shell_qt;

#[cfg(target_os = "macos")]
mod shell_macos;

fn main() {
    #[cfg(target_os = "macos")]
    {
        shell_macos::run();
        return;
    }

    #[cfg(feature = "gtk")]
    shell_gtk::run();

    #[cfg(feature = "qt")]
    shell_qt::run();
}
