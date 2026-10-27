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

pub mod gpu_view;
pub mod image_view;
pub mod meter;
pub mod paper_board;
pub mod spline;
pub mod tone_panel;
pub mod window_chrome;
pub mod window_title;
pub mod workspace;
pub mod tetra_eval;
pub mod button;
pub mod text_field;
pub mod tab_strip;
pub mod find_bar;
pub mod status_line;
pub mod downloads;
pub mod tab_search;
// AETHERMACDEVTOOLS (SR312): the system-log Console and the developer-tools pane (`widgets::{console_rows, devtools_rows}`).
pub mod console_view;
pub mod devtools_view;

// The UI bootstrapping closure
type BootstrapFn = Box<
    dyn FnOnce(
        &NSWindow,
        async_channel::Sender<bandy::SMessage>,
        std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
        tokio::sync::broadcast::Receiver<bandy::SMessage>,
        bandy::state::WorkspaceState,
    ) -> (
        Retained<NSView>,
        Retained<workspace::sidebar::SidebarDelegate>,
        Option<Retained<workspace::comms::CommsDelegate>>,
        Option<Retained<workspace::editor::EditorDelegate>>,
        Option<Retained<workspace::console::ConsoleDelegate>>,
    ) + 'static,
>;

// The single-view vessel bootstrapping closure (see [`Backend::new_vessel`]).
type VesselBootstrapFn = Box<dyn FnOnce(&NSWindow) -> Retained<NSView> + 'static>;

/// Configuration for a single-view vessel window (title + initial content size +
/// the closure that builds its one content view).
struct VesselConfig {
    title: String,
    content_size: (f64, f64),
    build_view: VesselBootstrapFn,
}

// -----------------------------------------------------------------------------
// APP DELEGATE
// -----------------------------------------------------------------------------
struct AppDelegateIvars {
    bootstrap: RefCell<Option<BootstrapFn>>,
    tx_event: RefCell<Option<async_channel::Sender<bandy::SMessage>>>,
    app_state: RefCell<Option<std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>>>,
    rx_synapse: RefCell<Option<tokio::sync::broadcast::Receiver<bandy::SMessage>>>,
    workspace_state: RefCell<Option<bandy::state::WorkspaceState>>,

    // Single-view vessel path (mutually exclusive with `bootstrap` above)
    vessel: RefCell<Option<VesselConfig>>,
    vessel_view: RefCell<Option<Retained<NSView>>>,

    // Workspace-window title, derived from the vessel's app id in `Backend::new`
    // (e.g. "org.unaos.lumen" → "UnaOS: Lumen").
    window_title: RefCell<String>,

    window: RefCell<Option<Retained<NSWindow>>>,
    // Holding the delegate to prevent dropping
    window_delegate: RefCell<Option<Retained<window_chrome::WindowDelegate>>>,
    toolbar_delegate: RefCell<Option<Retained<window_chrome::ToolbarDelegate>>>,
    sidebar_delegate: RefCell<Option<Retained<workspace::sidebar::SidebarDelegate>>>,
    comms_delegate: RefCell<Option<Retained<workspace::comms::CommsDelegate>>>,
    editor_delegate: RefCell<Option<Retained<workspace::editor::EditorDelegate>>>,
    console_delegate: RefCell<Option<Retained<workspace::console::ConsoleDelegate>>>,
    // The "Save" menu item (Cmd+S). Its target is wired to the editor delegate
    // after bootstrap, so Cmd+S fires `EditorSaveRequest`.
    save_menu_item: RefCell<Option<Retained<objc2_app_kit::NSMenuItem>>>,
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

                vessel: RefCell::new(None),
                vessel_view: RefCell::new(None),

                window_title: RefCell::new(String::from("UnaOS")),

                window: RefCell::new(None),
                window_delegate: RefCell::new(None),
                toolbar_delegate: RefCell::new(None),
                sidebar_delegate: RefCell::new(None),
                comms_delegate: RefCell::new(None),
                editor_delegate: RefCell::new(None),
                console_delegate: RefCell::new(None),
                save_menu_item: RefCell::new(None),
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
                let app_menu_item: Retained<NSMenuItem> = msg_send![app_menu_item, initWithTitle: &*NSString::from_str("App"), action: None::<objc2::runtime::Sel>, keyEquivalent: &*NSString::from_str("")];
                let app_menu: Allocated<NSMenu> = msg_send![NSMenu::class(), alloc];
                let app_menu: Retained<NSMenu> = msg_send![app_menu, initWithTitle: &*NSString::from_str("App")];

