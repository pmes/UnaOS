#!/usr/bin/env python3
import os
import sys
import hashlib
import re

def sha256_file(filepath):
    sha256_hash = hashlib.sha256()
    try:
        with open(filepath, "rb") as f:
            for byte_block in iter(lambda: f.read(4096), b""):
                sha256_hash.update(byte_block)
        return sha256_hash.hexdigest()
    except Exception as e:
        return None

def main():
    if len(sys.argv) < 2:
        print("Usage: validate-manifest.py <path_to_MANIFEST>")
        sys.exit(1)
        
    manifest_path = sys.argv[1]
    if not os.path.exists(manifest_path):
        print(f"Error: {manifest_path} does not exist.")
        sys.exit(1)
        
    base_dir = os.path.dirname(manifest_path)
    if not base_dir:
        base_dir = "."
        
    manifest_entries = []
    
    # We assume format could have sha256 hashes (64 hex chars) and filenames.
    sha_regex = re.compile(r'([a-fA-F0-9]{64})')
    
    # Track order to ensure append-only (monotonically increasing if there is a sequence number)
    last_seq = None
    is_ordered = True

    try:
        with open(manifest_path, 'r') as f:
            for line_no, line in enumerate(f, 1):
                line = line.strip()
                if not line or line.startswith('#'):
                    continue
                    
                match = sha_regex.search(line)
                if match:
                    sha = match.group(1).lower()
                    # extract the filename, assuming it's the token after the hash
                    parts = line.split()
                    try:
                        idx = parts.index(match.group(1))
                        filename = parts[idx+1]
                    except (ValueError, IndexError):
                        # Fallback: just take the last token
                        filename = parts[-1]
                    
                    # Optional: check if first token is a sequence/timestamp
                    try:
                        seq = int(parts[0])
                        if last_seq is not None and seq <= last_seq:
                            is_ordered = False
                            print(f"Warning: append-only ordering violated at line {line_no} (seq {seq} <= {last_seq})")
                        last_seq = seq
                    except ValueError:
                        pass
                        
                    manifest_entries.append((filename, sha, line_no))
                else:
                    print(f"Warning: could not parse line {line_no} in MANIFEST: {line}")
    except Exception as e:
        print(f"Error reading MANIFEST: {e}")
        sys.exit(1)
        
    errors = 0
    manifest_files = set()
    
    for filename, expected_sha, line_no in manifest_entries:
        manifest_files.add(filename)
        filepath = os.path.join(base_dir, filename)
        
        if not os.path.exists(filepath):
            print(f"Error [Line {line_no}]: File {filename} listed in MANIFEST but not found on disk.")
            errors += 1
            continue
            
        actual_sha = sha256_file(filepath)
        if actual_sha != expected_sha:
            print(f"Error [Line {line_no}]: Hash mismatch for {filename}.")
            print(f"  Expected: {expected_sha}")
            print(f"  Actual:   {actual_sha}")
            errors += 1
            
    # Check for orphaned files
    # We ignore standard files like MANIFEST itself, and directories
    actual_files = set()
    for item in os.listdir(base_dir):
        if os.path.isfile(os.path.join(base_dir, item)):
            if item not in ["MANIFEST", "MANIFEST.bak"] and not item.startswith("."):
                actual_files.add(item)
                
    orphans = actual_files - manifest_files
    for orphan in orphans:
        print(f"Error: Orphaned file found on disk but not in MANIFEST: {orphan}")
        errors += 1
        
    if not is_ordered:
        print("Error: Append-only ordering is not intact.")
        errors += 1

    if errors == 0:
        print(f"Success: MANIFEST validated perfectly. {len(manifest_entries)} files checked.")
        sys.exit(0)
    else:
        print(f"Validation failed with {errors} errors.")
        sys.exit(1)

if __name__ == '__main__':
    main()
