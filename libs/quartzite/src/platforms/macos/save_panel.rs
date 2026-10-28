// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER (SAVEDIALOG, LEDGER SR364): the platform save panel on AppKit, the GTK face's `save_dialog` counterpart. The
//! image surface's one bus lane (`image_view::bootstrap_image_surface`) calls [`run`] for a `SavePanel` addressed to its
//! surface; the panel asks for a path and nothing else (the owner writes the bytes through gneiss_pal's ONE writer) and
//! the answer goes back as `SavePanelChose`. Aether's page view and facet-view share it.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_foundation::NSString;

/// `NSModalResponseOK`.
const MODAL_OK: isize = 1;

/// Show the save panel seeded with `name` (app-modal; opening in `folder` when given: SAVERECENT, SR391) and answer
/// `SavePanelChose { url, path }`: the chosen file's path, or `None` when the person cancelled (or no panel could be
/// made). Main thread only.
pub fn run(url: String, name: &str, folder: Option<&str>, syn: &bandy::Synapse) {
    let path = unsafe {
        let panel: *mut AnyObject = msg_send![class!(NSSavePanel), savePanel];
        if panel.is_null() {
            None
        } else {
            let seed = NSString::from_str(name);
            let _: () = msg_send![panel, setNameFieldStringValue: &*seed];
            let _: () = msg_send![panel, setCanCreateDirectories: Bool::YES];
            // SAVERECENT (SR391): the initial directory.
            set_directory(panel, folder);
            let response: isize = msg_send![panel, runModal];
            if response != MODAL_OK {
                None
            } else {
                let chosen: *mut AnyObject = msg_send![panel, URL];
                if chosen.is_null() {
                    None
                } else {
                    let p: Option<Retained<NSString>> = msg_send![chosen, path];
                    p.map(|p| p.to_string())
                }
            }
        }
    };
    syn.fire(bandy::SMessage::SavePanelChose { url, path });
}

/// SAVERECENT (SR391) / SAVERECENTOPEN (SR403): open `panel` (an NSSavePanel, or its subclass NSOpenPanel) in `folder`
/// when given (`setDirectoryURL:`); the save panel and the open panel share it. Main thread only; `panel` non-null.
pub(super) unsafe fn set_directory(panel: *mut AnyObject, folder: Option<&str>) {
    let Some(f) = folder else { return };
    let dir = NSString::from_str(f);
    unsafe {
        let u: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: &*dir, isDirectory: Bool::YES];
        if !u.is_null() {
            let _: () = msg_send![panel, setDirectoryURL: u];
        }
    }
}