                let quit_title = NSString::from_str("Quit");
                let quit_key = NSString::from_str("q");
                let quit_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let quit_item: Retained<NSMenuItem> = msg_send![quit_item, initWithTitle: &*quit_title, action: Some(objc2::sel!(terminate:)), keyEquivalent: &*quit_key];
                app_menu.addItem(&quit_item);

                let _: () = msg_send![&app_menu_item, setSubmenu: &*app_menu];
                main_menu.addItem(&app_menu_item);

                // File Menu — holds Save (Cmd+S). Target is wired to the editor
                // delegate after bootstrap so Cmd+S fires `EditorSaveRequest`.
                let file_menu_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let file_menu_item: Retained<NSMenuItem> = msg_send![file_menu_item, initWithTitle: &*NSString::from_str("File"), action: None::<objc2::runtime::Sel>, keyEquivalent: &*NSString::from_str("")];
                let file_menu: Allocated<NSMenu> = msg_send![NSMenu::class(), alloc];
                let file_menu: Retained<NSMenu> = msg_send![file_menu, initWithTitle: &*NSString::from_str("File")];

                let save_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let save_item: Retained<NSMenuItem> = msg_send![save_item, initWithTitle: &*NSString::from_str("Save"), action: Some(objc2::sel!(saveDocument:)), keyEquivalent: &*NSString::from_str("s")];
                file_menu.addItem(&save_item);
                *self.ivars().save_menu_item.borrow_mut() = Some(save_item.clone());

                let _: () = msg_send![&file_menu_item, setSubmenu: &*file_menu];
                main_menu.addItem(&file_menu_item);

                // Edit Menu
                let edit_menu_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let edit_menu_item: Retained<NSMenuItem> = msg_send![edit_menu_item, initWithTitle: &*NSString::from_str("Edit"), action: None::<objc2::runtime::Sel>, keyEquivalent: &*NSString::from_str("")];
                let edit_menu: Allocated<NSMenu> = msg_send![NSMenu::class(), alloc];
                let edit_menu: Retained<NSMenu> = msg_send![edit_menu, initWithTitle: &*NSString::from_str("Edit")];

                let undo_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let undo_item: Retained<NSMenuItem> = msg_send![undo_item, initWithTitle: &*NSString::from_str("Undo"), action: Some(objc2::sel!(undo:)), keyEquivalent: &*NSString::from_str("z")];
                edit_menu.addItem(&undo_item);

                let redo_item: Allocated<NSMenuItem> = msg_send![NSMenuItem::class(), alloc];
                let redo_item: Retained<NSMenuItem> = msg_send![redo_item, initWithTitle: &*NSString::from_str("Redo"), action: Some(objc2::sel!(redo:)), keyEquivalent: &*NSString::from_str("Z")];
                edit_menu.addItem(&redo_item);

                edit_menu.addItem(&NSMenuItem::separatorItem(mtm));

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

            // 0.5 Single-view vessel path: a plain titled window holding one custom view.
            // Mutually exclusive with the workspace bootstrap below (see `Backend::new_vessel`).
            if let Some(vessel) = self.ivars().vessel.borrow_mut().take() {
                let (window, window_delegate) =
                    window_chrome::create_vessel_window(mtm, &vessel.title, vessel.content_size);

                let view = (vessel.build_view)(&window);
                window.setContentView(Some(&view));

                *self.ivars().vessel_view.borrow_mut() = Some(view);
                *self.ivars().window.borrow_mut() = Some(window.clone());
                *self.ivars().window_delegate.borrow_mut() = Some(window_delegate);

                window.makeKeyAndOrderFront(None::<&AnyObject>);
                unsafe {
                    let _: () = msg_send![&window, center];
                    // A vessel launched from a background shell inherits no active
                    // app, so `makeKeyAndOrderFront` alone leaves it buried behind
                    // whatever had focus. Explicitly activate the app so the vessel
                    // window comes to the foreground.
                    let app = NSApplication::sharedApplication(mtm);
                    let _: () = msg_send![&app, activateIgnoringOtherApps: true];
                }
                return;
            }

