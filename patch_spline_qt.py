import re

with open('libs/quartzite/src/spline.rs', 'r') as f:
    content = f.read()

qt_block = """        #[cfg(all(target_os = "linux", feature = "qt"))]
        {
            use crate::platforms::qt::ffi;

            // To fulfill the nervous system, we inject the event_tx to the backend.
            let _ = crate::platforms::qt::window::GLOBAL_TX.set(_tx_event);

            // Spawn the tokio backend to listen to StateInvalidated pings from Vein/Cortex
            crate::platforms::qt::window::spawn_state_listener(_app_state, _rx_synapse);

            // Set the master static state for MatrixModelRust to consume on boot
            let _ = crate::platforms::qt::vein_bridge::WORKSPACE_STATE.set(_workspace_tetra.clone());

            let default_tetra = bandy::state::StreamState::default();
            let stream_tetra = match &_workspace_tetra.right_pane {
                bandy::state::ViewEntity::Stream(tetra) => tetra,
                _ => &default_tetra,
            };
            return crate::NativeView {
                ptr: ffi::create_main_window(
                    _workspace_tetra.split_ratio,
                    stream_tetra.input_anchor.clone() as i32,
                    stream_tetra.scroll_behavior.clone() as i32,
                    stream_tetra.alignment.clone() as i32
                ),
            };
        }"""

pattern = r'#\[cfg\(all\(target_os = "linux", feature = "qt"\)\)\]\s*\{.*?return crate::NativeView \{.*?\}\);\n\s*\}'
content = re.sub(pattern, qt_block, content, flags=re.DOTALL)

with open('libs/quartzite/src/spline.rs', 'w') as f:
    f.write(content)
