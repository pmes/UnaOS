use gtk4::prelude::*;
use gtk4::Entry;
use crate::NativeView;
use bandy::{SMessage, Synapse};
use crate::tetra::TextAction;

pub fn bootstrap_text_field(
    placeholder: &str,
    text_action: TextAction,
    synapse: Synapse,
) -> NativeView {
    let entry = Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    
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