            // 1. Create the native window through our chrome abstraction
            let (window, window_delegate, toolbar_delegate) =
                window_chrome::create_window(mtm, &self.ivars().window_title.borrow());

            // 2. Invoke the bootstrap closure to get the root view
            if let Some(bootstrap_fn) = self.ivars().bootstrap.borrow_mut().take() {
                let tx_event = self.ivars().tx_event.borrow_mut().take().expect("tx_event missing");
                let app_state = self.ivars().app_state.borrow_mut().take().expect("app_state missing");
                let rx_synapse = self.ivars().rx_synapse.borrow_mut().take().expect("rx_synapse missing");
                let workspace_state = self.ivars().workspace_state.borrow_mut().take().expect("workspace_state missing");

                let (root_view, sidebar_delegate, comms_delegate, editor_delegate, console_delegate) = bootstrap_fn(
                    &window,
                    tx_event,
                    app_state,
                    rx_synapse,
                    workspace_state,
                );
                window.setContentView(Some(&root_view));

                // Wire the "Save" menu item (Cmd+S) to the editor delegate, if the
                // active layout has an editor. With an explicit target the action
                // reaches the delegate directly (it is not otherwise in the
                // responder chain).
                if let (Some(save_item), Some(editor)) =
                    (self.ivars().save_menu_item.borrow().as_ref(), editor_delegate.as_ref())
                {
                    unsafe {
                        let _: () = msg_send![&**save_item, setTarget: &**editor];
                    }
                }

                // Store the internal UI delegates to prevent them from dropping.
                // Exactly one of comms/editor is populated (the active right pane);
                // console is populated when the workspace requested a bottom pane.
                *self.ivars().sidebar_delegate.borrow_mut() = Some(sidebar_delegate);
                *self.ivars().comms_delegate.borrow_mut() = comms_delegate;
                *self.ivars().editor_delegate.borrow_mut() = editor_delegate;
                *self.ivars().console_delegate.borrow_mut() = console_delegate;
            }

            // 3. Keep references alive
            *self.ivars().window.borrow_mut() = Some(window.clone());
            *self.ivars().window_delegate.borrow_mut() = Some(window_delegate);
            *self.ivars().toolbar_delegate.borrow_mut() = Some(toolbar_delegate);

            // 4. Show the window
            window.makeKeyAndOrderFront(None::<&AnyObject>);
            unsafe {
                // Installing the split-view content view lets AppKit shrink the
                // window to the panes' minimal fitting size ("opens collapsed").
                // Re-assert the intended workspace size before centering.
                let _: () = msg_send![&window, setContentSize: objc2_foundation::NSSize::new(1200.0, 800.0)];
                let _: () = msg_send![&window, center];
                // Same burial hazard as the vessel path above: launched from a
                // terminal shell there is no active app, so order-front alone
                // leaves the window behind whatever had focus.
                let app = NSApplication::sharedApplication(mtm);
                let _: () = msg_send![&app, activateIgnoringOtherApps: true];
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
        tx_event: async_channel::Sender<bandy::SMessage>,
        app_state: std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
        rx_synapse: tokio::sync::broadcast::Receiver<bandy::SMessage>,
        workspace_state: bandy::state::WorkspaceState,
        bootstrap: F,
    ) -> Self
    where
        F: FnOnce(
            &NSWindow,
            async_channel::Sender<bandy::SMessage>,
            std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
            tokio::sync::broadcast::Receiver<bandy::SMessage>,
            bandy::state::WorkspaceState,
        ) -> (
            Retained<NSView>,
            Retained<workspace::sidebar::SidebarDelegate>,
            Option<Retained<workspace::comms::CommsDelegate>>,
            Option<Retained<workspace::editor::EditorDelegate>>,
            Option<Retained<workspace::console::ConsoleDelegate>>,
        ) + 'static,
    {
        // Allocate and initialize the custom delegate
        let delegate: Allocated<AppDelegate> = unsafe { msg_send![AppDelegate::class(), alloc] };
        let delegate: Retained<AppDelegate> = unsafe { msg_send![delegate, init] };

        // Derive the window title from the app id's last segment, first letter
        // uppercased ("org.unaos.lumen" → "UnaOS: Lumen").
        let seg = _app_id.rsplit('.').next().unwrap_or(_app_id);
        let mut chars = seg.chars();
        let title = match chars.next() {
            Some(c) => format!("UnaOS: {}{}", c.to_uppercase(), chars.as_str()),
            None => String::from("UnaOS"),
        };
        *delegate.ivars().window_title.borrow_mut() = title;

        // Attach the bootstrap closure and dependencies
        *delegate.ivars().bootstrap.borrow_mut() = Some(Box::new(bootstrap));
        *delegate.ivars().tx_event.borrow_mut() = Some(tx_event);
        *delegate.ivars().app_state.borrow_mut() = Some(app_state);
        *delegate.ivars().rx_synapse.borrow_mut() = Some(rx_synapse);
        *delegate.ivars().workspace_state.borrow_mut() = Some(workspace_state);

        Self { delegate }
    }

