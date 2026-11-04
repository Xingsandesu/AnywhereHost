#!/usr/bin/env python3
"""Compare release hot paths in disposable SQLite fixtures; no real Codex or user state.

--baseline archives a Git revision in a temporary directory and injects only the
current measurement harness. Results count Rust allocation traffic, not RSS or
SQLite's native allocations. Run on an otherwise idle machine.
"""
import argparse
import io
import json
import pathlib
import platform
import re
import statistics
import subprocess
import tarfile
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--baseline', help='Git revision to compare against')
parser.add_argument('--repeats', type=int, default=3)
parser.add_argument('--output', type=pathlib.Path)
args = parser.parse_args()
if args.repeats < 1:
    parser.error('--repeats must be positive')


def run(root):
    samples = []
    memory = []
    for _ in range(args.repeats):
        result = subprocess.run([
            'cargo', 'test', '--locked', '--release', '--bin', 'anywhere-host',
            '--manifest-path', str(root / 'Cargo.toml'),
            'benchmark_', '--', '--ignored', '--test-threads=1', '--nocapture',
        ], check=True, text=True, stdout=subprocess.PIPE)
        rows = [json.loads(line.split('BENCH ', 1)[1]) for line in result.stdout.splitlines() if 'BENCH ' in line]
        if len(rows) != 4:
            raise RuntimeError('Expected all four benchmark measurements')
        samples.append({row['name']: row for row in rows})
        memory.extend(json.loads(line.split('MEMORY ', 1)[1]) for line in result.stdout.splitlines() if 'MEMORY ' in line)
    timings = {
        name: {
            'nsPerOp': statistics.median(sample[name]['nsPerOp'] for sample in samples),
            'allocatedBytesPerOp': statistics.median(sample[name]['allocatedBytesPerOp'] for sample in samples),
            'samples': [sample[name] for sample in samples],
        }
        for name in samples[0]
    }
    if len(memory) != args.repeats:
        raise RuntimeError('Missing fan-out buffer memory measurements')
    return timings, {'eventWireBytes32Clients': statistics.median(row['eventWireBytes32Clients'] for row in memory)}


report = {'platform': platform.platform(), 'rustc': subprocess.check_output(['rustc', '-V'], text=True).strip(), 'repeats': args.repeats}
if args.baseline:
    revision = subprocess.check_output(['git', 'rev-parse', '--verify', args.baseline + '^{commit}'], cwd=ROOT, text=True).strip()
    report['baselineRevision'] = revision
    with tempfile.TemporaryDirectory(prefix='anywhere-benchmark-') as temporary:
        root = pathlib.Path(temporary)
        archive = subprocess.check_output(['git', 'archive', revision], cwd=ROOT)
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(root, filter='data')
        source = root / 'backend/src'
        (source / 'benchmark.rs').write_text((ROOT / 'backend/src/benchmark.rs').read_text())
        if 'mod benchmark;' not in (source / 'main.rs').read_text():
            with (source / 'main.rs').open('a') as file:
                file.write('\n#[cfg(test)]\nmod benchmark;\n')
        for name in ('events.rs', 'security.rs'):
            if 'fn benchmark_' in (source / name).read_text():
                continue
            current = (ROOT / 'backend/src' / name).read_text()
            test = re.search(r'    #\[test\]\n    #\[ignore[^\n]+\]\n    fn benchmark_[\s\S]+?(?=\n    #\[test\]|\n})', current)
            if not test:
                raise RuntimeError(f'Missing benchmark in {name}')
            code = test.group().replace('events.wire(event.sequence).unwrap()', 'serde_json::to_string(&events.get(event.sequence).unwrap()).unwrap()')
            code = code.replace('frame.text', 'frame')
            if name == 'events.rs':
                code = 'use serde_json::json;\n' + code
            with (source / name).open('a') as file:
                file.write('\n#[cfg(test)]\nmod performance_baseline {\nuse super::*;\n' + code + '\n}\n')
        report['before'], report['beforeMemory'] = run(root)
report['after'], report['afterMemory'] = run(ROOT)
if 'before' in report:
    report['speedup'] = {name: report['before'][name]['nsPerOp'] / row['nsPerOp'] for name, row in report['after'].items()}
encoded = json.dumps(report, indent=2) + '\n'
if args.output:
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(encoded)
print(encoded, end='')
