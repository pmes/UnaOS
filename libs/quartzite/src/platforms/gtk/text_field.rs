use gtk4::prelude::*;
use super::glyph::GlyphEntry;
use crate::NativeView;
use bandy::{SMessage, Synapse};
use crate::tetra::TextAction;

pub fn bootstrap_text_field(
    placeholder: &str,
    text_action: TextAction,
    synapse: Synapse,
) -> NativeView {
    // QUARTZFONT (SR64): the field is quartzite's own editor, its text measured and painted by font_core.
    let entry = GlyphEntry::new(placeholder);
    entry.set_hexpand(true);
    
    let syn = synapse.clone();
    entry.connect_activate(move |e| {
        let text = e.text().to_string();
        let msg = match text_action {
            TextAction::OpenDocument => SMessage::OpenDocument { url: text },
            TextAction::ConsoleInput => SMessage::ConsoleInput(text),
            TextAction::BrowserText => SMessage::BrowserText(text),
        };
        syn.fire(msg);
    });
    
    entry.into()
}
