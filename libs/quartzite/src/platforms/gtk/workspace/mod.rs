pub mod comms;
pub mod reactor;
pub mod sidebar;
pub mod translator;

use async_channel::{Receiver, Sender};
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

pub fn build(
    window: &NativeWindow,
    tx_event: Sender<Event>,
    app_state: Arc<RwLock<AppState>>,
    rx_synapse: Receiver<SMessage>,
    brain_icon: gtk4::Image,
) -> WorkspaceWidgets {
    // Spawn translator
    let rx_gui = translator::spawn_translator(rx_synapse, app_state);

    // Build Sidebar Lobes
    let (sidebar_widgets, sidebar_pointers) = sidebar::build(window, tx_event.clone());

    // Build Comms Lobes
    let (comms_widgets, comms_pointers) = comms::build(
        window,
        tx_event.clone(),
        sidebar_pointers.active_target.clone(),
        sidebar_widgets.composer_btn.clone(),
    );

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
        history_sync_cursor: comms_pointers.history_sync_cursor,
        preflight_overlay: comms_pointers.preflight_overlay,
        preflight_stack_container: comms_pointers.preflight_stack_container,
        preflight_stack: comms_pointers.preflight_stack,
        preflight_sys_buf: comms_pointers.preflight_sys_buf,
        preflight_dir_buf: comms_pointers.preflight_dir_buf,
        preflight_eng_buf: comms_pointers.preflight_eng_buf,
        preflight_prm_buf: comms_pointers.preflight_prm_buf,
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
