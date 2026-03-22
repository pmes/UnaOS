import re

with open('libs/bandy/src/state.rs', 'r') as f:
    content = f.read()

# Make sure HashSet is imported at the top
if 'use std::collections::HashSet;' not in content:
    content = content.replace('use std::collections::{HashMap, VecDeque};', 'use std::collections::{HashMap, VecDeque, HashSet};')

with open('libs/bandy/src/state.rs', 'w') as f:
    f.write(content)
