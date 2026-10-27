// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER (SAVEDIALOG, LEDGER SR364): the platform save panel on GTK 4 (`FileDialog::save`, GTK 4.10+), the AppKit
//! face's `save_panel` counterpart. The image surface's one bus lane (`image_view::bootstrap_image_surface`) calls
//! [`run`] for a `SavePanel` addressed to its surface; the dialog asks for a path and nothing else (the owner writes
//! the bytes through gneiss_pal's ONE writer) and the answer goes back as `SavePanelChose`.

use bandy::{SMessage, Synapse};
use gtk4::prelude::*;

/// Show the save dialog over `widget`'s window, seeded with `name`, and answer `SavePanelChose { url, path }`: the
/// chosen file's path, or `None` when the dialog was dismissed (or failed). Main context only.
pub fn run(widget: &impl IsA<gtk4::Widget>, url: String, name: &str, syn: Synapse) {
    let dialog = gtk4::FileDialog::new();
    dialog.set_initial_name(Some(name));
    let parent = widget.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
    gtk4::glib::spawn_future_local(async move {
        let path = dialog.save_future(parent.as_ref()).await.ok().and_then(|f| f.path()).map(|p| p.to_string_lossy().into_owned());
        syn.fire(SMessage::SavePanelChose { url, path });
    });
}
