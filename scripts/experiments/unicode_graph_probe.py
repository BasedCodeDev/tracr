#!/usr/bin/env python3
"""Bounded feasibility probe, NOT Tracer's renderer. Standard library only.

Uses Git's coloured ASCII positions; never uses parent IDs to choose geometry.
Parent IDs are an independent oracle. Unsupported syntax/topology is rejected.
All repositories are automatically removed. Reports go only to --output.
"""
import argparse
from collections import defaultdict
from dataclasses import dataclass
import html
import json
import os
import platform
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time
import tracemalloc

ANSI = re.compile(r'\x1b\[([0-9;]*)m')
PALETTE = {31: '#e06c75', 32: '#98c379', 33: '#e5c07b',
           34: '#61afef', 35: '#c792ea', 36: '#56c8d8', None: '#cbd5e1'}


@dataclass
class Row:
    cells: list
    oid: str = ''
    subject: str = ''


class Unsupported(ValueError):
    pass


def parse(raw):
    rows = []
    for line in raw.splitlines():
        fields = line.split('\0')
        cells, color, offset = [], None, 0
        for match in ANSI.finditer(fields[0]):
            cells.extend((c, color) for c in fields[0][offset:match.start()])
            codes = [int(c or 0) for c in match[1].split(';')]
            for code in codes:
                if code in PALETTE:
                    color = code
                elif code in (0, 39):
                    color = None
            offset = match.end()
        cells.extend((c, color) for c in fields[0][offset:])
        rows.append(Row(cells, fields[1] if len(fields) > 1 else '',
                        fields[2] if len(fields) > 2 else ''))
    return rows


def transform(rows):
    """Infer local connections and rasterize on a doubled orthogonal grid.

    Deliberately bounded to the basic * | / \\ vocabulary. The doubled grid
    avoids inventing branch placement, but its density is itself under review.
    Returns glyphs, directed paths, and original commit coordinates.
    """
    source = {(x, y): (ch, color) for y, row in enumerate(rows)
              for x, (ch, color) in enumerate(row.cells) if ch != ' '}
    bad = sorted({ch for ch, _ in source.values()} - set('*|/\\'))
    if bad:
        raise Unsupported('unsupported Git graph glyphs: ' + repr(bad))
    paths = defaultdict(set)
    colors, ports = {}, defaultdict(set)
    nodes = {(next(x for x, (ch, _) in enumerate(row.cells) if ch == '*') * 2, y * 2): row.oid
             for y, row in enumerate(rows) if row.oid}

    def edge(a, b, color):
        paths[a].add(b)
        dx, dy = b[0] - a[0], b[1] - a[1]
        ports[a].add((dx, dy))
        ports[b].add((-dx, -dy))
        colors.setdefault(a, color)
        colors.setdefault(b, color)

    for (x, y), (ch, color) in source.items():
        a = (x * 2, y * 2)
        colors.setdefault(a, color)
        targets = []
        if ch in '*|':
            for nx, allowed in [(x, '*|'), (x - 1, '/'), (x + 1, '\\')]:
                if source.get((nx, y + 1), (' ', None))[0] in allowed:
                    targets.append(nx)
        else:
            nx = x + (1 if ch == '\\' else -1)
            if source.get((nx, y + 1), (' ', None))[0] in '*|/\\':
                targets.append(nx)
        if not targets and y == len(rows) - 1 and ch != '*':
            edge(a, (a[0], a[1] + 1), color)
        for nx in targets:
            b = (nx * 2, y * 2 + 2)
            cur = a
            nxt = (cur[0], cur[1] + 1)
            edge(cur, nxt, color)
            cur = nxt
            while cur[0] != b[0]:
                nxt = (cur[0] + (1 if b[0] > cur[0] else -1), cur[1])
                edge(cur, nxt, color)
                cur = nxt
            edge(cur, b, color)
    glyphs = {}
    chars = {
        frozenset([(0, -1), (0, 1)]): '│',
        frozenset([(-1, 0), (1, 0)]): '─',
        frozenset([(0, 1), (1, 0)]): '╭',
        frozenset([(0, 1), (-1, 0)]): '╮',
        frozenset([(0, -1), (1, 0)]): '╰',
        frozenset([(0, -1), (-1, 0)]): '╯',
        frozenset([(0, -1), (0, 1), (1, 0)]): '├',
        frozenset([(0, -1), (0, 1), (-1, 0)]): '┤',
        frozenset([(-1, 0), (1, 0), (0, 1)]): '┬',
        frozenset([(-1, 0), (1, 0), (0, -1)]): '┴',
    }
    for point in colors:
        mask = frozenset(ports[point])
        if point in nodes:
            ch = '●'
        elif len(mask) == 4:
            raise Unsupported('ambiguous crossing/junction at ' + str(point))
        elif len(mask) < 2:
            ch = '│' if not mask or next(iter(mask))[0] == 0 else '─'
        else:
            ch = chars[mask]
        glyphs[point] = (ch, colors[point])
    return glyphs, dict(paths), nodes


