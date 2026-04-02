// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow, NSView,
    NSResponder
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol};
use std::cell::RefCell;

pub mod spline;
pub mod window_chrome;
pub mod workspace;

// The UI bootstrapping closure
type BootstrapFn = Box<
    dyn FnOnce(
        &NSWindow,
        async_channel::Sender<gneiss_pal::Event>,
        std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
        tokio::sync::broadcast::Receiver<bandy::SMessage>,
        bandy::state::WorkspaceState,
    ) -> (
        Retained<NSView>,
        Retained<workspace::sidebar::SidebarDelegate>,
        Retained<workspace::comms::CommsDelegate>,
    ) + 'static,
>;

// -----------------------------------------------------------------------------
// APP DELEGATE
// -----------------------------------------------------------------------------
struct AppDelegateIvars {
    bootstrap: RefCell<Option<BootstrapFn>>,
    tx_event: RefCell<Option<async_channel::Sender<gneiss_pal::Event>>>,
    app_state: RefCell<Option<std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>>>,
    rx_synapse: RefCell<Option<tokio::sync::broadcast::Receiver<bandy::SMessage>>>,
    workspace_state: RefCell<Option<bandy::state::WorkspaceState>>,

    window: RefCell<Option<Retained<NSWindow>>>,
    // Holding the delegate to prevent dropping
    window_delegate: RefCell<Option<Retained<window_chrome::WindowDelegate>>>,
    toolbar_delegate: RefCell<Option<Retained<window_chrome::ToolbarDelegate>>>,
    sidebar_delegate: RefCell<Option<Retained<workspace::sidebar::SidebarDelegate>>>,
    comms_delegate: RefCell<Option<Retained<workspace::comms::CommsDelegate>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaAppDelegate"]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(AppDelegateIvars {
                bootstrap: RefCell::new(None),
                tx_event: RefCell::new(None),
                app_state: RefCell::new(None),
                rx_synapse: RefCell::new(None),
                workspace_state: RefCell::new(None),

                window: RefCell::new(None),
                window_delegate: RefCell::new(None),
                toolbar_delegate: RefCell::new(None),
                sidebar_delegate: RefCell::new(None),
                comms_delegate: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &AnyObject) {
            let mtm = MainThreadMarker::from(self);

            // 0. Build the Main Menu to populate the Responder Chain
            unsafe {
                use objc2_app_kit::{NSMenu, NSMenuItem};
                use objc2_foundation::NSString;

                let main_menu: Allocated<NSMenu> = msg_send![NSMenu::class(), alloc];
                let title = NSString::from_str("MainMenu");
                let main_menu: Retained<NSMenu> = msg_send![main_menu, initWithTitle: &*title];

                // App Menu
                let app_menu_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let app_menu_item: Retained<NSMenuItem> = msg_send![app_menu_item, initWithTitle: &*NSString::from_str("App"), action: None, keyEquivalent: &*NSString::from_str("")];
                let app_menu: Allocated<NSMenu> = msg_send![NSMenu::class(), alloc];
                let app_menu: Retained<NSMenu> = msg_send![app_menu, initWithTitle: &*NSString::from_str("App")];

                let quit_title = NSString::from_str("Quit");
                let quit_key = NSString::from_str("q");
                let quit_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let quit_item: Retained<NSMenuItem> = msg_send![quit_item, initWithTitle: &*quit_title, action: Some(objc2::sel!(terminate:)), keyEquivalent: &*quit_key];
                app_menu.addItem(&quit_item);

                let _: () = msg_send![&app_menu_item, setSubmenu: &*app_menu];
                main_menu.addItem(&app_menu_item);

                // Edit Menu
                let edit_menu_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let edit_menu_item: Retained<NSMenuItem> = msg_send![edit_menu_item, initWithTitle: &*NSString::from_str("Edit"), action: None, keyEquivalent: &*NSString::from_str("")];
                let edit_menu: Allocated<NSMenu> = msg_send![NSMenu::class(), alloc];
                let edit_menu: Retained<NSMenu> = msg_send![edit_menu, initWithTitle: &*NSString::from_str("Edit")];

                let undo_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let undo_item: Retained<NSMenuItem> = msg_send![undo_item, initWithTitle: &*NSString::from_str("Undo"), action: Some(objc2::sel!(undo:)), keyEquivalent: &*NSString::from_str("z")];
                edit_menu.addItem(&undo_item);

                let redo_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let redo_item: Retained<NSMenuItem> = msg_send![redo_item, initWithTitle: &*NSString::from_str("Redo"), action: Some(objc2::sel!(redo:)), keyEquivalent: &*NSString::from_str("Z")];
                edit_menu.addItem(&redo_item);

                edit_menu.addItem(&NSMenuItem::separatorItem());

                let cut_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let cut_item: Retained<NSMenuItem> = msg_send![cut_item, initWithTitle: &*NSString::from_str("Cut"), action: Some(objc2::sel!(cut:)), keyEquivalent: &*NSString::from_str("x")];
                edit_menu.addItem(&cut_item);

                let copy_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let copy_item: Retained<NSMenuItem> = msg_send![copy_item, initWithTitle: &*NSString::from_str("Copy"), action: Some(objc2::sel!(copy:)), keyEquivalent: &*NSString::from_str("c")];
                edit_menu.addItem(&copy_item);

                let paste_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let paste_item: Retained<NSMenuItem> = msg_send![paste_item, initWithTitle: &*NSString::from_str("Paste"), action: Some(objc2::sel!(paste:)), keyEquivalent: &*NSString::from_str("v")];
                edit_menu.addItem(&paste_item);

                let select_all_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let select_all_item: Retained<NSMenuItem> = msg_send![select_all_item, initWithTitle: &*NSString::from_str("Select All"), action: Some(objc2::sel!(selectAll:)), keyEquivalent: &*NSString::from_str("a")];
                edit_menu.addItem(&select_all_item);

                let _: () = msg_send![&edit_menu_item, setSubmenu: &*edit_menu];
                main_menu.addItem(&edit_menu_item);

                NSApplication::sharedApplication(mtm).setMainMenu(Some(&main_menu));
            }

            // 1. Create the native window through our chrome abstraction
            let (window, window_delegate, toolbar_delegate) = window_chrome::create_window(mtm);

            // 2. Invoke the bootstrap closure to get the root view
            if let Some(bootstrap_fn) = self.ivars().bootstrap.borrow_mut().take() {
                let tx_event = self.ivars().tx_event.borrow_mut().take().expect("tx_event missing");
                let app_state = self.ivars().app_state.borrow_mut().take().expect("app_state missing");
                let rx_synapse = self.ivars().rx_synapse.borrow_mut().take().expect("rx_synapse missing");
                let workspace_state = self.ivars().workspace_state.borrow_mut().take().expect("workspace_state missing");

                let (root_view, sidebar_delegate, comms_delegate) = bootstrap_fn(
                    &window,
                    tx_event,
                    app_state,
                    rx_synapse,
                    workspace_state,
                );
                window.setContentView(Some(&root_view));

                // Store the internal UI delegates to prevent them from dropping
                *self.ivars().sidebar_delegate.borrow_mut() = Some(sidebar_delegate);
                *self.ivars().comms_delegate.borrow_mut() = Some(comms_delegate);
            }

            // 3. Keep references alive
            *self.ivars().window.borrow_mut() = Some(window.clone());
            *self.ivars().window_delegate.borrow_mut() = Some(window_delegate);
            *self.ivars().toolbar_delegate.borrow_mut() = Some(toolbar_delegate);

            // 4. Show the window
            window.makeKeyAndOrderFront(None::<&AnyObject>);
            unsafe {
                let _: () = msg_send![&window, center];
            }
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn application_should_terminate_after_last_window_closed(
            &self,
            _sender: &NSApplication,
        ) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }
    }
);