    /// The lightweight sibling of [`Backend::new`] for single-view vessels.
    ///
    /// Where `new` boots the full workspace chrome (toolbar, sidebar, comms panes),
    /// `new_vessel` opens one plain titled window whose entire content is the view
    /// returned by `build_view`. The closure runs on the main thread once AppKit has
    /// finished launching; anything the view needs (a Synapse receiver, shared state)
    /// is captured by the closure — the vessel stays in charge of its own wiring.
    pub fn new_vessel<F>(
        _app_id: &str,
        title: &str,
        content_size: (f64, f64),
        build_view: F,
    ) -> Self
    where
        F: FnOnce(&NSWindow) -> Retained<NSView> + 'static,
    {
        let delegate: Allocated<AppDelegate> = unsafe { msg_send![AppDelegate::class(), alloc] };
        let delegate: Retained<AppDelegate> = unsafe { msg_send![delegate, init] };

        *delegate.ivars().vessel.borrow_mut() = Some(VesselConfig {
            title: title.to_string(),
            content_size,
            build_view: Box::new(build_view),
        });

        Self { delegate }
    }

    pub fn new_tetra_vessel(
        app_id: &str,
        title: &str,
        content_size: (f64, f64),
        tetra_node: crate::tetra::TetraNode,
        synapse: bandy::Synapse,
    ) -> Self {
        Self::new_vessel(app_id, title, content_size, move |window| {
            // The title bar mirrors the engine: page `<title>` and favicon
            // arrive over bandy and are written onto this window. Inert for a
            // tetra vessel that is not a browser — nothing else fires those.
            crate::platforms::macos::window_title::bind_browser_title(window, synapse.clone());
            crate::platforms::macos::tetra_eval::eval_tetra(tetra_node, synapse.clone())
        })
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

/// AETHERMACCHROME (SR205): the Aether browser window on AppKit, the GTK face's `bootstrap_browser_chrome`
/// counterpart over the same `Browser*` wire: the tetra `toolbar` (back, forward, reload, the address field) across
/// the top; under it the side tab column ([`tab_strip`], carrying the find bar's Cmd+F button) beside the page column
/// (the [`find_bar`] over the page surface, the [`downloads`] list under it); the [`status_line`] at the foot. The
/// title bar follows the page (`window_title::bind_browser_title`). The page surface is the one `eval_tetra` builds
/// (stretched), so it takes the window's slack; every other piece hugs. STRIP2 (SR219): the top strip stands between
/// the toolbar and the body, the side column in the body; Principia's `aether.tabs.layout` shows one
/// (`tab_strip::bootstrap_tab_strips`); the keys are the Tab menu's ([`install_tab_keys`]). AETHERMACDEVTOOLS (SR312): the
/// page sits under the developer-tools pane's overlay ([`devtools_view::bootstrap_devtools`]); the Develop menu
/// ([`install_develop_menu`]) carries its keys, F6 and the System Log, and sends Principia's accessibility rows.
pub fn bootstrap_browser_chrome(window: &NSWindow, toolbar: crate::tetra::TetraNode, synapse: bandy::Synapse) -> Retained<NSView> {
    window_title::bind_browser_title(window, synapse.clone());
    let bar = tetra_eval::eval_tetra(toolbar, synapse.clone());
    tab_strip::hug(&bar, 1);
    let find = find_bar::bootstrap_find_bar(synapse.clone());
    let strips = tab_strip::bootstrap_tab_strips(synapse.clone(), find_bar::find_toggle(&find));
    let page = tetra_eval::eval_tetra(crate::tetra::TetraNode::Surface { id: "viewport".into() }, synapse.clone()); gpu_view::attach_page(&page);
    let downloads = downloads::bootstrap_downloads(synapse.clone());
    install_tab_keys(&bar, &find, &synapse);
    let surface = page.clone();
    let page = devtools_view::bootstrap_devtools(synapse.clone(), surface.clone());
    install_develop_menu(&bar, &surface, &synapse);
    let status = status_line::bootstrap_status_line(synapse);

    let page_col = tab_strip::stack(true);
    let body = tab_strip::stack(false);
    let root = tab_strip::stack(true);
    unsafe {
        // NSLayoutAttributeWidth = 7: the page column's and the root's children fill their width.
        let _: () = msg_send![&page_col, setAlignment: 7isize];
        let _: () = msg_send![&root, setAlignment: 7isize];
        // NSLayoutAttributeHeight = 8: the side column and the page column take the body's full height.
        let _: () = msg_send![&body, setAlignment: 8isize];
        let _: () = msg_send![&page_col, addArrangedSubview: &*find.root];
        let _: () = msg_send![&page_col, addArrangedSubview: &*page];
        let _: () = msg_send![&page_col, addArrangedSubview: &*downloads];
        let _: () = msg_send![&body, addArrangedSubview: &*strips.side];
        let _: () = msg_send![&body, addArrangedSubview: &*page_col];
        let _: () = msg_send![&root, addArrangedSubview: &*bar];
        let _: () = msg_send![&root, addArrangedSubview: &*strips.top];
        let _: () = msg_send![&root, addArrangedSubview: &*body];
        let _: () = msg_send![&root, addArrangedSubview: &*status];
        Retained::cast_unchecked::<NSView>(root)
    }
}

/// STRIP2 (SR219): the browser's keys as a visible "Tab" menu on the main menu (a hidden item's key equivalent needs
/// `allowsKeyEquivalentWhenHidden`, macOS 13+): one item per `chrome_rows::key_verbs` (Primary = Cmd, Control =
/// Ctrl: Cmd+T new, Cmd+W close, Ctrl+Tab / Ctrl+PgDn next, Ctrl+Shift+Tab / Ctrl+PgUp previous, Cmd+D bookmark,
/// Cmd+= / Cmd++ / Cmd+- / Cmd+0 zoom), each firing its bandy verb through a `bootstrap_button` target; then
/// Cmd+L (the toolbar's address field takes the keys), Cmd+F (the find bar) and Cmd+Shift+A (the tab search).
fn install_tab_keys(bar: &NSView, find: &find_bar::FindBar, synapse: &bandy::Synapse) {
    use crate::widgets::chrome_rows::{self, KeyMods};
    use tab_strip::{CMD, CONTROL, SHIFT};
    let Some(mtm) = MainThreadMarker::new() else { return };
    let menu = tab_strip::new_menu("Tab");
    let add = |title: &str, key: &str, mask: usize, target: Retained<NSView>| unsafe {
        let item = tab_strip::menu_item(title, key, mask, &target);
        let _: () = msg_send![&*menu, addItem: &*item];
    };
    for kv in chrome_rows::key_verbs() {
        let Some(key) = mac_key(kv.key) else { continue };
        let mask = match kv.mods {
            KeyMods::Primary => CMD,
            KeyMods::PrimaryShift => CMD | SHIFT,
            KeyMods::Control => CONTROL,
            KeyMods::ControlShift => CONTROL | SHIFT,
        };
        let title = verb_title(&kv.msg);
        add(title, &key, mask, button::bootstrap_button(title, kv.msg.clone(), synapse.clone()));
    }
    unsafe {
        let sep: Retained<AnyObject> = msg_send![objc2::class!(NSMenuItem), separatorItem];
        let _: () = msg_send![&*menu, addItem: &*sep];
    }
    let field_host: Retained<NSView> = objc2::Message::retain(bar);
    add("Open Location", "l", CMD, button::bootstrap_button_with("Open Location", Box::new(move || focus_address(&field_host))));
    let f = find.clone();
    add("Find", "f", CMD, button::bootstrap_button_with("Find", Box::new(move || find_bar::show(&f))));
    let syn = synapse.clone();
    add("Search Tabs", "A", CMD, button::bootstrap_button_with("Search Tabs", Box::new(move || tab_search::open(&syn))));
    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let main: *mut AnyObject = msg_send![&app, mainMenu];
        if main.is_null() {
            return;
        }
        let item: *mut AnyObject = msg_send![objc2::class!(NSMenuItem), alloc];
        let t = objc2_foundation::NSString::from_str("Tab");
        let k = objc2_foundation::NSString::from_str("");
        let item: *mut AnyObject = msg_send![item, initWithTitle: &*t, action: None::<objc2::runtime::Sel>, keyEquivalent: &*k];
        let Some(item) = Retained::<AnyObject>::from_raw(item) else { return };
        let _: () = msg_send![&*item, setSubmenu: &*menu];
        let _: () = msg_send![main, addItem: &*item];
    }
}

/// A `chrome_rows::KeyVerb` key (a GTK keysym name) as an AppKit key equivalent; `None` for one AppKit has no
/// spelling for here.
fn mac_key(key: &str) -> Option<String> {
    Some(match key {
        "Tab" => "\t".to_string(),
        // NSPageDownFunctionKey / NSPageUpFunctionKey
        "Page_Down" => "\u{F72D}".to_string(),
        "Page_Up" => "\u{F72C}".to_string(),
        "plus" => "+".to_string(),
        "equal" => "=".to_string(),
        "minus" => "-".to_string(),
        k if k.chars().count() == 1 => k.to_string(),
        _ => return None,
    })
}

/// The Tab menu's title for a key's verb.
fn verb_title(m: &bandy::SMessage) -> &'static str {
    use bandy::SMessage as M;
    match m {
        M::BrowserTabNew => "New Tab",
        M::BrowserTabClose(_) => "Close Tab",
        M::BrowserTabStep(n) if *n > 0 => "Next Tab",
        M::BrowserTabStep(_) => "Previous Tab",
        M::BrowserBookmarkToggle => "Bookmark This Page",
        M::BrowserZoom(n) if *n > 0 => "Zoom In",
        M::BrowserZoom(n) if *n < 0 => "Zoom Out",
        M::BrowserZoom(_) => "Actual Size",
        _ => "Tab",
    }
}