def oracle(rendered, parents):
    """Follow only the rendered directed strokes, stopping at the next commit."""
    glyphs, paths, nodes = rendered
    loaded = set(nodes.values())
    bottom = max((p[1] for p in glyphs), default=0)
    errors = []
    for start, oid in nodes.items():
        todo, seen, actual, tails = list(paths.get(start, ())), set(), set(), []
        while todo:
            p = todo.pop()
            if p in seen:
                continue
            seen.add(p)
            if p in nodes:
                actual.add(nodes[p])
                continue
            children = paths.get(p, ())
            if not children:
                tails.append(p)
            todo.extend(children)
        expected = set(parents[oid]) & loaded
        if actual != expected:
            errors.append(f'{oid[:7]}: expected parents {sorted(p[:7] for p in expected)}, rendered {sorted(p[:7] for p in actual)}')
        missing = set(parents[oid]) - loaded
        if missing and not tails:
            errors.append(oid[:7] + ': unloaded ancestry has no open tail')
        if any(p[1] < bottom for p in tails):
            errors.append(oid[:7] + ': dangling connector above boundary')
        if tails and not missing:
            errors.append(oid[:7] + ': spurious unfinished ancestry')
    return errors


def git(repo, *args, data=None):
    return subprocess.run(['git', '-C', str(repo), *args], input=data,
                          capture_output=True, text=True, check=True, timeout=60).stdout


def fixture(root, name, parents, refs=None, detached=None):
    repo = root / name
    repo.mkdir()
    git(repo, 'init', '-q', '--initial-branch=main')
    stream = []
    for i, ps in enumerate(parents, 1):
        message = f'{name} commit {i}'
        stream.extend(['reset refs/heads/build\n\n', 'commit refs/heads/build\n',
                       f'mark :{i}\n', f'committer Probe <probe@example.invalid> {1700000000+i} +0000\n',
                       f'data {len(message)}\n{message}\n'])
        if ps:
            stream.append(f'from :{ps[0]}\n')
            stream.extend(f'merge :{p}\n' for p in ps[1:])
        stream.append('\n')
    for ref, mark in (refs or {'main': len(parents)}).items():
        stream.append(f'reset refs/heads/{ref}\nfrom :{mark}\n\n')
    stream.append('reset refs/heads/build\n\ndone\n')
    git(repo, 'fast-import', '--quiet', data=''.join(stream))
    if detached:
        oid = git(repo, 'rev-parse', detached).strip()
        git(repo, 'update-ref', '--no-deref', 'HEAD', oid)
        git(repo, 'update-ref', '-d', 'refs/heads/detached')
    return repo


def history(repo, limit, head=False):
    return parse(git(repo, '-c', 'log.graphColors=cyan,magenta,yellow,green,blue,red',
                     'log', '--all', '--graph', '--topo-order', '--color=always',
                     '--no-show-signature', '--no-notes', '--format=%x00%H%x00%s%x00',
                     f'--max-count={limit}', *(['HEAD'] if head else []), '--'))


def parents_of(repo):
    return {fields[0]: fields[1:] for line in git(repo, 'rev-list', '--all', 'HEAD', '--parents').splitlines()
            if (fields := line.split())}


def svg_preview(rows, rendered):
    glyphs, _, nodes = rendered
    labels = {row.oid: row.subject for row in rows if row.oid}
    width = max(x for x, _ in glyphs) + 4
    height = max(y for _, y in glyphs) + 1
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="{height*20+70}">',
           '<rect width="100%" height="100%" fill="#171b22"/>',
           '<g font-family="DejaVu Sans Mono,monospace" font-size="16">',
           '<text x="20" y="25" fill="#dce4ef">Experimental output — actual Git fixture; not approved for integration</text>']
    for (x, y), (ch, color) in sorted(glyphs.items(), key=lambda p: (p[0][1], p[0][0])):
        out.append(f'<text x="{20+x*10}" y="{55+y*20}" fill="{PALETTE[color]}">{ch}</text>')
    for (x, y), oid in nodes.items():
        out.append(f'<text x="{20+width*10}" y="{55+y*20}" fill="#dce4ef">{oid[:7]} {html.escape(labels[oid])}</text>')
    out.append('</g></svg>')
    return '\n'.join(out)


def measure(rows):
    samples = []
    for _ in range(7):
        start = time.perf_counter_ns()
        rendered = transform(rows)
        samples.append((time.perf_counter_ns() - start) / 1e6)
    tracemalloc.start()
    rendered = transform(rows)
    retained, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return dict(median_ms=round(statistics.median(samples), 3),
                retained_bytes=retained, peak_bytes=peak,
                input_cells=sum(len(r.cells) for r in rows),
                output_cells=len(rendered[0]))


