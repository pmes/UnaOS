use gtk4::prelude::*;
use super::glyph::GlyphEntry;
use crate::NativeView;
use bandy::{SMessage, Synapse};
use crate::tetra::TextAction;

pub fn bootstrap_text_field(
    placeholder: &str,
    secret: bool,
    text_action: TextAction,
    synapse: Synapse,
) -> NativeView {
    // QUARTZFONT (SR64): the field is quartzite's own editor, its text measured and painted by font_core.
    let entry = GlyphEntry::new(placeholder);
    entry.set_secret(secret); // LANDINGSECRETFIELD (SR432): a secret field draws bullets, refuses copy
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
        if secret { e.set_text(""); } // SECRETFIELD-CLEAR (SR449): a secret field's text is gone once its Enter is taken (the String above already moved into msg)
    });
    
    entry.into()
}
