#!/usr/bin/env python3
"""Run the isolated gleisbau binary on real Git DAGs; never enables it in Tracer."""
import argparse
import hashlib
import platform
from collections import Counter
import html
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time

from unicode_graph_probe import FIXTURES, fixture, git, history

ANSI = re.compile(r'\x1b\[([0-9;]*)m')
COLORS = {1: '#e06c75', 2: '#98c379', 3: '#e5c07b', 4: '#61afef',
          5: '#c792ea', 6: '#56c8d8', 7: '#cbd5e1', None: '#cbd5e1'}
N, S, E, W = (0, -1), (0, 1), (1, 0), (-1, 0)
PORTS = {'│': (N, S), '─': (W, E), '╭': (S, E), '╮': (S, W),
         '╰': (N, E), '╯': (N, W), '├': (N, S, E), '┤': (N, S, W),
         '┬': (W, E, S), '┴': (W, E, N), '┼': (N, S, W, E),
         '●': (N, S, W, E), '○': (N, S, W, E), '<': (W, E), '>': (W, E)}


def cells(line):
    result, offset, color = [], 0, None
    for m in ANSI.finditer(line):
        result.extend((c, color) for c in line[offset:m.start()])
        codes = [int(c or 0) for c in m[1].split(';')]
        if len(codes) == 3 and codes[:2] == [38, 5]:
            color = codes[2]
        elif codes == [0]:
            color = None
        else:
            raise ValueError('unexpected colour escape ' + m[0])
        offset = m.end()
    result.extend((c, color) for c in line[offset:])
    return result


def oracle(output, commits):
    """Trace actual Unicode ports, not the library's internal parent lists.

    CROSS means pass straight through, not a junction. Movement is downward or
    horizontal, since input is topological newest-first. Nodes stop traversal.
    """
    if output['status'] != 'rendered':
        return [output.get('error', output['status'])]
    grid = {(x, y): ch for y, line in enumerate(output['lines'])
            for x, (ch, _) in enumerate(cells(line)) if ch != ' '}
    nodes = {(p['column'], p['row']): p['oid'] for p in output['positions']}
    errors = []
    for point, oid in nodes.items():
        if grid.get(point) not in ('●', '○'):
            errors.append(f'{oid[:7]}: commit-to-row map misses node at {point}')
    unknown = set(grid.values()) - PORTS.keys()
    if unknown:
        return errors + ['unsupported oracle glyphs: ' + repr(sorted(unknown))]
    expected_nodes = {c['oid'] for c in commits}
    if set(nodes.values()) != expected_nodes or len(nodes) != len(commits) or len(output['positions']) != len(commits):
        errors.append('commit mapping loses or duplicates an OID')
    starts_by_oid = {}
    for point, oid in nodes.items():
        starts_by_oid.setdefault(oid, []).append(point)
    bottom = len(output['lines']) - 1
    for c in commits:
        starts = starts_by_oid.get(c['oid'], [])
        if not starts:
            continue
        start = starts[0]
        todo = [(start, None)]
        seen, actual, tails = set(), set(), set()
        while todo:
            point, arrival = todo.pop()
            state = (point, arrival)
            if state in seen:
                continue
            seen.add(state)
            if point in nodes and point != start:
                actual.add(nodes[point])
                continue
            ch = grid.get(point, ' ')
            choices = PORTS.get(ch, ())
            if ch in '<>':
                # Gleisbau arrows point toward the merge CHILD; ancestry flows
                # in the opposite direction, never back into another child.
                choices = (E,) if ch == '<' else (W,)
            if ch == '┼' and arrival is not None:
                # Opposite the port through which this path entered the crossing.
                choices = ((-arrival[0], -arrival[1]),)
            advanced = False
            for direction in choices:
                if direction == N or direction == arrival:
                    continue
                target = (point[0] + direction[0], point[1] + direction[1])
                reverse = (-direction[0], -direction[1])
                arrow = grid.get(target)
                if arrow in ('<', '>') and direction != (E if arrow == '<' else W):
                    continue
                if reverse in PORTS.get(grid.get(target), ()):
                    advanced = True
                    todo.append((target, reverse))
                elif direction == S and point[1] == bottom and ch not in ('●', '○'):
                    tails.add(point)
            physically_connected = any(
                direction != arrival and (-direction[0], -direction[1]) in PORTS.get(
                    grid.get((point[0]+direction[0], point[1]+direction[1])), ())
                for direction in PORTS.get(ch, ()))
            # An upward edge or child-facing arrow can be a valid branch that
            # ancestry traversal intentionally does not follow. A true dangling
            # stroke has no onward physical connection at all.
            if not advanced and not physically_connected and ch not in ('●', '○') and point[1] < bottom:
                errors.append(c['oid'][:7] + ': dangling rendered stroke above boundary at ' + str(point))
        expected = set(c['parents']) & expected_nodes
        missing = set(c['parents']) - expected_nodes
        if actual != expected:
            errors.append(f"{c['oid'][:7]}: expected {sorted(x[:7] for x in expected)}, drawn {sorted(x[:7] for x in actual)}")
        if missing and not tails:
            errors.append(c['oid'][:7] + ': no open continuation for unloaded parent')
        if tails and not missing:
            errors.append(c['oid'][:7] + ': false open continuation')
    return errors


