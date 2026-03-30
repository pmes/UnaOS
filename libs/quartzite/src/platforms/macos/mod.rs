// SPDX-License-Identifier: GPL-3.0-or-later

//! The macOS Embassy.
//!
//! Exclusively relies on pure `objc2` and `objc2-app-kit`.
//! Absolutely no intermediate abstractions or UI frameworks beyond raw Apple libraries.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2::runtime::{ProtocolObject, NSObjectProtocol, Sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSResponder, NSWindow, NSWindowStyleMask,
    NSBackingStoreType, NSWindowDelegate, NSToolbar, NSView
};
use objc2_foundation::{NSObject, NSNotification, NSString, NSRect, NSPoint, NSSize};
use std::cell::RefCell;

use crate::platforms::macos::toolbar::{create_toolbar, ToolbarDelegate};
use crate::platforms::macos::workspace::{create_workspace, WorkspaceRefs};
use crate::platforms::macos::spline::initialize_spline;
use bandy::synapse::Synapse;

pub mod spline;
pub mod toolbar;
pub mod workspace;

/// The memory-safe state container for the Application Delegate.
/// We store `Retained<T>` references wrapped in `RefCell` to prevent dangling pointers.
pub struct AppDelegateIvars {
    pub window: RefCell<Option<Retained<NSWindow>>>,
    pub toolbar_delegate: RefCell<Option<Retained<ToolbarDelegate>>>,
    pub workspace_refs: RefCell<Option<WorkspaceRefs>>,
    pub synapse: RefCell<Option<Synapse>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaAppDelegate"]
    #[ivars = AppDelegateIvars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = MainThreadOnly::new();

            // 1. Kickstart the GCD Spline to receive cross-thread UI updates
            if let Some(synapse) = self.ivars().synapse.borrow().as_ref() {
                initialize_spline(synapse.clone());
            }

            // 2. Build the Billet-Aluminum Window
            let window: Retained<NSWindow> = unsafe { msg_send![NSWindow::class(), alloc] };
            let window: Retained<NSWindow> = unsafe {
                msg_send![
                    window,
                    initWithContentRect: NSRect {
                        origin: NSPoint { x: 0.0, y: 0.0 },
                        size: NSSize { width: 1024.0, height: 768.0 },
                    },
                    styleMask: (NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable).bits(),
                    backing: NSBackingStoreType::Buffered.0 as u64, // NSBackingStoreBuffered
                    defer: false
                ]
            };

            unsafe {
                let title = NSString::from_str("UnaOS");
                let _: () = msg_send![&window, setTitle: &*title];
                let _: () = msg_send![&window, setToolbarStyle: 2_isize]; // NSWindowToolbarStyleUnifiedCompact
                let _: () = msg_send![&window, setTitleVisibility: 1_isize]; // NSWindowTitleHidden
            }

            // 3. Assemble and attach the Toolbar
            let (toolbar, tb_delegate) = create_toolbar();
            unsafe {
                let _: () = msg_send![&window, setToolbar: &*toolbar];
            }
            *self.ivars().toolbar_delegate.borrow_mut() = Some(tb_delegate);

            // 4. Assemble and attach the Workspace Hierarchy
            let workspace = create_workspace();
            unsafe {
                let sv_view: Retained<NSView> = Retained::cast::<NSView>(workspace.split_view.clone());
                let _: () = msg_send![&window, setContentView: &*sv_view];
                let _: () = msg_send![&window, makeKeyAndOrderFront: None::<&objc2::runtime::AnyObject>];
            }
            *self.ivars().workspace_refs.borrow_mut() = Some(workspace);

            // Retain the Window
            *self.ivars().window.borrow_mut() = Some(window);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

pub struct Backend {
    pub synapse: Synapse,
}

impl Backend {
    pub fn new(synapse: Synapse) -> Self {
        Self { synapse }
    }

    /// The definitive entry point for the macOS UI.
    /// Hijacks the main thread and hands control to NSApp.
    pub fn execute(&self) -> Result<(), String> {
        let mtm = MainThreadOnly::new();

        let app: Retained<NSApplication> = unsafe { msg_send![NSApplication::class(), sharedApplication] };

        let delegate: Retained<AppDelegate> = unsafe { msg_send![AppDelegate::class(), alloc] };
        let delegate: Retained<AppDelegate> = unsafe { msg_send![delegate, init] };

        // Populate Ivars
        *delegate.ivars().synapse.borrow_mut() = Some(self.synapse.clone());

        let proto: &ProtocolObject<dyn NSApplicationDelegate> = ProtocolObject::from_ref(&*delegate);
        unsafe {
            let _: () = msg_send![&app, setDelegate: proto];
            let _: () = msg_send![&app, run];
        }

        Ok(())
    }
}
