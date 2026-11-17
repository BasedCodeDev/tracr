"""Worktree view fills a path and keeps the normal relink confirmation boundary."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from pty_support import Terminal

binary=str(Path(sys.argv[1]).resolve())
fake=str(Path(__file__).with_name('relink_fake.py').resolve())
with tempfile.TemporaryDirectory(prefix='tracr-worktrees-') as tmp:
    root=Path(tmp);old=root/'project';old.mkdir();new=root/'external worktree'
    def git(*args):
        subprocess.run(['git','-C',str(old),'-c','core.hooksPath=/dev/null','-c','user.name=Fixture','-c','user.email=fixture@example.invalid',*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
    git('init','-b','main');git('commit','--allow-empty','-m','fixture')
    git('worktree','add','-b','feature/worktree',str(new))
    home=root/'codex';home.mkdir()
    native=home/'native.json';native.write_text(json.dumps({'cwd':str(old),'archived':False,'profile':':workspace'}))
    env={**os.environ,'XDG_STATE_HOME':str(root/'state')}
    t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
    try:
        t.inspect('ID: session')
        t.send(b'm');t.expect('[Folders] Worktrees')
        t.wait(lambda:'Loading folders' not in t.text())
        t.send(b'\x1b[C');t.expect('Folders [Worktrees]')
        # The browser behind the modal can already show this branch. Wait for
        # the picker row itself before sending navigation keys.
        t.expect('external worktree · feature/worktree')
        t.wait(lambda:'Loading worktrees' not in t.text())
        t.send(b'\x1b[B');t.expect('> project (main) · main')
        t.send(b'\x1b[B');t.expect('> external worktree · feature/worktree')
        t.send(b'\r')
        t.send(b'\x1b[D');t.expect('[Folders] Worktrees')
        assert 'Enter confirms' not in t.text()
        assert json.loads(native.read_text())['cwd']==str(old)
        t.send(b'\x1b[C');t.expect('Folders [Worktrees]')
        t.expect('> external worktree · feature/worktree')
        t.wait(lambda:'Loading worktrees' not in t.text())
        t.resize(24,60);t.expect('[Worktrees]')
        t.send(b'\t\t\r');t.expect('Enter confirms')
        assert json.loads(native.read_text())['cwd']==str(old)
        t.send(b'\r');t.expect('Session relinked and verified.')
        assert json.loads(native.read_text())['cwd']==str(new)
        assert old.is_dir() and new.is_dir()
        t.finish()
    finally:t.close()
print('worktree tabs, path selection, resize and explicit relink confirmation: passed')
