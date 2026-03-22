import re

with open('handlers/matrix/src/lib.rs', 'r') as f:
    content = f.read()

# I apparently reverted matrix changes earlier by accident, let's re-apply them.
content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\nuse async_channel::Sender;\n', '', content)
content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\nuse elessar::\{Context, Spline\};\n', '', content)
content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\nuse gneiss_pal::Event;\n', '', content)
content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\nuse gtk4::prelude::\*;\n', '', content)
content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\nuse gtk4::\{ListBox, ScrolledWindow, Widget\};\n', '', content)

content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\npub struct ProjectView \{.*?\n\}\n', '', content, flags=re.DOTALL)
content = re.sub(r'#\[cfg\(feature = "gtk"\)\]\nimpl ProjectView \{.*?\n\}\n', '', content, flags=re.DOTALL)

content = re.sub(r'/// The UI Builder.*?#\[cfg\(feature = "gtk"\)\].*?\npub fn create_view.*?\}\n\n', '', content, flags=re.DOTALL)

match = re.search(r'impl MatrixScanner \{', content)
if match:
    insert_pos = match.end()
    build_genesis_tree_code = """
    pub fn build_genesis_tree(dir: &Path, absolute_root: &Path) -> Vec<bandy::state::TopologyNode> {
        let mut nodes = Vec::new();

        let Ok(entries) = std::fs::read_dir(dir) else {
            return nodes;
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();

                if file_name == "target" || file_name == ".git" || file_name == "node_modules" {
                    continue;
                }

                if path.is_dir() {
                    dirs.push((path, file_name));
                } else {
                    files.push((path, file_name));
                }
            }
        }

        dirs.sort_by(|a, b| a.1.cmp(&b.1));
        files.sort_by(|a, b| a.1.cmp(&b.1));

        for (path, file_name) in dirs {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
            let children = Self::build_genesis_tree(&path, absolute_root);
            nodes.push(bandy::state::TopologyNode {
                id,
                label: file_name,
                children,
                is_expanded: false,
            });
        }

        for (path, file_name) in files {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
            nodes.push(bandy::state::TopologyNode {
                id,
                label: file_name,
                children: Vec::new(),
                is_expanded: false,
            });
        }

        nodes
    }
"""
    content = content[:insert_pos] + "\n" + build_genesis_tree_code + content[insert_pos:]

with open('handlers/matrix/src/lib.rs', 'w') as f:
    f.write(content)
