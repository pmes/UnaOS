pub mod comms;
pub mod reactor;
pub mod sidebar;
pub mod translator;

use async_channel::Sender;
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use bandy::SMessage;
use bandy::state::AppState;
use std::sync::{Arc, RwLock};
use crate::Event;
use crate::NativeWindow;

pub struct WorkspaceWidgets {
    pub left_stack: gtk4::Stack,
    pub right_stack: gtk4::Stack,
    pub status_group: gtk4::Box,
    pub left_switcher: gtk4::StackSwitcher,
    pub right_switcher: gtk4::StackSwitcher,
}

use gtk4::prelude::*;

// Why Broadcast? MPMC channels (`async_channel`) load-balance by consuming messages,
// causing UI starvation if a background thread wins the race. `tokio::sync::broadcast`
// enforces pub/sub physics, ensuring the UI and Backend independently observe the exact
// same state reality without stealing from each other.
pub fn build(
    window: &NativeWindow,
    tx_event: Sender<Event>,
    app_state: Arc<RwLock<AppState>>,
    rx_synapse: BroadcastReceiver<SMessage>,
    brain_icon: gtk4::Image,
    workspace_tetra: &crate::tetra::WorkspaceTetra,
    workspace_state: &bandy::state::WorkspaceState,
) -> WorkspaceWidgets {
    // Spawn translator
    let rx_gui = translator::spawn_translator(rx_synapse, app_state);

    // Build Sidebar Lobes
    let (sidebar_widgets, sidebar_pointers) = sidebar::build(window, tx_event.clone(), workspace_tetra, workspace_state);

    // Extract StreamTetra from Right Pane
    let default_tetra = crate::tetra::StreamTetra::default();
    let stream_tetra = match &workspace_tetra.right_pane {
        crate::tetra::TetraNode::Stream(tetra) => tetra,
        _ => &default_tetra,
    };

    // Build Comms Lobes
    let (comms_widgets, comms_pointers) = comms::build(
        window,
        tx_event.clone(),
        sidebar_pointers.active_target.clone(),
        sidebar_widgets.composer_btn.clone(),
        stream_tetra,
        sidebar_pointers.matrix_selection.clone(),
    );

    // Network Inspector Window
    let net_window = gtk4::Window::builder()
        .title("Network Inspector")
        .default_width(600)
        .default_height(600)
        .hide_on_close(true)
        .build();
    let net_buffer = sourceview5::Buffer::new(None);
    let net_view = sourceview5::View::with_buffer(&net_buffer);
    net_view.set_editable(false);
    net_view.set_monospace(true);
    let net_scroll = gtk4::ScrolledWindow::builder()
        .child(&net_view)
        .build();
    net_window.set_child(Some(&net_scroll));

    let net_btn = sidebar_widgets.network_btn.clone();
    sidebar_widgets.network_btn.connect_clicked(move |_| {
        net_window.present();
    });

    // Reactor bindings
    let pointers = reactor::ReactorPointers {
        spinner_una: sidebar_pointers.spinner_una,
        label_una: sidebar_pointers.label_una,
        spinner_s9: sidebar_pointers.spinner_s9,
        label_s9: sidebar_pointers.label_s9,
        token_label: sidebar_pointers.token_label,
        pulse_icon: brain_icon,
        context_view: sidebar_pointers.context_view,
        active_directive: comms_pointers.active_directive,
        console_store: comms_pointers.console_store,
        is_fetching: comms_pointers.is_fetching,
        is_prepending: comms_pointers.is_prepending,
        preflight_overlay: comms_pointers.preflight_overlay,
        preflight_stack_container: comms_pointers.preflight_stack_container,
        preflight_stack: comms_pointers.preflight_stack,
        preflight_sys_buf: comms_pointers.preflight_sys_buf,
        preflight_dir_buf: comms_pointers.preflight_dir_buf,
        preflight_eng_buf: comms_pointers.preflight_eng_buf,
        preflight_prm_buf: comms_pointers.preflight_prm_buf,
        matrix_store: sidebar_pointers.matrix_store,
        matrix_selection: sidebar_pointers.matrix_selection,
        net_buffer,
        network_btn: net_btn,
    };

    reactor::spawn_listener(pointers, rx_gui);

    WorkspaceWidgets {
        left_stack: sidebar_widgets.left_stack,
        right_stack: comms_widgets.workspace_stack,
        status_group: sidebar_widgets.status_group,
        left_switcher: sidebar_widgets.left_switcher,
        right_switcher: comms_widgets.right_switcher,
    }
}
