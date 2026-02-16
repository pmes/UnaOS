pub use gneiss_pal;
pub use quartzite;

// Facade for the Trinity
pub mod prelude {
    pub use gneiss_pal::{AppHandler, DashboardState, Event, GuiUpdate};
    pub use quartzite::Backend;
    pub use gtk4::prelude::*;
    pub use gtk4::{self, ApplicationWindow, Widget, Window};
}