def payload(repo, limit):
    commits = []
    for line in git(repo, 'log', '--all', 'HEAD', '--topo-order', '--format=%H%x00%P%x00%s', f'--max-count={limit}').splitlines():
        oid, parents, subject = line.split('\0')
        commits.append(dict(oid=oid, parents=parents.split(), subject=subject))
    refs = {}
    for line in git(repo, 'for-each-ref', '--format=%(objectname) %(refname:short)').splitlines():
        oid, name = line.split(' ', 1)
        refs.setdefault(oid, name)
    return dict(commits=commits, refs=refs)


def run(binary, data):
    start = time.monotonic()
    try:
        p = subprocess.run([str(binary)], input=json.dumps(data), capture_output=True, text=True, timeout=30)
        if p.returncode:
            return dict(status='process-error', error=p.stderr[-2000:], wall_seconds=time.monotonic()-start)
        result = json.loads(p.stdout)
        result['stderr'] = p.stderr[-2000:]
        result['wall_seconds'] = time.monotonic()-start
        return result
    except subprocess.TimeoutExpired:
        return dict(status='timeout', error='30 second process limit exceeded')


def preview(output, data):
    lines = output['lines']
    width = max((len(cells(line)) for line in lines), default=0)
    positions = {p['row']: p for p in output['positions']}
    commits = {c['oid']: c for c in data['commits']}
    s = [f'<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="{len(lines)*24+70}">',
         '<rect width="100%" height="100%" fill="#171b22"/>',
         '<g font-family="DejaVu Sans Mono,monospace" font-size="20">',
         '<text x="20" y="25" font-size="16" fill="#cbd5e1">gleisbau 0.7.5 — actual experimental output, not enabled in Tracer</text>']
    for y, line in enumerate(lines):
        for x, (ch, color) in enumerate(cells(line)):
            if ch != ' ':
                s.append(f'<text x="{20+x*12}" y="{55+y*24}" fill="{COLORS.get(color, COLORS[None])}">{html.escape(ch)}</text>')
        if y in positions:
            oid = positions[y]['oid']
            label = data['refs'].get(oid, '')
            text = oid[:7] + ' ' + commits[oid]['subject'] + (' ['+label+']' if label else '')
            s.append(f'<text x="{44+width*12}" y="{55+y*24}" font-size="16" fill="#dce4ef">{html.escape(text)}</text>')
    return '\n'.join(s + ['</g></svg>'])


