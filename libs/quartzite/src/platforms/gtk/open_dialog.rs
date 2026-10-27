// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER (OPEN-DIALOG-ONE, LEDGER SR386): the platform open panel on GTK 4 (`FileDialog::open`, GTK 4.10+), the
//! AppKit face's `open_panel` counterpart and `save_dialog`'s sibling. The image surface's one bus lane
//! (`image_view::bootstrap_image_surface`) calls [`run`] for an `OpenPanel` addressed to its surface and the answer goes
//! back as `OpenPanelChose`; a face's own button that wants a file (the workspace attach button) awaits [`choose`]. The
//! dialog asks for a path and nothing else: the owner reads the file.

use std::path::PathBuf;

use bandy::{SMessage, Synapse};
use gtk4::prelude::*;

/// Show the open dialog over `parent`, offering the files whose names end in one of `filters` (extensions without the
/// dot; empty = any file), and answer the chosen file's path, or `None` when it was dismissed (or failed). Main context.
pub async fn choose(parent: Option<&gtk4::Window>, filters: &[String]) -> Option<PathBuf> {
    choose_in(parent, filters, None).await
}

/// [`choose`], opening in `folder` when given (SAVERECENTOPEN, SR403: the Aether chrome's Ctrl/Cmd+O sends the recent
/// folder, as the save dialog's `set_initial_folder` does for a Save As).
pub async fn choose_in(parent: Option<&gtk4::Window>, filters: &[String], folder: Option<&str>) -> Option<PathBuf> {
    let dialog = gtk4::FileDialog::new();
    if let Some(f) = folder {
        dialog.set_initial_folder(Some(&gtk4::gio::File::for_path(f)));
    }
    if !filters.is_empty() {
        let filter = gtk4::FileFilter::new();
        for ext in filters {
            filter.add_suffix(ext);
        }
        let list = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
        list.append(&filter);
        dialog.set_filters(Some(&list));
        dialog.set_default_filter(Some(&filter));
    }
    dialog.open_future(parent).await.ok().and_then(|f| f.path())
}

/// Show the open dialog over `widget`'s window (opening in `folder` when given: SAVERECENTOPEN, SR403) and answer
/// `OpenPanelChose { url, path }`: the chosen file's path, or `None` when the dialog was dismissed (or failed). Main
/// context only.
pub fn run(widget: &impl IsA<gtk4::Widget>, url: String, filters: Vec<String>, folder: Option<String>, syn: Synapse) {
    let parent = widget.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
    gtk4::glib::spawn_future_local(async move {
        let path = choose_in(parent.as_ref(), &filters, folder.as_deref()).await.map(|p| p.to_string_lossy().into_owned());
        syn.fire(SMessage::OpenPanelChose { url, path });
    });
}