FIXTURES = {
    'linear': ([[], [1], [2], [3]], None, None),
    'merge': ([[], [1], [1], [2, 3]], {'main': 4, 'side': 3}, None),
    'four-lanes': ([[], [1], [1], [2], [3], [4], [5], [6, 7], [4], [9], [7]],
                   {'main': 8, 'navigation': 10, 'release': 7, 'fix': 11}, None),
    'parallel-four': ([[], [1], [2], [1], [4], [1], [6], [1], [8]],
                      {'main': 3, 'navigation': 5, 'release': 7, 'fix': 9}, None),
    'octopus': ([[], [1], [1], [1], [1], [2, 3, 4, 5]], None, None),
    'crossing': ([[], [1], [1], [2, 3], [3, 2], [4, 5]], None, None),
    'disconnected': ([[], [1], [], [3]], {'main': 4, 'other': 2}, None),
    'detached': ([[], [1], [1], [3]], {'main': 2, 'detached': 4}, 'detached'),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'git': subprocess.check_output(['git', '--version'], text=True).strip(),
              'python': platform.python_version(), 'platform': platform.platform(),
              'cases': [], 'benchmarks': [], 'prefixes': []}
    # Clear ambient Git routing/config; fixtures must not touch a user's repository.
    for key in list(os.environ):
        if key.startswith('GIT_'):
            del os.environ[key]
    os.environ.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull)
    with tempfile.TemporaryDirectory(prefix='tracr-unicode-probe-') as tmp:
        root = Path(tmp)
        cases = FIXTURES
        for name, (ps, refs, detached) in cases.items():
            repo = fixture(root, name, ps, refs, detached)
            parents = parents_of(repo)
            full = history(repo, 100, bool(detached))
            (args.output / f'{name}-native.txt').write_text('\n'.join(
                ''.join(ch for ch, _ in row.cells) + row.oid[:7] + ' ' + row.subject for row in full) + '\n')
            for limit in range(1, len(ps) + 1):
                rows = history(repo, limit, bool(detached))
                entry = dict(fixture=name, limit=limit)
                try:
                    rendered = transform(rows)
                    errors = oracle(rendered, parents)
                    entry.update(status='rejected' if errors else 'passed', errors=errors)
                    if limit == len(ps):
                        (args.output / f'{name}-prototype.svg').write_text(svg_preview(rows, rendered))
                        entry['row_expansion'] = round((max(y for _, y in rendered[0]) + 1) / len(rows), 2)
                except Unsupported as e:
                    entry.update(status='unsupported', errors=[str(e)])
                report['cases'].append(entry)
        # Repeated merges and concurrent refs, with boundaries inside merge groups.
        ps = [[]]
        for _ in range(180):
            base = len(ps)
            ps.extend([[base], [base], [base + 1, base + 2]])
        repo = fixture(root, 'prefixes', ps, {'main': len(ps), 'side': len(ps)-1})
        previous = None
        for limit in (128, 256, 512):
            rows = history(repo, limit)
            refreshed = history(repo, limit)
            entry = dict(limit=limit, refresh_equal=rows == refreshed)
            if previous:
                # Exclude only the prior last commit and its trailing connector rows.
                cutoff = max(i for i, row in enumerate(previous) if row.oid)
                entry['native_completed_rows_stable'] = previous[:cutoff] == rows[:cutoff]
            try:
                rendered = transform(rows)
                entry['topology_errors'] = oracle(rendered, parents_of(repo))[:5]
                if previous:
                    old = transform(previous)[0]
                    new = rendered[0]
                    entry['unicode_completed_rows_stable'] = all(new.get(p) == v for p, v in old.items() if p[1] < cutoff * 2)
            except Unsupported as e:
                entry['unsupported'] = str(e)
            report['prefixes'].append(entry)
            previous = rows
        repo = fixture(root, 'benchmark', [[]] + [[i] for i in range(1, 10000)])
        for limit in (128, 512, 2048, 10000):
            start = time.perf_counter_ns()
            rows = history(repo, limit)
            read_ms = (time.perf_counter_ns() - start) / 1e6
            report['benchmarks'].append(dict(commits=limit, shape='linear', git_read_parse_ms=round(read_ms, 3), **measure(rows)))
        # A wider case is needed so linear timing cannot masquerade as width coverage.
        ps = [[]]
        refs = {'main': 1}
        for branch in range(8):
            parent = 1
            for _ in range(64):
                ps.append([parent])
                parent = len(ps)
            refs[f'lane-{branch}'] = parent
        repo = fixture(root, 'wide', ps, refs)
        rows = history(repo, 512)
        try:
            report['benchmarks'].append(dict(commits=512, shape='eight-lanes', **measure(rows)))
        except Unsupported as e:
            report['benchmarks'].append(dict(commits=512, shape='eight-lanes', unsupported=str(e)))
    report['topology_gate_passed'] = all(c['status'] == 'passed' for c in report['cases'])
    report['accepted_for_integration'] = False  # Requires visual and UI acceptance as well.
    report['note'] = 'Probe completed; rejected/unsupported cases are evidence, not hidden test successes.'
    (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'cases': len(report['cases']), 'passed': sum(c['status'] == 'passed' for c in report['cases']),
                      'results': str(args.output / 'results.json'), 'benchmarks': report['benchmarks'],
                      'prefixes': report['prefixes']}, indent=2))


if __name__ == '__main__':
    main()
