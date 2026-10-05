use gtk4::prelude::*;
use gtk4::Button;
use crate::NativeView;
use bandy::{SMessage, Synapse};

pub fn bootstrap_button(
    label: &str,
    action_msg: SMessage,
    synapse: Synapse,
) -> NativeView {
    // QUARTZFONT (SR64): GTK draws the button's frame; its label is quartzite's own text (font_core).
    let btn = Button::new();
    btn.set_child(Some(&super::glyph::GlyphLabel::new(label)));
    // what Button::with_label adds: the theme's text-button padding
    btn.add_css_class("text-button");
    
    let syn = synapse.clone();
    btn.connect_clicked(move |_| {
        syn.fire(action_msg.clone());
    });
    
    btn.into()
}
