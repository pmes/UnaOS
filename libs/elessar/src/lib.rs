pub use gneiss_pal;
pub use quartzite;

// Facade for the Trinity
pub mod prelude {
    pub use gneiss_pal::{AppHandler, DashboardState, Event, GuiUpdate};
    pub use gtk4::prelude::*;
    pub use gtk4::{self, ApplicationWindow, Widget, Window};
    pub use quartzite::Backend;
}

use std::path::Path;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Spline {
    /// The Monolith itself. Defined by the presence of MEMORIA.md.
    UnaOS,
    /// A Rust Crate (Cargo.toml).
    Rust,
    /// A Node/Web Project (package.json).
    Web,
    /// A Python Project (requirements.txt / pyproject.toml).
    Python,
    /// Unknown territory.
    Void,
}

pub struct Context {
    pub path: std::path::PathBuf,
    pub spline: Spline,
}

impl Context {
    /// Scans the given path to determine its Spline.
    pub fn new(path: &Path) -> Self {
        let spline = detect_spline(path);
        Self {
            path: path.to_path_buf(),
            spline,
        }
    }
}

fn detect_spline(path: &Path) -> Spline {
    if path.join("MEMORIA.md").exists() {
        return Spline::UnaOS;
    }
    if path.join("Cargo.toml").exists() {
        return Spline::Rust;
    }
    if path.join("package.json").exists() {
        return Spline::Web;
    }
    if path.join("requirements.txt").exists() || path.join("pyproject.toml").exists() {
        return Spline::Python;
    }

    Spline::Void
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_self_recognition() {
        // We assume we are running tests from inside libs/elessar or workspace root.
        // Let's find the workspace root.
        let mut current = env::current_dir().unwrap();

        // Walk up until we find MEMORIA.md or hit root
        loop {
            if current.join("MEMORIA.md").exists() {
                let ctx = Context::new(&current);
                assert_eq!(ctx.spline, Spline::UnaOS);
                return;
            }
            if !current.pop() {
                break;
            }
        }

        // If we didn't find it, we might be in a CI environment where we just check for Cargo.toml
        let ctx = Context::new(&env::current_dir().unwrap());
        // At minimum, we should be Rust
        assert!(matches!(ctx.spline, Spline::Rust | Spline::UnaOS));
    }
}
