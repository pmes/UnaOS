#!/usr/bin/env python3
import sys
import re
import argparse

def strip_control_bytes(text):
    # Strip \x00-\x1F except \n, \r, \t
    return re.sub(r'[\x00-\x08\x0b-\x0c\x0e-\x1f]', '', text)

def parse_log(filepath):
    with open(filepath, 'r', errors='replace') as f:
        content = f.read()
    
    content = strip_control_bytes(content)
    
    # Split by boots
    # We look for lines containing "MARK " and " boot<N> "
    # The actual line from logs is e.g.
    # 2026-07-19T22:29:39Z MARK MARK R23s1 boot12 ORIN ...
    
    lines = content.split('\n')
    
    boots = []
    current_boot = None
    
    for line in lines:
        if ' MARK ' in line and ' boot' in line:
            match = re.search(r'boot(\d+)', line)
            if match:
                boot_num = match.group(1)
                current_boot = {
                    'number': boot_num,
                    'start_line': line,
                    'witnesses': [],
                    'lines': []
                }
                boots.append(current_boot)
        if current_boot:
            current_boot['lines'].append(line)
            # Extract witness lines
            if '::' in line and 'witness' in line:
                current_boot['witnesses'].append(line)
    
    # Classify each boot
    for boot in boots:
        boot_text = '\n'.join(boot['lines'])
        classes = []
        if 'RAS' in boot_text:
            classes.append('RAS')
        if 'panic' in boot_text.lower():
            classes.append('panic')
        if 'PASS' in boot_text:
            classes.append('PASS')
        if 'lease' in boot_text.lower():
            classes.append('lease')
        
        boot['classes'] = classes if classes else ['unknown']
        
    return boots

def print_boot_summary(boot):
    print(f"Boot {boot['number']}: {', '.join(boot['classes'])}")
    print(f"  Start: {boot['start_line']}")
    for w in boot['witnesses']:
        print(f"  Witness: {w.strip()}")
    print("")

def diff_boots(boots1, boots2):
    print("=== DIFF BETWEEN TWO LOG FILES ===")
    
    # We'll just compare boot counts and classifications for now, or match by number
    b1_dict = {b['number']: b for b in boots1}
    b2_dict = {b['number']: b for b in boots2}
    
    all_nums = sorted(list(set(b1_dict.keys()) | set(b2_dict.keys())), key=int)
    for num in all_nums:
        b1 = b1_dict.get(num)
        b2 = b2_dict.get(num)
        
        if b1 and not b2:
            print(f"Boot {num} only in file 1")
        elif b2 and not b1:
            print(f"Boot {num} only in file 2")
        else:
            classes1 = set(b1['classes'])
            classes2 = set(b2['classes'])
            w1 = set([w.strip() for w in b1['witnesses']])
            w2 = set([w.strip() for w in b2['witnesses']])
            
            if classes1 != classes2 or w1 != w2:
                print(f"Boot {num} differs:")
                if classes1 != classes2:
                    print(f"  Classes: {list(classes1)} vs {list(classes2)}")
                if w1 != w2:
                    added = w2 - w1
                    removed = w1 - w2
                    if added:
                        print(f"  Added witnesses: {added}")
                    if removed:
                        print(f"  Removed witnesses: {removed}")
            else:
                print(f"Boot {num} matches exactly.")

def main():
    parser = argparse.ArgumentParser(description="Analyze serial captures")
    parser.add_argument("logs", nargs='+', help="Log files to parse (1 or 2 files)")
    args = parser.parse_args()
    
    if len(args.logs) > 2:
        print("Please provide 1 or 2 log files.")
        sys.exit(1)
        
    boots_list = []
    for log_file in args.logs:
        print(f"--- Parsing {log_file} ---")
        boots = parse_log(log_file)
        boots_list.append(boots)
        for b in boots:
            print_boot_summary(b)
            
    if len(boots_list) == 2:
        diff_boots(boots_list[0], boots_list[1])

if __name__ == '__main__':
    main()
