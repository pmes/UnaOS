import re

with open('libs/bandy/src/state.rs', 'r') as f:
    content = f.read()

# find duplicate import and delete the injected lines that are wrong
content = re.sub(r'use serde::\{Deserialize, Serialize\};\nuse std::collections::HashSet;\n\n#\[derive\(Clone, Debug, Serialize, Deserialize\)\]', '#[derive(Clone, Debug, Serialize, Deserialize)]', content)

with open('libs/bandy/src/state.rs', 'w') as f:
    f.write(content)
