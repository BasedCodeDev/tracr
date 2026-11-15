#!/usr/bin/env python3
"""Research only: real protocol-created turns against a loopback scripted provider.
Creates fresh disposable homes; never reads credentials or edits stored history.
Compares default/DB listing with and without a relocation-time turn.
"""
from diagnostics import run_cli, scratch_dir
import argparse, json, time, threading, http.server
from pathlib import Path
import probe_codex_relocation as p

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--keep-temp', action='store_true', help='Retain full diagnostic scratch instead of cleaning it')
    parser.add_argument('--codex',required=True)
    args=parser.parse_args()
    binary=str(Path(args.codex).resolve(strict=True))
    p.ALLOWED_METHODS = p.ALLOWED_METHODS | {'turn/start'}
    root=Path(scratch_dir(prefix='tracr-turn-relink-'))
    class Handler(http.server.BaseHTTPRequestHandler):
     def log_message(self,*args): pass
     def do_POST(self):
      self.rfile.read(int(self.headers.get('Content-Length','0')))
      item={'type':'message','id':'msg_test','role':'assistant','status':'completed','content':[{'type':'output_text','text':'Local fixture response.','annotations':[]}]}
      events=[('response.created',{'response':{'id':'resp_test','status':'in_progress','output':[]}}),('response.output_item.done',{'output_index':0,'item':item}),('response.completed',{'response':{'id':'resp_test','status':'completed','output':[item],'usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2}}})]
      self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
      for event,data in events:
       data['type']=event
       self.wfile.write(f'event: {event}\ndata: {json.dumps(data)}\n\n'.encode()); self.wfile.flush()
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    def turn(s,tid):
     t=s.call('turn/start',{'threadId':tid,'input':[{'type':'text','text':'Reply with the fixture response only.','text_elements':[]}]})['turn']
     deadline=time.monotonic()+30
     while time.monotonic()<deadline:
      line=s.q.get(timeout=30)
      v=json.loads(line);s.record('receive',v)
      if 'method' in v and 'id' in v: raise RuntimeError(v)
      if v.get('method')=='turn/completed':
       assert v['params']['turn']['status']=='completed',v
       return
     raise RuntimeError('turn timeout')
    results=[]
    try:
     for mode in ('legacy','paginated'):
      for strategy in ('resume','turn'):
       home=root/(mode+'-'+strategy);home.mkdir();env=p.environment(home)
       config=home/'codex/config.toml';config.write_text(config.read_text().replace('http://127.0.0.1:1/v1',f'http://127.0.0.1:{server.server_port}/v1'))
       old=home/'old';new=home/'new';old.mkdir();new.mkdir();s=None
       try:
        s=p.Server(binary,home,env)
        tid=s.call('thread/start',{'cwd':str(old),'historyMode':mode,'modelProvider':'probe','model':'probe','approvalPolicy':'never','sandbox':'read-only'})['thread']['id']
        turn(s,tid);s.close();old.rmdir()
        s=p.Server(binary,home,env)
        before=p.history(s,tid)
        s.call('thread/resume',{'threadId':tid,'cwd':str(new),'excludeTurns':True})
        if strategy=='turn':turn(s,tid)
        s.close();s=p.Server(binary,home,env)
        result={'mode':mode,'strategy':strategy,'id':tid,'read':p.read(s,tid)['cwd'],'list':next(t['cwd'] for t in p.inventory(s) if t['id']==tid),'db_list':next(t['cwd'] for t in s.call('thread/list',{'limit':100,'modelProviders':[],'sourceKinds':[],'useStateDbOnly':True})['data'] if t['id']==tid),'resume':s.call('thread/resume',{'threadId':tid,'excludeTurns':True})['thread']['cwd'],'expected':str(new),'history_before':before,'history_after':p.history(s,tid)}
        results.append(result);print(json.dumps({k:v for k,v in result.items() if not k.startswith('history')}),flush=True)
       finally:
        if s:s.close()
    finally:
     server.shutdown();server.server_close();(root/'results.json').write_text(json.dumps(results,indent=2));print('Scratch:',root,flush=True)

if __name__ == '__main__':
    run_cli(main)
