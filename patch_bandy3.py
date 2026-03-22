import re

with open('libs/bandy/src/state.rs', 'r') as f:
    content = f.read()

content = content.replace('Matrix(TopologyState)', 'Topology(TopologyState)')

content = content.replace('ViewEntity::Matrix', 'ViewEntity::Topology')

with open('libs/bandy/src/state.rs', 'w') as f:
    f.write(content)