/// Cmd+L: the first address field under `host` (the toolbar's `UnaTextField`) takes the keys, its text selected.
fn focus_address(host: &NSView) {
    let Some(field) = first_text_field(host) else { return };
    unsafe {
        let window: *mut AnyObject = msg_send![&field, window];
        if !window.is_null() {
            let _: objc2::runtime::Bool = msg_send![window, makeFirstResponder: &*field];
        }
    }
}

/// The first `UnaTextField` in `v`'s subtree, depth-first.
fn first_text_field(v: &NSView) -> Option<Retained<NSView>> {
    unsafe {
        let is: objc2::runtime::Bool = msg_send![v, isKindOfClass: text_field::UnaTextField::class()];
        if is.as_bool() {
            return Some(objc2::Message::retain(v));
        }
        let subs: Retained<objc2_foundation::NSArray<NSView>> = msg_send![v, subviews];
        subs.to_vec().iter().find_map(|s| first_text_field(s))
    }
}

/// AETHERMACDEVTOOLS (SR312): a visible "Develop" menu on the main menu, in [`install_tab_keys`]' shape (each item's
/// target a `bootstrap_button`): Developer Tools (Cmd+Option+I, and F12) fires `devtools_rows::toggle_key`, the toggle
/// the shell's `DevtoolsChrome::on_key` takes; JavaScript Console (Cmd+Option+J) fires `devtools_rows::console_verbs`
/// (the Console panel, opening a closed pane); Move Focus (F6) and Move Focus Back (Shift+F6) walk
/// `a11y_rows::F6_ORDER` ([`f6_focus`]); System Log (Ctrl+`) opens [`console_view::open_console_window`]. Then the
/// accessibility rows go out ([`send_a11y_rows`]).
fn install_develop_menu(bar: &NSView, surface: &NSView, synapse: &bandy::Synapse) {
    use crate::widgets::devtools_rows;
    use tab_strip::{CMD, CONTROL, SHIFT};
    /// `NSEventModifierFlagOption`.
    const OPTION: usize = 1 << 19;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let menu = tab_strip::new_menu("Develop");
    let add = |title: &str, key: &str, mask: usize, target: Retained<NSView>| unsafe {
        let item = tab_strip::menu_item(title, key, mask, &target);
        let _: () = msg_send![&*menu, addItem: &*item];
    };
    let sep = || unsafe {
        let s: Retained<AnyObject> = msg_send![objc2::class!(NSMenuItem), separatorItem];
        let _: () = msg_send![&*menu, addItem: &*s];
    };
    add("Developer Tools", "i", CMD | OPTION, button::bootstrap_button("Developer Tools", devtools_rows::toggle_key(), synapse.clone()));
    // NSF12FunctionKey
    add("Developer Tools (F12)", "\u{F70F}", 0, button::bootstrap_button("Developer Tools (F12)", devtools_rows::toggle_key(), synapse.clone()));
    let syn = synapse.clone();
    add(
        "JavaScript Console",
        "j",
        CMD | OPTION,
        button::bootstrap_button_with(
            "JavaScript Console",
            Box::new(move || {
                for m in devtools_rows::console_verbs(devtools_view::is_open()) {
                    syn.fire(m);
                }
            }),
        ),
    );
    sep();
    for (title, mask, back) in [("Move Focus", 0usize, false), ("Move Focus Back", SHIFT, true)] {
        let field_host: Retained<NSView> = objc2::Message::retain(bar);
        let page: Retained<NSView> = objc2::Message::retain(surface);
        // NSF6FunctionKey
        add(title, "\u{F709}", mask, button::bootstrap_button_with(title, Box::new(move || f6_focus(&field_host, &page, back))));
    }
    sep();
    let syn = synapse.clone();
    add("System Log", "`", CONTROL, button::bootstrap_button_with("System Log", Box::new(move || console_view::open_console_window(syn.clone()))));
    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let main: *mut AnyObject = msg_send![&app, mainMenu];
        if !main.is_null() {
            let item: *mut AnyObject = msg_send![objc2::class!(NSMenuItem), alloc];
            let t = objc2_foundation::NSString::from_str("Develop");
            let k = objc2_foundation::NSString::from_str("");
            let item: *mut AnyObject = msg_send![item, initWithTitle: &*t, action: None::<objc2::runtime::Sel>, keyEquivalent: &*k];
            if let Some(item) = Retained::<AnyObject>::from_raw(item) {
                let _: () = msg_send![&*item, setSubmenu: &*menu];
                let _: () = msg_send![main, addItem: &*item];
            }
        }
    }
    send_a11y_rows(synapse);
}

