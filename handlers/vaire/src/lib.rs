use gtk4::prelude::*;
use gtk4::{Box, Orientation, Label, Align, Widget};

pub fn create_view() -> Widget {
    let vaire_box = Box::new(Orientation::Vertical, 10);
    vaire_box.set_valign(Align::Center);
    vaire_box.append(&Label::new(Some("No Git Repository Detected")));
    vaire_box.upcast::<Widget>()
}
