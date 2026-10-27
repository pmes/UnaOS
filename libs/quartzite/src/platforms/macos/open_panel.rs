// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER (OPEN-DIALOG-ONE, LEDGER SR386): the platform open panel on AppKit, the GTK face's `open_dialog` counterpart
//! and `save_panel`'s sibling. The image surface's one bus lane (`image_view::bootstrap_image_surface`) calls [`run`]
//! for an `OpenPanel` addressed to its surface; the panel asks for one file and nothing else (the owner reads it) and
//! the answer goes back as `OpenPanelChose`.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_foundation::{NSArray, NSString};

/// `NSModalResponseOK`.
const MODAL_OK: isize = 1;

/// Show the open panel (app-modal, one file, no folders), offering the files whose names end in one of `filters`
/// (extensions without the dot; empty = any file), and answer `OpenPanelChose { url, path }`: the chosen file's path,
/// or `None` when the person cancelled (or no panel could be made). SAVERECENTOPEN (SR403): opening in `folder` when
/// given (`save_panel::set_directory`, the save panel's own). Main thread only.
pub fn run(url: String, filters: &[String], folder: Option<&str>, syn: &bandy::Synapse) {
    let path = unsafe {
        let panel: *mut AnyObject = msg_send![class!(NSOpenPanel), openPanel];
        if panel.is_null() {
            None
        } else {
            let _: () = msg_send![panel, setCanChooseFiles: Bool::YES];
            let _: () = msg_send![panel, setCanChooseDirectories: Bool::NO];
            let _: () = msg_send![panel, setAllowsMultipleSelection: Bool::NO];
            super::save_panel::set_directory(panel, folder); // SAVERECENTOPEN (SR403)
            if !filters.is_empty() {
                let exts: Vec<Retained<NSString>> = filters.iter().map(|e| NSString::from_str(e)).collect();
                let refs: Vec<&NSString> = exts.iter().map(|e| &**e).collect();
                let types = NSArray::from_slice(&refs);
                let _: () = msg_send![panel, setAllowedFileTypes: &*types];
            }
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
    syn.fire(bandy::SMessage::OpenPanelChose { url, path });
}
