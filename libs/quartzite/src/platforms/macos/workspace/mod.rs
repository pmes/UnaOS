// SPDX-License-Identifier: GPL-3.0-or-later

//! The Workspace Assembly
//!
//! Brings together the `Sidebar` (Left Pane) and `Comms` (Right Pane)
//! into a billet-aluminum `NSSplitView`.

pub mod sidebar;
pub mod comms;

use objc2::rc::Retained;
use objc2::{msg_send};
use objc2_app_kit::{NSSplitView, NSView};
use objc2_foundation::MainThreadOnly;

use crate::platforms::macos::workspace::sidebar::{create_sidebar, SidebarRefs};
use crate::platforms::macos::workspace::comms::{create_comms_pane, CommsRefs};

/// Holds the strong `Retained` references to the entire workspace hierarchy.
/// This will live inside the `RefCell` of the `AppDelegateIvars` to prevent premature deallocation.
pub struct WorkspaceRefs {
    pub split_view: Retained<NSSplitView>,
    pub sidebar: SidebarRefs,
    pub comms: CommsRefs,
}

pub fn create_workspace() -> WorkspaceRefs {
    let _mtm = MainThreadOnly::new();

    // Create the Split View
    let split_view: Retained<NSSplitView> = unsafe { msg_send![NSSplitView::class(), alloc] };
    let split_view: Retained<NSSplitView> = unsafe { msg_send![split_view, initWithFrame: foundation::NSRect::ZERO] };

    unsafe {
        let _: () = msg_send![&split_view, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&split_view, setVertical: true];
        let _: () = msg_send![&split_view, setDividerStyle: 2_isize]; // NSSplitViewDividerStyleThin
    }

    // Assemble the panes
    let sidebar_refs = create_sidebar();
    let comms_refs = create_comms_pane();

    unsafe {
        // We cast NSScrollView to NSView to add it to the split view
        let sv_view: Retained<NSView> = Retained::cast::<NSView>(sidebar_refs.scroll_view.clone());
        let _: () = msg_send![&split_view, addArrangedSubview: &*sv_view];
        let _: () = msg_send![&split_view, addArrangedSubview: &*comms_refs.container];

        // Ensure the sidebar doesn't compress indefinitely
        let _: () = msg_send![&sv_view, setTranslatesAutoresizingMaskIntoConstraints: false];
    }

    WorkspaceRefs {
        split_view,
        sidebar: sidebar_refs,
        comms: comms_refs,
    }
}
