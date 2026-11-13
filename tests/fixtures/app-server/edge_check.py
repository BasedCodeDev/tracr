"""Resume failure-path checks; all state and child processes belong to this test."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
for scenario in ['changed', 'cancel', 'missing', 'direct', 'launch']:
    with tempfile.TemporaryDirectory(prefix='tracr-edge-') as temp:
        home = Path(temp)
        for name in ['resume', 'listed', 'changed']:
            (home / name).mkdir()
        executable = home / 'fake'
        shutil.copyfile(Path(__file__).with_name('fake.py'), executable)
        executable.chmod(0o755)
        settings = home / 'fake.json'
        settings.write_text('{}')
        (home / 'config.toml').write_text('')
        unrelated = subprocess.Popen(['sleep', '60'])
        terminal = Terminal([binary, '--config', str(home/'config.toml'), '--codex-home', temp, '--codex-bin', str(executable), str(home/'listed')])
        try:
            terminal.expect('DIRECTORY MISMATCH')
            before_handoff = len(terminal.output)
            if scenario == 'changed':
                settings.write_text(json.dumps({'read_cwd': str(home/'changed')}))
            elif scenario == 'cancel':
                settings.write_text(json.dumps({'read_wait': True}))
            elif scenario == 'missing':
                (home/'resume').rmdir()
            elif scenario == 'direct':
                settings.write_text(json.dumps({'direct': False}))
            elif scenario == 'launch':
                executable.unlink()
            terminal.send(b'\r\r')
            if scenario == 'changed':
                terminal.expect('Resume folder changed')
                assert not (home/'launch.json').exists()
                terminal.send(b'\r')
                terminal.wait(lambda: (home/'launch.json').exists())
                assert json.loads((home/'launch.json').read_text())['cwd'] == str(home/'changed')
                terminal.send(b'x')
                terminal.wait(lambda: b'toggle/resume' in terminal.output[before_handoff:])
            elif scenario == 'cancel':
                terminal.expect('Preparing resume')
                terminal.send(b'\x1b')
                terminal.expect('Resume cancelled')
                (home/'release').touch()
            else:
                terminal.expect({'missing':'Folder no longer exists', 'direct':'direct input', 'launch':'Could not resume Codex'}[scenario])
            if scenario != 'changed':
                assert not (home/'launch.json').exists()
            assert unrelated.poll() is None
            terminal.finish()
            print(scenario + ': passed')
        finally:
            if (home/'launch.json').exists():
                pid = json.loads((home/'launch.json').read_text())['pid']
                try:
                    os.kill(pid, 9)
                except ProcessLookupError:
                    pass
            terminal.close()
            unrelated.terminate()
            unrelated.wait()
