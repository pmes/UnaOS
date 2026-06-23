cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[macro_use]
        pub mod x86_64;
        pub use x86_64::*;
    } else if #[cfg(target_arch = "aarch64")] {
        #[macro_use]
        pub mod aarch64;
        pub use aarch64::*;
    }
}

// Common architectural traits or functions can go here
pub trait Architecture {
    fn init();
    fn halt() -> !;
}
