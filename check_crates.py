import os
import re
import urllib.request
import json
import time

def get_latest_version(crate_name):
    url = f"https://crates.io/api/v1/crates/{crate_name}"
    try:
        req = urllib.request.Request(url, headers={'User-Agent': 'UnaOS/1.0'})
        with urllib.request.urlopen(req) as response:
            data = json.loads(response.read())
            return data['crate']['max_version']
    except Exception as e:
        return "ERROR"

crates = {}
for root, dirs, files in os.walk('.'):
    if 'Cargo.toml' in files:
        path = os.path.join(root, 'Cargo.toml')
        with open(path, 'r') as f:
            content = f.read()
            # simple regex to find deps: name = "version" or name = { version = "version" }
            # this is a simple approximation
            for match in re.finditer(r'^([a-zA-Z0-9_-]+)\s*=\s*(?:\{.*version\s*=\s*)?"([^"]+)"', content, re.MULTILINE):
                name = match.group(1)
                version = match.group(2)
                if name not in ['name', 'version', 'edition', 'license', 'description', 'repository', 'workspace']:
                    crates[name] = version

print(f"Found {len(crates)} unique dependencies.")
outdated = []
count = 0
for name, version in crates.items():
    latest = get_latest_version(name)
    count += 1
    if latest != "ERROR" and not version.startswith(latest) and version != latest:
        # Simple check, if version is "0.19" and latest is "0.21.1", they don't match
        # If version is "1.0" and latest is "1.0.104", version.startswith("1.0") is true.
        if latest.startswith(version):
            continue
        outdated.append((name, version, latest))
    time.sleep(0.1)

print("\nOutdated Crates:")
for name, version, latest in sorted(outdated):
    print(f"- {name}: current=\"{version}\" -> latest=\"{latest}\"")

