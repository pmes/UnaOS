//! AETHERCHROME (SR84): the browser's tab strip, painted by GTK. The strip's MODEL is
//! `aether::chrome::tabs` (in the shell); this face only draws what `SMessage::BrowserTabs` says and
//! sends the person's clicks back as `BrowserTabSelect` / `BrowserTabClose` / `BrowserTabNew`.
//! Labels are quartzite's own text (QUARTZFONT, SR64).

use gtk4::prelude::*;
use gtk4::{Box, Button, Orientation, ScrolledWindow};

use super::glyph::GlyphLabel;
use crate::NativeView;
use bandy::{SMessage, Synapse};

/// The longest label a tab shows, in characters (the rest is an ellipsis).
const LABEL_MAX: usize = 24;

fn short(label: &str) -> String {
    if label.chars().count() <= LABEL_MAX {
        label.to_string()
    } else {
        let mut s: String = label.chars().take(LABEL_MAX - 1).collect();
        s.push('…');
        s
    }
}

fn tab_button(id: u64, label: &str, loading: bool, active: bool, synapse: &Synapse) -> gtk4::Widget {
    let row = Box::new(Orientation::Horizontal, 4);
    let select = Button::new();
    let text = if loading { format!("◌ {}", short(label)) } else { short(label) };
    select.set_child(Some(&GlyphLabel::new(&text)));
    select.set_tooltip_text(Some(label));
    if !active {
        select.add_css_class("flat");
    }
    let syn = synapse.clone();
    select.connect_clicked(move |_| syn.fire(SMessage::BrowserTabSelect(id)));
    let close = Button::new();
    close.set_child(Some(&GlyphLabel::new("×")));
    close.add_css_class("flat");
    close.set_tooltip_text(Some("Close tab (Ctrl+W)"));
    let syn = synapse.clone();
    close.connect_clicked(move |_| syn.fire(SMessage::BrowserTabClose(Some(id))));
    row.append(&select);
    row.append(&close);
    if active {
        row.add_css_class("linked");
    }
    row.upcast()
}

/// The strip: one button per tab (click selects, × closes) and a `+` that opens a new tab.
pub fn bootstrap_tab_strip(synapse: Synapse) -> NativeView {
    let outer = Box::new(Orientation::Horizontal, 4);
    outer.set_margin_start(4);
    outer.set_margin_end(4);
    outer.set_margin_top(2);
    let tabs = Box::new(Orientation::Horizontal, 2);
    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .hexpand(true)
        .child(&tabs)
        .build();
    let new_tab = Button::new();
    new_tab.set_child(Some(&GlyphLabel::new("+")));
    new_tab.add_css_class("flat");
    new_tab.set_tooltip_text(Some("New tab (Ctrl+T)"));
    let syn = synapse.clone();
    new_tab.connect_clicked(move |_| syn.fire(SMessage::BrowserTabNew));
    outer.append(&scroller);
    outer.append(&new_tab);

    let mut rx = synapse.subscribe();
    let syn = synapse.clone();
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                Ok(SMessage::BrowserTabs { active, tabs: list }) => {
                    while let Some(child) = tabs.first_child() {
                        tabs.remove(&child);
                    }
                    for (id, label, loading) in &list {
                        tabs.append(&tab_button(*id, label, *loading, *id == active, &syn));
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    outer.upcast()
}
