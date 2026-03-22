import re

with open('libs/quartzite/src/platforms/qt/vein_bridge.rs', 'r') as f:
    content = f.read()

# Add static OnceLock
static_decl = "pub static WORKSPACE_STATE: OnceLock<bandy::state::WorkspaceState> = OnceLock::new();\n"
content = re.sub(r'pub static MATRIX_MODEL_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::MatrixModel>> =\n\s*OnceLock::new\(\);\n', 'pub static MATRIX_MODEL_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::MatrixModel>> =\n    OnceLock::new();\n' + static_decl, content)

# Update MatrixModelRust::default()
default_impl = """impl Default for MatrixModelRust {
    fn default() -> Self {
        let workspace_state = WORKSPACE_STATE.get().cloned().unwrap_or_default();
        let rows = if let bandy::state::ViewEntity::Topology(topology_state) = workspace_state.left_pane {
            topology_state.tree.flatten().into_iter().map(|(n, depth)| {
                MatrixNodeRow {
                    id: n.id.clone(),
                    label: n.label.clone(),
                    depth,
                }
            }).collect()
        } else {
            Vec::new()
        };
        Self { rows }
    }
}"""

content = re.sub(r'impl Default for MatrixModelRust \{[\s\S]*?\}\n\}', default_impl, content)

# Map role 0 to matrixLabel instead of display
content = content.replace('roles.insert(0, cxx_qt_lib::QByteArray::from("display"));', 'roles.insert(0, cxx_qt_lib::QByteArray::from("matrixLabel"));')

with open('libs/quartzite/src/platforms/qt/vein_bridge.rs', 'w') as f:
    f.write(content)