unsafe impl NSObjectProtocol for AppDelegate {}

// -----------------------------------------------------------------------------
// BACKEND
// -----------------------------------------------------------------------------
pub struct Backend {
    delegate: Retained<AppDelegate>,
}

impl Backend {
    pub fn new<F>(
        _app_id: &str,
        tx_event: async_channel::Sender<gneiss_pal::Event>,
        app_state: std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
        rx_synapse: tokio::sync::broadcast::Receiver<bandy::SMessage>,
        workspace_state: bandy::state::WorkspaceState,
        bootstrap: F,
    ) -> Self
    where
        F: FnOnce(
            &NSWindow,
            async_channel::Sender<gneiss_pal::Event>,
            std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
            tokio::sync::broadcast::Receiver<bandy::SMessage>,
            bandy::state::WorkspaceState,
        ) -> (
            Retained<NSView>,
            Retained<workspace::sidebar::SidebarDelegate>,
            Retained<workspace::comms::CommsDelegate>,
        ) + 'static,
    {
        // Allocate and initialize the custom delegate
        let delegate: Allocated<AppDelegate> = unsafe { msg_send![AppDelegate::class(), alloc] };
        let delegate: Retained<AppDelegate> = unsafe { msg_send![delegate, init] };

        // Attach the bootstrap closure and dependencies
        *delegate.ivars().bootstrap.borrow_mut() = Some(Box::new(bootstrap));
        *delegate.ivars().tx_event.borrow_mut() = Some(tx_event);
        *delegate.ivars().app_state.borrow_mut() = Some(app_state);
        *delegate.ivars().rx_synapse.borrow_mut() = Some(rx_synapse);
        *delegate.ivars().workspace_state.borrow_mut() = Some(workspace_state);

        Self { delegate }
    }

    pub fn run(&self) {
        let mtm = MainThreadMarker::new().expect("Must run on main thread");
        let app = NSApplication::sharedApplication(mtm);

        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.setDelegate(Some(ProtocolObject::from_ref(&*self.delegate)));

        // Hand over control to AppKit
        unsafe {
            let _: () = msg_send![&app, run];
        }
    }
}