def compare_prefix(old, new):
    oldpos = old['positions']
    # Only the last old commit / its trailing boundary rows may resolve differently.
    stable = oldpos[:-1]
    fresh = {p['oid']: p for p in new['positions']}
    changes = [p['oid'][:7] for p in stable if fresh.get(p['oid']) != p]
    cutoff = oldpos[-1]['row'] if oldpos else 0
    return dict(stable_commit_positions=not changes, moved_oids=changes[:8],
                completed_rows_equal=old['lines'][:cutoff] == new['lines'][:cutoff])


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    for key in list(os.environ):
        if key.startswith('GIT_'):
            del os.environ[key]
    os.environ.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull)
    report = dict(version='gleisbau 0.7.5', git=subprocess.check_output(['git','--version'],text=True).strip(),
                  python=platform.python_version(), platform=platform.platform(),
                  binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  cases=[], prefixes=[], benchmarks=[])
    cases = FIXTURES
    with tempfile.TemporaryDirectory(prefix='tracr-gleisbau-probe-') as tmp:
        root = Path(tmp)
        for name, (ps, refs, detached) in cases.items():
            repo = fixture(root, name, ps, refs, detached)
            for limit in range(1, len(ps)+1):
                data = payload(repo, limit)
                result = run(binary, data)
                errors = oracle(result, data['commits'])
                report['cases'].append(dict(fixture=name, limit=limit, status=result['status'], errors=errors))
                if name == 'four-lanes' and limit == 2:
                    (args.output / 'four-lanes-prefix-2.json').write_text(json.dumps(dict(input=data,output=result),indent=2)+'\n')
                if limit == len(ps):
                    (args.output / f'{name}.json').write_text(json.dumps(dict(input=data, output=result), indent=2)+'\n')
                    if result['status'] == 'rendered':
                        (args.output / f'{name}.svg').write_text(preview(result, data))
            if name == 'linear':
                shallow = root / 'shallow'
                git(root, 'clone', '--quiet', '--depth=2', repo.as_uri(), str(shallow))
                data = payload(shallow, 10)
                result = run(binary, data)
                report['cases'].append(dict(fixture='shallow', limit=10, status=result['status'], errors=oracle(result,data['commits'])))
        result = run(binary, {'commits': [], 'refs': {}})
        report['cases'].append(dict(fixture='empty', limit=0, status=result['status'], errors=oracle(result, [])))
        ps = [[]]
        for _ in range(180):
            base = len(ps)
            ps.extend([[base], [base], [base+1, base+2]])
        repo = fixture(root, 'prefix', ps, {'main':len(ps), 'side':len(ps)-1})
        previous = None
        for limit in (128,256,512):
            data = payload(repo,limit)
            result = run(binary,data)
            again = run(binary,data)
            entry = dict(limit=limit, status=result['status'], errors=oracle(result,data['commits'])[:10])
            if result['status']=='rendered':
                entry['repeat_equal'] = result['lines']==again.get('lines') and result['positions']==again.get('positions')
                if previous and previous['status']=='rendered':
                    entry.update(compare_prefix(previous,result))
                (args.output / f'prefix-{limit}.json').write_text(json.dumps(dict(input=data,output=result),indent=2)+'\n')
            report['prefixes'].append(entry)
            previous=result
        # Also vary prefix size in the small graphs; merge-heavy large histories
        # alone do not expose length-dependent track packing.
        for name in ('four-lanes','parallel-four','crossing'):
            repo=root/name
            old=None
            for limit in range(2,len(cases[name][0])+1):
                result=run(binary,payload(repo,limit))
                if old and old['status']==result['status']=='rendered':
                    report['prefixes'].append(dict(fixture=name,limit=limit,**compare_prefix(old,result)))
                old=result
        report['ordering_policies'] = []
        for order in ('shortest-reverse', 'shortest-forward', 'longest-reverse', 'longest-forward'):
            variants = []
            for name in ('four-lanes', 'parallel-four', 'octopus', 'crossing', 'disconnected', 'prefix'):
                limits = (128, 256, 512) if name == 'prefix' else range(1, len(cases[name][0])+1)
                old = None
                changes = 0
                errors = 0
                for limit in limits:
                    data = payload(root/name, limit)
                    data['order'] = order
                    result = run(binary, data)
                    errors += bool(oracle(result, data['commits']))
                    if old and old['status'] == result['status'] == 'rendered':
                        changes += not compare_prefix(old, result)['completed_rows_equal']
                    old = result
                variants.append(dict(fixture=name, rejected_prefixes=errors, unstable_expansions=changes))
            report['ordering_policies'].append(dict(order=order, fixtures=variants))
        linear=fixture(root,'benchmark' ,[[]]+[[i] for i in range(1,10000)])
        wideparents=[[]]
        tips=[1]*8
        # Interleave branches and close with an octopus so eight lanes coexist.
        for _ in range(64):
            for b in range(8):
                wideparents.append([tips[b]])
                tips[b]=len(wideparents)
        wideparents.append(tips)
        wide=fixture(root,'wide',wideparents)
        for repo,limits in ((linear,(128,512,2048,10000)),(wide,(128,512))):
            for limit in limits:
                data=payload(repo,limit)
                samples=[run(binary,data) for _ in range(7)]
                good=[r for r in samples if r['status']=='rendered']
                entry=dict(shape=repo.name,commits=len(data['commits']),status=samples[0]['status'])
                if repo == wide:
                    entry['topology_errors'] = oracle(samples[0], data['commits'])[:8]
                    entry['native_graph_columns'] = max(len(row.cells) for row in history(repo, limit))
                if good:
                    entry.update(timing_us={k:statistics.median(r['timing_us'][k] for r in good) for k in ('ingest','layout','render')},
                                 retained_working_bytes=statistics.median(r['retained_working_bytes'] for r in good),
                                 peak_working_bytes=statistics.median(r['peak_working_bytes'] for r in good),
                                 max_columns=max(len(cells(line)) for line in good[0]['lines']))
                else:
                    entry['error']=samples[0].get('error')
                report['benchmarks'].append(entry)
    report['summary']=dict(cases=len(report['cases']),passed=sum(not c['errors'] for c in report['cases']),
                           statuses=dict(Counter(c['status'] for c in report['cases'])),
                           unstable_prefixes=sum(not c.get('completed_rows_equal',True) for c in report['prefixes']))
    report['accepted_for_integration'] = False
    (args.output/'results.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(dict(summary=report['summary'],benchmarks=report['benchmarks'],prefixes=report['prefixes'][:3]),indent=2))


if __name__=='__main__':
    main()
