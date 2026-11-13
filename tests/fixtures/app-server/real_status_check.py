"""Opt-in live-status acceptance: real daemon, separate owner, local scripted model.
No credentials or model inference. Run with tracr and Codex executable paths.
"""
import base64
import http.server
import importlib.util
import json
import os
from pathlib import Path
import queue
import struct
import socket
from types import SimpleNamespace
import subprocess
import sys
import tempfile
import threading
import time
from pty_support import Terminal

ROOT=Path(__file__).resolve().parents[3]
spec=importlib.util.spec_from_file_location('probe', ROOT/'scripts/probe_codex_relocation.py')
probe=importlib.util.module_from_spec(spec);spec.loader.exec_module(probe)

class Owner:
    def __init__(self, codex, env):
        self.sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
        self.sock.settimeout(8)
        self.sock.connect(str(Path(env['CODEX_HOME'])/'app-server-control/app-server-control.sock'))
        self.p=SimpleNamespace(stdin=self.sock.makefile('wb'),stdout=self.sock.makefile('rb'))
        self.q=queue.Queue();self.seq=0
        key=base64.b64encode(os.urandom(16)).decode()
        self.p.stdin.write(f'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode());self.p.stdin.flush()
        def read():
            try:
                header=b''
                while not header.endswith(b'\r\n\r\n'):
                    data=self.p.stdout.read(1)
                    if not data: raise RuntimeError('proxy handshake closed')
                    header+=data
                assert b'101 Switching Protocols' in header, header
                while True:
                    prefix=self.p.stdout.read(2)
                    if not prefix: break
                    length=prefix[1]&127
                    if length==126:length=struct.unpack('!H',self.p.stdout.read(2))[0]
                    if length==127:length=struct.unpack('!Q',self.p.stdout.read(8))[0]
                    data=self.p.stdout.read(length)
                    if prefix[0]&15==1:self.q.put(json.loads(data))
            except Exception as error:self.q.put(error)
            self.q.put(None)
        self.reader=threading.Thread(target=read,daemon=True);self.reader.start()
        try:
            self.call('initialize',{'clientInfo':{'name':'tracr_status_owner','version':'0.2'},'capabilities':{'experimentalApi':True}})
            self.send({'method':'initialized'})
        except BaseException:
            self.close();raise
    def send(self, message):
        data=json.dumps(message).encode();mask=os.urandom(4)
        prefix=bytes([129,128|len(data)]) if len(data)<126 else bytes([129,254])+struct.pack('!H',len(data))
        self.p.stdin.write(prefix+mask+bytes(v^mask[i%4] for i,v in enumerate(data)));self.p.stdin.flush()
    def call(self, method, params):
        self.seq+=1;id=self.seq
        self.send({'id':id,'method':method,'params':params})
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            result=self.q.get(timeout=max(.01,deadline-time.monotonic()))
            if result is None or isinstance(result,Exception):raise RuntimeError(str(result))
            if result.get('id')==id and 'method' not in result:
                if 'error' in result:raise RuntimeError(str(result['error']))
                return result['result']
        raise RuntimeError('owner RPC timeout')
    def close(self):
        try:self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:pass
        self.p.stdin.close();self.p.stdout.close();self.sock.close()


def main():
    binary,codex=(str(Path(p).resolve()) for p in sys.argv[1:])
    home=Path(tempfile.mkdtemp(prefix='tracr-real-status-'))
    env=probe.environment(home);env['PATH']='/usr/bin:/bin'
    release=threading.Event()
    received=threading.Event()
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):
            self.send_response(200);self.end_headers();self.wfile.write(b'{"data":[]}')
        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length','0')))
            received.set()
            release.wait(30)
            item={'type':'function_call','id':'fc_wait','call_id':'call_wait','name':'request_user_input','arguments':json.dumps({'questions':[{'id':'choice','header':'Test','question':'Synthetic status test?','options':[{'label':'One','description':'First choice'},{'label':'Two','description':'Second choice'}]}]})}
            events=[('response.created',{'response':{'id':'resp_status','status':'in_progress','output':[]}}),('response.output_item.done',{'output_index':0,'item':item}),('response.completed',{'response':{'id':'resp_status','status':'completed','output':[item],'usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2}}})]
            self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
            for event,data in events:
                data['type']=event
                try:self.wfile.write(f'event: {event}\ndata: {json.dumps(data)}\n\n'.encode());self.wfile.flush()
                except BrokenPipeError:break
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    config=home/'codex/config.toml'
    config.write_text(config.read_text().replace('http://127.0.0.1:1/v1',f'http://127.0.0.1:{server.server_port}/v1'))
    project=home/'project';project.mkdir()
    daemon_log=(home/'daemon.log').open('w')
    daemon=subprocess.Popen([codex,'app-server','--listen','unix://'],env=env,cwd=home,stdout=daemon_log,stderr=daemon_log)
    owner=None;terminal=None;results=[]
    try:
        # The owned daemon binds its home-specific rendezvous before a proxy can connect.
        deadline=time.monotonic()+10
        while not (home/'codex/app-server-control/app-server-control.sock').exists():
            assert time.monotonic()<deadline, 'daemon did not bind socket'
            time.sleep(.1)
        for _ in range(1):
            try:owner=Owner(codex,env);break
            except (RuntimeError,queue.Empty):
                if daemon.poll() is not None:raise RuntimeError('daemon exited: '+(home/'daemon.log').read_text()[-2000:])
                time.sleep(.1)
        assert owner is not None
        thread=owner.call('thread/start',{'cwd':str(project),'modelProvider':'probe','model':'probe','approvalPolicy':'never','sandbox':'read-only'})['thread']
        turn=owner.call('turn/start',{'threadId':thread['id'],'input':[{'type':'text','text':'Synthetic local status test.','text_elements':[]}], 'collaborationMode':{'mode':'plan','settings':{'model':'probe','reasoning_effort':None,'developer_instructions':None}}})['turn']
        assert received.wait(15),'scripted model did not receive request'
        terminal=Terminal([binary,'--codex-home',str(home/'codex'),'--codex-bin',codex,str(project)],env)
        terminal.expect('Running');results.append('running')
        release.set()
        terminal.expect('Waiting for input');results.append('waiting for input')
        owner.call('turn/interrupt',{'threadId':thread['id'],'turnId':turn['id']})
        terminal.wait(lambda:'Waiting for input' not in terminal.text() and 'Running' not in terminal.text());results.append('idle')
        terminal.finish();assert daemon.poll() is None;
        assert owner.call('thread/read', {'threadId':thread['id'],'includeTurns':False})['thread']['status']['type'] == 'idle';results.append('shared daemon survives browser exit')
        result={'status':'passed','checks':results,'evidence':str(home)}
        (home/'results.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result))
    finally:
        release.set()
        if terminal:
            (home/'terminal.bin').write_bytes(terminal.output);terminal.close()
        if owner:owner.close()
        daemon.terminate()
        try:daemon.wait(timeout=10)
        except subprocess.TimeoutExpired:daemon.kill();daemon.wait()
        server.shutdown();server.server_close();daemon_log.close()
        print('Real status evidence:',home)

if __name__=='__main__':main()
