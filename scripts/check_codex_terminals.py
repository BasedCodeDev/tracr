#!/usr/bin/env python3
"""Disposable native terminal list/terminate acceptance; no real account or inference."""
import argparse, http.server, json, os, socket, subprocess, sys, tempfile, threading, time
from check_codex_release import connect
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests/fixtures/app-server'))
from real_status_check import Owner, probe

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--codex',required=True);args=parser.parse_args()
    binary=str(Path(args.codex).resolve());root=Path(tempfile.mkdtemp(prefix='tracr-terminal-probe-'))
    env=probe.environment(root);env['PATH']='/usr/bin:/bin'
    project=root/'project';project.mkdir();requests=[]
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):self.send_response(200);self.end_headers();self.wfile.write(b'{"data":[]}')
        def do_POST(self):
            request=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            requests.append(request)
            if len(requests)==1:
                item={'type':'function_call','id':'fc_background','call_id':'call_background','name':'exec_command','arguments':json.dumps({'cmd':'sleep 300','yield_time_ms':1000,'max_output_tokens':100})}
            else:
                item={'type':'message','id':'msg_done','role':'assistant','status':'completed','content':[{'type':'output_text','text':'Disposable background process started.','annotations':[]}]}
            events=[('response.created',{'response':{'id':f'resp_{len(requests)}','status':'in_progress','output':[]}}),('response.output_item.done',{'output_index':0,'item':item}),('response.completed',{'response':{'id':f'resp_{len(requests)}','status':'completed','output':[item],'usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2}}})]
            self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
            for name,data in events:
                data['type']=name;self.wfile.write(f'event: {name}\ndata: {json.dumps(data)}\n\n'.encode());self.wfile.flush()
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
    config=root/'codex/config.toml';config.write_text(config.read_text().replace('http://127.0.0.1:1/v1',f'http://127.0.0.1:{server.server_port}/v1'))
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1]
    log=(root/'daemon.log').open('w');daemon=subprocess.Popen([binary,'app-server','--listen',f'ws://127.0.0.1:{port}'],env=env,cwd=root,stdin=subprocess.PIPE,stdout=log,stderr=log)
    client=None;tid=None;result={'passed':False,'version':subprocess.check_output([binary,'--version'],text=True).strip()}
    try:
        deadline=time.monotonic()+15
        while True:
            try:client=connect(binary,env,port);break
            except (OSError,RuntimeError):
                if time.monotonic()>deadline:raise RuntimeError('daemon did not start')
                time.sleep(.1)
        thread=client.call('thread/start',{'cwd':str(project),'modelProvider':'probe','model':'probe','approvalPolicy':'never','sandbox':'danger-full-access'})['thread'];tid=thread['id']
        client.call('turn/start',{'threadId':tid,'input':[{'type':'text','text':'Run the disposable background terminal test.','text_elements':[]}]})
        deadline=time.monotonic()+25
        while True:
            state=client.call('thread/read',{'threadId':tid,'includeTurns':False})['thread']['status']
            terminals=client.call('thread/backgroundTerminals/list',{'threadId':tid,'limit':100})
            if terminals['data'] and state['type']=='idle':break
            if time.monotonic()>deadline:raise RuntimeError(f'background terminal not idle/available: {state}, {terminals}')
            time.sleep(.1)
        result['before']=terminals
        before=client.call('thread/read',{'threadId':tid,'includeTurns':False})['thread']
        fields=['id','createdAt','updatedAt','model','modelProvider','gitInfo','historyMode','projectId']
        result['metadata_before']={k:before.get(k) for k in fields}
        assert len(terminals['data'])==1 and terminals.get('nextCursor') is None
        terminal=terminals['data'][0];assert terminal['cwd']==str(project) and 'sleep 300' in terminal['command']
        result['terminate']=client.call('thread/backgroundTerminals/terminate',{'threadId':tid,'processId':terminal['processId']})
        result['after']=client.call('thread/backgroundTerminals/list',{'threadId':tid,'limit':100})
        assert result['terminate']['terminated'] is True and result['after']['data']==[]
        after=client.call('thread/read',{'threadId':tid,'includeTurns':False})['thread']
        result['cwd_after']=after['cwd'];assert result['cwd_after']==str(project)
        result['metadata_after']={k:after.get(k) for k in fields}
        assert result['metadata_before']==result['metadata_after'], 'Cleanup changed relocation fingerprint'
        result['passed']=True
    finally:
        if client:
            if tid:
                try:
                    for terminal in client.call('thread/backgroundTerminals/list',{'threadId':tid,'limit':100})['data']:
                        client.call('thread/backgroundTerminals/terminate',{'threadId':tid,'processId':terminal['processId']})
                except Exception:pass
            client.close()
        daemon.terminate()
        try:daemon.wait(timeout=10)
        except subprocess.TimeoutExpired:daemon.kill();daemon.wait()
        server.shutdown();server.server_close();log.close()
        (root/'results.json').write_text(json.dumps(result,indent=2)+'\n');print('Evidence:',root,flush=True)
    print(json.dumps(result),flush=True)
if __name__=='__main__':main()
