#!/usr/bin/env python3
"""Supplement the isolated library trial with scale, cached viewports and previews."""
import argparse
import hashlib
import html
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time

from unicode_graph_probe import FIXTURES, fixture, git
from gleisbau_probe import payload, run, oracle, cells, COLORS, compare_prefix


def svg(view, title):
    width = view['request']['width']
    rows = view['rows']
    output = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{max(480, 24+width*12)}" height="{70+24*len(rows)}">',
              '<rect width="100%" height="100%" fill="#171b22"/>',
              '<style>text{font-family:"DejaVu Sans Mono",monospace;font-size:20px}</style>',
              f'<text x="12" y="24" fill="#cbd5e1" style="font-size:13px">{html.escape(title)}</text>']
    # A line's tspans preserve the adapter's text verbatim, including clipped spaces.
    for index,row in enumerate(rows):
        spans = ''.join(f'<tspan fill="{COLORS.get(span["color"], COLORS[None])}">{html.escape(span["text"])}</tspan>' for span in row)
        output.append(f'<text x="12" y="{54+index*24}" xml:space="preserve">{spans}</text>')
    return '\n'.join(output+['</svg>'])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args(); binary=args.binary.resolve(); args.output.mkdir(parents=True,exist_ok=True)
    for key in list(os.environ):
        if key.startswith('GIT_'): del os.environ[key]
    os.environ.update(GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=os.devnull)
    report=dict(binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), git=git(Path.cwd(),'--version').strip(),
                platform=platform.platform(), previews=[], benchmarks=[], prefix_stability=[], accepted_for_integration=False)
    with tempfile.TemporaryDirectory(prefix='gleisbau-acceptance-') as tmp:
        root=Path(tmp)
        for name in ('merge','parallel-four','crossing','octopus','four-lanes'):
            ps,refs,detached=FIXTURES[name]; repo=fixture(root,name,ps,refs,detached)
            for limit in ([2,len(ps)] if name=='four-lanes' else [len(ps)]):
                data=payload(repo,limit)
                for commit in data['commits']: commit['subject'] += ' · Café e\u0301 界 👩‍💻'
                data['viewports']=[dict(x=0,y=0,width=96,height=40),dict(x=0,y=0,width=32,height=12),
                                   dict(x=1,y=1,width=32,height=8),dict(x=0,y=0,width=32,height=12)]
                result=run(binary,data); errors=oracle(result,data['commits'])
                entry=dict(fixture=name,limit=limit,status=result['status'],topology_errors=errors)
                if result['status']=='rendered':
                    assert 'viewport_error' not in result, result.get('viewport_error')
                    assert result['render_passes']==1
                    views=result['viewports']; assert views[1]==views[3]
                    for view in views:
                        y=view['request']['y']; height=view['request']['height']
                        expected=[(p['oid'],p['row']-y) for p in result['positions'] if y <= p['row'] < y+height]
                        assert [(p['oid'],p['row']) for p in view['positions']]==expected
                    entry['cached_viewports_passed']=True
                    entry['rows_per_commit']=len(result['lines'])/max(1,len(data['commits']))
                    entry['graph_columns']=max(map(lambda s:len(cells(s)),result['lines']),default=0)
                    for index,label in enumerate(('wide','narrow','scrolled')):
                        stem=f'{name}-{limit}-{label}'
                        (args.output/f'{stem}.svg').write_text(svg(views[index],f'Gleisbau 0.7.5 — {name}, {limit} commits — experimental'))
                        (args.output/f'{stem}.txt').write_text('\n'.join(''.join(s['text'] for s in row) for row in views[index]['rows'])+'\n')
                    (args.output/f'{name}-{limit}.ansi').write_text('\n'.join(result['lines'])+'\n')
                    (args.output/f'{name}-{limit}.json').write_text(json.dumps(dict(input=data,output=result),indent=2)+'\n')
                report['previews'].append(entry)
        # Git log hides parents at shallow boundaries. Read the raw commit header
        # so the trial cannot mistake a shallow boundary for a genuine root.
        shallow_source=fixture(root,'shallow-source',[[],[1],[2],[3]])
        shallow=root/'shallow'
        git(root,'clone','--quiet','--depth=2',shallow_source.as_uri(),str(shallow))
        data=payload(shallow,10)
        boundary=data['commits'][-1]
        boundary['parents']=[line.split()[1] for line in git(shallow,'cat-file','-p',boundary['oid']).splitlines() if line.startswith('parent ')]
        boundary['boundary']='shallow'
        boundary['subject']+=' [shallow boundary]'
        data['viewports']=[dict(x=0,y=0,width=96,height=12)]
        output=run(binary,data)
        errors=oracle(output,data['commits'])
        report['shallow_boundary']=dict(status=output['status'],raw_parent_ids=boundary['parents'],topology_errors=errors)
        if output['status']=='rendered' and 'viewports' in output:
            (args.output/'shallow-boundary.svg').write_text(svg(output['viewports'][0],'Gleisbau — explicit shallow ancestry — experimental'))
        (args.output/'shallow-boundary.json').write_text(json.dumps(dict(input=data,output=output),indent=2)+'\n')
        linear=fixture(root,'linear-30000',[[]]+[[i] for i in range(1,30000)])
        # Same 64 × 16 topology as the browser acceptance fixture, plus 64 merges.
        parents=[[]]; heads=[]
        for branch in range(64):
            parent=1
            for i in range(16): parents.append([parent]); parent=len(parents)
            heads.append(parent)
        main=1
        for head in heads: parents.append([main,head]); main=len(parents)
        refs={f'branch-{i:02}':head for i,head in enumerate(heads)}; refs['main']=main
        wide=fixture(root,'wide-64',parents,refs)
        for repo,limits in [(linear,(128,512,2048,10000,30000)),(wide,(128,512,len(parents)))]:
            previous=None
            for limit in limits:
                data=payload(repo,limit); samples=[run(binary,data) for _ in range(7)]
                output=samples[0]; rendered=[r for r in samples if r['status']=='rendered']
                entry=dict(fixture=repo.name,commits=len(data['commits']),statuses=[r['status'] for r in samples])
                if rendered:
                    errors=oracle(output,data['commits'])
                    entry.update(topology_error_count=len(errors),topology_errors=errors[:8],
                                 repeat_equal=all(r['lines']==output['lines'] and r['positions']==output['positions'] for r in rendered),
                                 timing_us={key:statistics.median(r['timing_us'][key] for r in rendered) for key in ('ingest','layout','render')},
                                 retained_working_bytes=statistics.median(r['retained_working_bytes'] for r in rendered),
                                 peak_working_bytes=statistics.median(r['peak_working_bytes'] for r in rendered))
                    if previous: report['prefix_stability'].append(dict(fixture=repo.name,limit=limit,**compare_prefix(previous,output)))
                    previous=output
                else: entry['errors']=[r.get('error') for r in samples]
                report['benchmarks'].append(entry)
    report['experiment_completed']=True
    report['topology_passed']=not report['shallow_boundary']['topology_errors'] and all(not p['topology_errors'] for p in report['previews']) and all(b.get('topology_error_count')==0 for b in report['benchmarks'])
    report['note']='Failed layouts are not valid performance evidence for a correct renderer. No visual approval or application integration is implied.'
    (args.output/'supplement.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__=='__main__': main()
