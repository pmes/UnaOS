import re

with open('apps/lumen/src/main.rs', 'r') as f:
    content = f.read()

# Replace workspace tetra declaration
workspace_tetra_block = """    // 7.5. Define the Workspace Layout via Declarative UI Engine
    let genesis_roots = matrix::MatrixScanner::build_genesis_tree(&absolute_workspace_root_arc, &absolute_workspace_root_arc);
    let workspace_state = bandy::state::WorkspaceState {
        left_pane: bandy::state::ViewEntity::Topology(bandy::state::TopologyState::new(genesis_roots)),
        right_pane: bandy::state::ViewEntity::Stream(bandy::state::StreamState::default()),
        split_ratio: 0.25,
    };

    let workspace_state_clone = workspace_state.clone();"""

content = re.sub(r'    // 7\.5\. Define the Workspace Layout via Declarative UI Engine[\s\S]*?let workspace_tetra_clone = workspace_tetra\.clone\(\);', workspace_tetra_block, content)

# Update workspace_tetra usages
content = content.replace('let mut workspace_tetra = workspace_tetra_clone;', 'let mut workspace_state = workspace_state_clone;')
content = content.replace('workspace_tetra.left_pane', 'workspace_state.left_pane')
content = content.replace('&workspace_tetra', '&workspace_state')

# Update quartzite::tetra matches inside the event loop
content = content.replace('quartzite::tetra::TetraNode::Matrix(ref mut matrix)', 'bandy::state::ViewEntity::Topology(ref mut matrix)')

with open('apps/lumen/src/main.rs', 'w') as f:
    f.write(content)
