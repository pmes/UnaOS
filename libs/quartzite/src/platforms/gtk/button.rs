use gtk4::prelude::*;
use gtk4::Button;
use crate::NativeView;
use bandy::{SMessage, Synapse};

pub fn bootstrap_button(
    label: &str,
    action_msg: SMessage,
    synapse: Synapse,
) -> NativeView {
    let btn = Button::with_label(label);
    
    let syn = synapse.clone();
    btn.connect_clicked(move |_| {
        syn.fire(action_msg.clone());
    });
    
    btn.into()
}