/// AETHERMACDEVTOOLS (SR312): F6 (`back`: Shift+F6) moves the keys to `a11y_rows::f6_step`'s stop: the address field
/// under `bar` ([`focus_address`]) or the page `surface`. Focus on the surface is the Page stop; anywhere else is neither.
fn f6_focus(bar: &NSView, surface: &NSView, back: bool) {
    use crate::widgets::a11y_rows::{f6_step, FocusStop};
    unsafe {
        let window: *mut AnyObject = msg_send![surface, window];
        if window.is_null() {
            return;
        }
        let first: *mut AnyObject = msg_send![window, firstResponder];
        let on_page = std::ptr::eq(first as *const AnyObject, surface as *const NSView as *const AnyObject);
        match f6_step(if on_page { Some(FocusStop::Page) } else { None }, back) {
            FocusStop::Address => focus_address(bar),
            FocusStop::Page => {
                let _: objc2::runtime::Bool = msg_send![window, makeFirstResponder: surface];
            }
        }
    }
}

/// AETHERMACDEVTOOLS (SR312): Principia's accessibility rows (`a11y_rows::read_rows`) go to the shell as
/// `BrowserA11yPrefs` now and again whenever a `PrefChanged` names one of them and the rows changed (the bus lane; the
/// GTK face also watches the store's folder, owed here). The dedupe lives in the lane's own thread.
fn send_a11y_rows(synapse: &bandy::Synapse) {
    use crate::widgets::a11y_rows;
    let syn = synapse.clone();
    let mut rx = synapse.subscribe();
    let spawned = std::thread::Builder::new().name("a11y-rows".into()).spawn(move || {
        let mut last = a11y_rows::read_rows();
        syn.fire(last.message());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a11y rows subscription runtime");
        rt.block_on(async move {
            loop {
                match rx.recv().await {
                    Ok(m) if a11y_rows::is_a11y_change(&m) => {
                        let rows = a11y_rows::read_rows();
                        if rows != last {
                            syn.fire(rows.message());
                            last = rows;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    });
    if let Err(e) = spawned {
        log::warn!("[aether] accessibility rows: no lane ({e})");
    }
}
