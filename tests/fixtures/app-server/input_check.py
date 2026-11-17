#!/usr/bin/env python3
"""Optional real-Codex input/scroll smoke test; synthetic fixture, no submitted prompt.
Run from the repository root: python tests/fixtures/app-server/input_check.py /path/to/codex
"""
import json, sys, tempfile, importlib.util, time
from pathlib import Path
from unittest.mock import patch
root = Path.cwd()
sys.path.insert(0, str(root/'scripts'))
sys.path.insert(0, str(root/'tests/fixtures/app-server'))
import probe_codex_relocation as probe
from pty_support import Terminal
spec = importlib.util.spec_from_file_location('real_check', root/'tests/fixtures/app-server/real_check.py')
real = importlib.util.module_from_spec(spec); spec.loader.exec_module(real)
codex = str(Path(sys.argv[1]).resolve())
original = Path.read_text
fixture_path = root/'tests/fixtures/codex-relocation/messages.json'
def contents(path, *args, **kwargs):
    text = original(path, *args, **kwargs)
    if path == fixture_path:
        items = json.loads(text)
        items[1]['content'][0]['text'] = ' '.join(f'HISTORY-{i:04}: '+ 'synthetic transcript ' * 5 for i in range(250))
        return json.dumps(items)
    return text
with tempfile.TemporaryDirectory(prefix='tracr-real-input-') as temp:
    home = Path(temp)/'fixture'
    terminal = None
    try:
        with patch.object(Path, 'read_text', contents):
            fixture = probe.prepare_fixture(codex, home, 'legacy')
        with (home/'codex/config.toml').open('a') as config:
            config.write('\n[projects.'+json.dumps(str(home/'old'))+']\ntrust_level = "trusted"\n')
        env = fixture['env']
        wrapper = home/'codex-no-daemon'
        wrapper.write_text('#!/usr/bin/python3\nimport os,sys\nbinary='+repr(codex)+'\nargs=sys.argv[1:]\nif args and args[0] == "resume": args=["--no-daemon"]+args\nos.execv(binary, [binary]+args)\n')
        wrapper.chmod(0o755)
        terminal = Terminal([str(root/'target/debug/tracr'), '--codex-home', str(home/'codex'), '--codex-bin', str(wrapper), str(home/'old')], env)
        terminal.wait(lambda: b'\x1b[?u' in terminal.output)
        terminal.send(b'\x1b[?0u\x1b[?1;2c')
        terminal.wait(lambda: b'\x1b[>31u' in terminal.output)
        terminal.inspect('ID: '+fixture['baseline']['id'])
        terminal.send(b'\x1bf')
        terminal.expect('Ctrl+B background')
        terminal.expect('HISTORY-0249')
        terminal.send(b'\x1b[200~TRACR_INPUT alpha beta\x1b[201~')
        terminal.expect('TRACR_INPUT alpha beta')
        terminal.send(b'\x1b[127;5u')
        terminal.wait(lambda: 'TRACR_INPUT alpha beta' not in terminal.text())
        terminal.send(b'gamma')
        terminal.expect('TRACR_INPUT alpha gamma')
        terminal.send(b'\x15')
        terminal.wait(lambda: 'TRACR_INPUT' not in terminal.text())
        terminal.send(b'\x1b[<64;200;10M'*40)
        terminal.expect('HISTORY-0100')
        assert 'HISTORY-0249' not in terminal.text()
        for _ in range(30):
            terminal.send(b'\x1b[<65;200;10M'*8)
            until=time.monotonic()+0.1
            terminal.wait(lambda: time.monotonic()>until)
            if 'HISTORY-0249' in terminal.text(): break
        terminal.expect('HISTORY-0249')
        terminal.send(b'\x02')
        terminal.send(b'q')
        terminal.expect('Close all owned terminals and quit?')
        terminal.send(b'y')
        terminal.wait(lambda: terminal.child.poll() is not None)
        assert b'\x1b[<u' in terminal.output
        print('Real Codex: enhanced negotiation, Ctrl+Backspace deletes a word, Ctrl+U, wheel up/down and protocol cleanup passed; no prompt submitted')
    except Exception:
        if terminal: print(terminal.text()[:5500], flush=True)
        raise
    finally:
        if terminal: terminal.close()
        if home.exists(): real.stop_daemons(codex, { 'CODEX_HOME':str(home/'codex'), 'HOME':str(home/'home'), 'PATH':'/usr/bin:/bin'}, home)
