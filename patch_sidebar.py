import re

with open('libs/quartzite/src/platforms/gtk/workspace/sidebar.rs', 'r') as f:
    content = f.read()

# Replace imports
content = content.replace('use crate::tetra::{WorkspaceTetra, TetraNode};', 'use bandy::state::{WorkspaceState, ViewEntity};\nuse bandy::state::TopologyNode;')

# Update build signature
content = content.replace('pub fn build(window: &NativeWindow, tx_event: Sender<Event>) -> (SidebarWidgets, SidebarPointers) {', 'pub fn build(window: &NativeWindow, tx_event: Sender<Event>, workspace_state: &WorkspaceState) -> (SidebarWidgets, SidebarPointers) {')

# Find the block where matrix_store is populated and fix it to use workspace_state
pattern = r'let matrix_store = gio::ListStore::new::<crate::widgets::model::MatrixNodeObject>\(\);\n\s*let workspace_tetra = WorkspaceTetra::default\(\);\n\s*if let TetraNode::Matrix\(matrix_tetra\) = &workspace_tetra\.left_pane \{\n\s*let flat_nodes = matrix_tetra\.tree\.flatten\(\);\n\s*for \(node, depth\) in flat_nodes \{\n\s*let obj = crate::widgets::model::MatrixNodeObject::new\(&node\.id, &node\.label, depth as u32\);\n\s*matrix_store\.append\(&obj\);\n\s*\}\n\s*\}'

replacement = """let matrix_store = gio::ListStore::new::<crate::widgets::model::MatrixNodeObject>();
    if let ViewEntity::Matrix(topology_state) = &workspace_state.left_pane {
        let flat_nodes = topology_state.tree.flatten();
        for (node, depth) in flat_nodes {
            let obj = crate::widgets::model::MatrixNodeObject::new(&node.id, &node.label, depth as u32);
            matrix_store.append(&obj);
        }
    }"""

content = re.sub(pattern, replacement, content)


with open('libs/quartzite/src/platforms/gtk/workspace/sidebar.rs', 'w') as f:
    f.write(content)
