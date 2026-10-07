#!/usr/bin/env python3
"""Check that every typed context field has a complete reference-table entry."""
import argparse
from collections import Counter
from pathlib import Path
import re

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source-root', type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument('--doc', type=Path)
args = parser.parse_args()
source = (args.source_root / 'src/db/contexts.rs').read_text()
doc_path = args.doc or args.source_root / 'docs/CONTEXT_VARIABLES.md'
doc = doc_path.read_text()
expected = {}
counts = {}
for struct, prefix in [('Context', ''), ('ContextSettings', 'settings.')]:
    match = re.search(r'pub struct ' + struct + r'\s*\{(.*?)\n\}', source, re.S)
    if not match:
        raise SystemExit(f'Cannot find Rust struct {struct}')
    fields = re.findall(r'^\s*pub (\w+):\s*([^\n]+),', match[1], re.M)
    if not fields:
        raise SystemExit(f'No fields extracted from {struct}')
    counts[struct] = len(fields)
    expected.update({prefix + name: kind.strip() for name, kind in fields})

section = doc.split('## Root context fields\n', 1)[1].split('## Runtime POML variables', 1)[0]
rows = {}
seen = Counter()
errors = []
for line in section.splitlines():
    if not line.startswith('| `'):
        continue
    cells = [cell.strip() for cell in line.strip().strip('|').split('|')]
    key = cells[0].strip('`')
    seen[key] += 1
    rows[key] = cells
    if len(cells) != 5 or not all(cells):
        errors.append(f'{key}: expected non-empty variable/type/allowed/default/effect cells')
for key in sorted(set(expected) - set(rows)):
    errors.append('Missing: ' + key)
for key in sorted(set(rows) - set(expected)):
    errors.append('Unknown typed field: ' + key)
for key, count in seen.items():
    if count != 1:
        errors.append(f'Duplicate: {key} ({count} rows)')

kinds = {'String': 'string', 'bool': 'boolean', 'i32': 'I32', 'usize': 'Usize', 'f32': 'F32', 'f64': 'F64',
         'Vec<String>': 'array of strings', 'serde_json::Value': 'JSON', 'ContextSettings': 'object',
         'std::collections::HashMap<String, Vec<String>>': 'object: name → array of strings'}
for key, rust_type in expected.items():
    if key not in rows or len(rows[key]) != 5:
        continue
    optional = rust_type.startswith('Option<')
    base = rust_type[7:-1] if optional else rust_type
    expected_type = kinds.get(base)
    if expected_type is None:
        errors.append(f'New Rust type for {key}: {rust_type}; extend this check')
        continue
    expected_type += ' or null' if optional else ''
    if rows[key][1] != expected_type:
        errors.append(f'{key}: documented type {rows[key][1]!r}, expected {expected_type!r}')
if errors:
    raise SystemExit('\n'.join(errors))
print(f'PASS: {counts["Context"]} root + {counts["ContextSettings"]} settings = {len(expected)} fields; '
      'all have matching types, allowed values, defaults, and effects.')
print('Checked:', doc_path)
