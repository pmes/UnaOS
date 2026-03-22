import re

with open('libs/quartzite/src/platforms/gtk/workspace/mod.rs', 'r') as f:
    content = f.read()

# Fix the sidebar::build call to include the workspace_state parameter
content = content.replace('let (sidebar_widgets, sidebar_pointers) = sidebar::build(window, tx_event.clone());', 'let (sidebar_widgets, sidebar_pointers) = sidebar::build(window, tx_event.clone(), workspace_state);')

# Also fix the `stream_tetra` extraction, it uses workspace_tetra variable name but the parameter is workspace_state
# Wait, let's see what the parameter is in mod.rs: `workspace_tetra: &bandy::state::WorkspaceState,`
content = content.replace('workspace_tetra: &bandy::state::WorkspaceState,', 'workspace_state: &bandy::state::WorkspaceState,')
content = content.replace('&workspace_tetra.right_pane', '&workspace_state.right_pane')

with open('libs/quartzite/src/platforms/gtk/workspace/mod.rs', 'w') as f:
    f.write(content)
