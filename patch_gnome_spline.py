import re

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

code = code.replace(
    'return build_gnome_ui(window, tx_event, rx, rx_synapse);',
    'return build_gnome_ui(window, tx_event, app_state, rx_synapse);'
)

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
