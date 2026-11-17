"""New-session handoff and remembered-folder acceptance, isolated from real homes."""
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-create-') as directory:
    root = Path(directory)
    project = root/'empty project $'
    project.mkdir()
    state = root/'state'
    config = root/'config.toml'
    config.write_text('')
    fake = root/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    homes = [root/'one', root/'two']
    for home in homes:
        home.mkdir()
        (home/'fake.json').write_text(json.dumps({'report_size': True, 'read_cwd': str(project)}))
    args = [binary, '--config', str(config), '--codex-bin', str(fake)]
    for home in homes:
        args += ['--codex-home', str(home)]
    env = {'XDG_STATE_HOME': str(state)}
    t = Terminal(args+[str(project)], env)
    try:
        t.expect('Create new session')
        assert 'All sessions' not in t.text()
        t.send(b'\x1b'); t.expect('Enter Sessions')
        t.send(b'\r')
        t.expect('Choose harness')
        t.send(b'\x1b')
        t.wait(lambda: 'Choose harness' not in t.text())
        assert not any((h/'launch.json').exists() for h in homes)
        t.send(b'\x1b'); t.expect('Enter Sessions')
        t.send(b'\r\x1b[B\r')
        t.wait(lambda: (homes[1]/'launch.json').exists())
        launch = json.loads((homes[1]/'launch.json').read_text())
        assert launch['args'] == ['--cd', str(project)], launch
        assert launch['cwd'] == str(project) and launch['home'] == str(homes[1]) and launch['foreground'], launch
        assert not (homes[0]/'launch.json').exists()
        t.wait(lambda: 'Folders' in t.text())
        t.wait(lambda: (homes[1]/'terminal-size.json').exists() and json.loads((homes[1]/'terminal-size.json').read_text())['columns'] == 206)
        saved = state/'tracr/browser-state.json'
        assert json.loads(saved.read_text())['folder'] == str(project)
        t.send(b'x')
        t.wait(lambda: 'exited: Success' in t.text() and 'Folders' in t.text())
        t.expect('New session')
        t.inspect('ID: created')
        assert '> New session' in t.text()
        t.send(b'n');t.expect('Choose harness')
        t.send(b'\x1b');t.wait(lambda:'Choose harness' not in t.text())
        t.finish()
    finally:
        t.close()
    # A single configured harness creates directly even in a nonempty folder.
    (homes[1]/'launch.json').unlink()
    single=[binary,'--config',str(config),'--codex-bin',str(fake),'--codex-home',str(homes[1]),str(project)]
    t=Terminal(single,env)
    try:
        t.inspect('ID: created')
        t.send(b'\x1b');t.expect('Enter Sessions');t.send(b'n');t.send(b'?');t.expect('Actions & shortcuts')
        assert not (homes[1]/'launch.json').exists()
        t.send(b'\x1b');t.wait(lambda:'Actions & shortcuts' not in t.text())
        t.send(b'\tn')
        t.wait(lambda:(homes[1]/'launch.json').exists())
        launch=json.loads((homes[1]/'launch.json').read_text())
        assert launch['args']==['--cd',str(project)] and launch['cwd']==str(project)
        assert 'Choose harness' not in t.text()
        t.send(b'x');t.wait(lambda:'exited: Success' in t.text() and 'Folders' in t.text())
        # Folder Return resumes the sole session directly, embedded.
        (homes[1]/'launch.json').unlink()
        t.send(b'\x1b');t.expect('Enter Sessions');t.send(b'\r')
        t.wait(lambda:(homes[1]/'launch.json').exists())
        resumed=json.loads((homes[1]/'launch.json').read_text())
        assert 'resume' in resumed['args'], resumed
        t.wait(lambda:'Folders' in t.text())
        t.wait(lambda:json.loads((homes[1]/'terminal-size.json').read_text())['columns']==206)
        t.send(b'x');t.wait(lambda:'exited: Success' in t.text() and 'Folders' in t.text())

        t.finish()
    finally:t.close()
    # Plain invocation restores the folder; explicit path overrides it.
    for extra, expected in [([], 'ID: created'), ([str(root/'missing')], 'Create new session')]:
        t = Terminal(args+extra, env)
        try:
            t.inspect(expected) if expected.startswith('ID:') else t.expect(expected)
            if extra:
                t.send(b'\r\r')
                t.expect('Cannot create a session')
            t.finish()
        finally:
            t.close()
    assert json.loads(saved.read_text())['folder'] == str(root/'missing')
    # First launch uses the caller cwd, and a corrupt file falls back nonfatally.
    for content in [None, 'invalid']:
        if content is None:
            saved.unlink()
        else:
            saved.write_text(content)
        t = Terminal(args, env)
        try:
            t.expect('Create new session')
            if content: t.expect('Invalid browser state')
            t.finish()
        finally:
            t.close()
        assert json.loads(saved.read_text())['folder'] == os.getcwd()
    print('creation, source picker, refresh selection and startup state: passed')
