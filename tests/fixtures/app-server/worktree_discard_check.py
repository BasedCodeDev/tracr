#!/usr/bin/env python3
"""Exercise the discard dialog and recovery against a faultable or real Codex."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from uuid import UUID

from pty_support import Terminal


def fake():
    if 'proxy' in sys.argv:
        return
    store = Path(os.environ['CODEX_HOME']) / 'threads.json'
    for line in sys.stdin:
        request = json.loads(line)
        if 'id' not in request:
            continue
        state = json.loads(store.read_text())
        method = request['method']
        params = request.get('params', {})
        tid = params.get('threadId')
        chats = state['chats']
        result, error = {}, None
        if method == 'thread/list':
            rows = []
            for chat in chats.values():
                if chat['archived'] != params.get('archived', False):
                    continue
                if 'attachedToThreadId' in params:
                    if state.get('unsupported'):
                        rows.append(chat)
                    elif chat.get('attachedToThreadId') == params['attachedToThreadId']:
                        rows.append(chat)
                elif 'parentThreadId' in params:
                    if chat.get('parentThreadId') == params['parentThreadId']:
                        rows.append(chat)
                elif not chat.get('attachedToThreadId'):
                    rows.append(chat)
            limit = min(2, params.get('limit', 100))
            start = int(params.get('cursor') or 0)
            result = {'data': rows[start:start + limit],
                      'nextCursor': str(start + limit) if start + limit < len(rows) else None}
        elif method == 'thread/read':
            if tid in chats:
                result = {'thread': chats[tid]}
            else:
                error = f'thread not loaded: {tid}'
        elif method == 'thread/delete':
            if state.get('fail') == tid:
                error = 'injected delete failure'
            else:
                assert tid in chats
                assert not any(t.get('attachedToThreadId') == tid for t in chats.values())
                assert not any(t.get('parentThreadId') == tid for t in chats.values())
                assert Path(state['worktree']).is_dir(), 'Git cleanup ran before Codex deletion'
                state['deleted'].append(tid)
                del chats[tid]
                store.write_text(json.dumps(state))
        elif method != 'initialize':
            error = f'unsupported: {method}'
        reply = {'id': request['id']}
        if error:
            reply['error'] = {'code': -32600, 'message': error}
        else:
            reply['result'] = result
        print(json.dumps(reply), flush=True)


def setup(root, env):
    main, worktree = root / 'project', root / 'project' / '.worktrees' / 'feature'
    main.mkdir()

    def git(*args, cwd=main):
        return subprocess.check_output(['git', '-C', str(cwd), *args], env=env, text=True).strip()

    git('init', '-qb', 'main')
    git('config', 'user.name', 'Test')
    git('config', 'user.email', 'test@example.invalid')
    (main / '.gitignore').write_text('ignored.txt\n/.worktrees/\n')
    (main / 'tracked.txt').write_text('main\n')
    git('add', '.')
    git('commit', '-qm', 'Base')
    git('branch', 'unrelated')
    git('worktree', 'add', '-qb', 'feature', str(worktree))
    git('commit', '--allow-empty', '-qm', 'Unmerged feature', cwd=worktree)
    (worktree / 'tracked.txt').write_text('staged\n')
    git('add', 'tracked.txt', cwd=worktree)
    (worktree / 'tracked.txt').write_text('modified\n')
    (worktree / 'untracked.txt').write_text('untracked\n')
    (worktree / 'ignored.txt').write_text('ignored\n')
    return main, worktree, git


def confirm(t):
    t.expect('>[ Cancel ]')
    t.send(b'\tfeature')
    t.send(b'\t\t')
    t.expect('>[ Discard worktree ]')
    t.send(b'\r\r')


def verify_git(main, worktree, git):
    assert not worktree.exists()
    assert str(worktree) not in git('worktree', 'list', '--porcelain')
    assert git('branch', '--format=%(refname:short)').splitlines() == ['main', 'unrelated']
    assert (main / 'tracked.txt').read_text() == 'main\n'
    assert git('log', '-1', '--format=%s') == 'Base'


def check_fake(root, binary):
    env = {**os.environ, 'XDG_STATE_HOME': str(root / 'state'),
           'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    env.pop('TRACR_DEV_CONTROL', None)
    main, worktree, git = setup(root, env)
    home = root / 'codex'
    home.mkdir()
    store = home / 'threads.json'
    child = worktree / 'subfolder'
    child.mkdir()

    def chat(n, folder, title, *, archived=False, **metadata):
        tid = str(UUID(int=n))
        return tid, dict(id=tid, cwd=str(folder), name=title, preview=title,
                         createdAt=n, updatedAt=n, archived=archived,
                         status={'type': 'notLoaded'}, source='cli', **metadata)

    parent = chat(1, worktree, 'Feature conversation')
    side = chat(2, child, 'Saved side chat', attachedToThreadId=parent[0])
    spawned = chat(3, child, 'Archived child', archived=True, parentThreadId=parent[0])
    unrelated = chat(4, main, 'Unrelated conversation')
    extra = chat(5, main, 'Unrelated paginated conversation')
    state = dict(chats=dict([parent, side, spawned, unrelated, extra]), deleted=[],
                 worktree=str(worktree), fail=spawned[0])
    store.write_text(json.dumps(state))

    def launch(folder):
        return Terminal([binary, '--codex-home', str(home), '--codex-bin',
                         str(Path(__file__).resolve()), str(folder)], env, start_in_sessions=False)

    t = launch(worktree)
    try:
        t.expect('[◇ feature *]')
        t.expect_focus('Folders')
        t.expect_action('Discard worktree')
        for scenario, change, expected in [
            ('unsupported', {'unsupported': True}, 'did not honor the side chat filter'),
            ('outside', {'cwd': str(main)}, 'outside this worktree'),
            ('busy', {'status': {'type': 'idle'}}, 'Close loaded Codex'),
        ]:
            incident = json.loads(store.read_text())
            if scenario == 'unsupported':
                incident.update(change)
            elif scenario == 'outside':
                incident['chats'][spawned[0]].update(change)
            else:
                incident['chats'][side[0]].update(change)
            store.write_text(json.dumps(incident))
            t.send(b'd')
            t.expect(expected)
            t.send(b'\x1b')
            t.wait(lambda: expected not in t.text())
            store.write_text(json.dumps(state))

        nested = worktree / 'nested'
        nested.mkdir()
        git('init', '-qb', 'main', cwd=nested)
        t.send(b'd')
        t.expect('Nested Git repository or submodule')
        t.send(b'\x1b')
        t.wait(lambda: 'Nested Git repository or submodule' not in t.text())
        shutil.rmtree(nested)

        t.send(b'd')
        t.expect('3 Codex conversations remaining')
        t.expect('Staged and modified')
        t.expect('Untracked')
        t.expect('Ignored')
        t.send(b'\x1b[6~')
        t.expect('Saved side chat')
        t.expect('Archived child [archived]')
        for rows, cols in [(40, 120), (24, 80), (20, 60), (16, 40)]:
            t.resize(rows, cols)
            t.expect('Type the branch name')
            t.expect('>[ Cancel ]')
        t.resize(9, 30)
        t.expect('Enlarge terminal')
        t.send(b'\x1b')
        assert worktree.exists() and not json.loads(store.read_text())['deleted']
        t.resize(30, 100)

        t.send(b'd')
        t.expect('3 Codex conversations remaining')
        (worktree / 'ignored.txt').write_text('changed after review')
        confirm(t)
        t.expect('reviewed worktree or its conversations changed')
        assert worktree.exists() and not json.loads(store.read_text())['deleted']
        t.send(b'\x1b')
        t.wait(lambda: 'Discard stopped' not in t.text())

        t.send(b'd')
        t.expect('3 Codex conversations remaining')
        confirm(t)
        t.expect('injected delete failure')
        t.expect('1 conversation deletions verified')
        assert worktree.exists() and json.loads(store.read_text())['deleted'] == [side[0]]
        t.send(b'\x1b')
        t.wait(lambda: 'Discard stopped' not in t.text())
        t.finish()
    except Exception as error:
        raise AssertionError(t.text()) from error
    finally:
        t.close()

    state = json.loads(store.read_text())
    state.pop('fail')
    store.write_text(json.dumps(state))
    t = launch(worktree)
    try:
        t.expect('Unfinished discard')
        t.send(b'D')
        t.expect('2 Codex conversations remaining')
        confirm(t)
        t.expect('Worktree, local branch, and conversations discarded')
        t.expect_action('Discard worktree', False)
        t.finish()
    finally:
        t.close()
    verify_git(main, worktree, git)
    state = json.loads(store.read_text())
    assert state['deleted'] == [side[0], spawned[0], parent[0]]
    assert state['chats'] == dict([unrelated, extra])

    # Simulate a restart just after worktree removal, with the branch moved by
    # another checkout before the branch-deletion step.
    journal = root / 'state' / 'tracr' / 'worktree-discard' / 'current.json'
    record = json.loads(journal.read_text())
    record['phase'] = 'RemovingWorktree'
    journal.write_text(json.dumps(record))
    git('branch', 'feature', 'HEAD')
    t = launch(main)
    try:
        t.expect('Unfinished discard')
        t.send(b'D')
        t.expect('local branch changed after worktree removal')
        t.send(b'\x1b')
        t.wait(lambda: 'local branch changed after worktree removal' not in t.text())
        t.finish()
    finally:
        t.close()
    assert git('rev-parse', 'feature') == git('rev-parse', 'main')


def check_native(root, binary, codex):
    sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'scripts'))
    import probe_codex_relocation as probe

    probe.ALLOWED_METHODS |= {'thread/fork'}

    env = probe.environment(root)
    env.update(GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1')
    main, worktree, git = setup(root, env)
    server = probe.Server(codex, root, env)
    try:
        ids = []
        for folder in (worktree, worktree, main):
            thread = server.call('thread/start', {
                'cwd': str(folder), 'modelProvider': 'probe', 'model': 'probe',
                'historyMode': 'paginated' if not ids else 'legacy',
                'approvalPolicy': 'never', 'sandbox': 'read-only',
            })['thread']
            ids.append(thread['id'])
            server.call('thread/inject_items', {'threadId': thread['id'], 'items': [
                {'type': 'message', 'role': 'user', 'content': [
                    {'type': 'input_text', 'text': 'Disposable test conversation'},
                ]},
            ]})
    finally:
        server.close()
    server = probe.Server(codex, root, env)
    try:
        server.call('thread/archive', {'threadId': ids[1]})
        side = server.call('thread/fork', {
            'threadId': ids[0], 'attachToSource': True, 'excludeTurns': True,
        })['thread']
        assert side['attachedToThreadId'] == ids[0]
    finally:
        server.close()

    t = Terminal([binary, '--codex-home', env['CODEX_HOME'], '--codex-bin', codex, str(worktree)],
                 env, start_in_sessions=False)
    try:
        t.expect('[◇ feature *]')
        t.expect_action('Discard worktree')
        t.send(b'd')
        t.expect('3 Codex conversations remaining')
        confirm(t)
        t.expect('Worktree, local branch, and conversations discarded')
        t.finish()
    finally:
        t.close()
    verify_git(main, worktree, git)
    server = probe.Server(codex, root, env)
    try:
        for tid in [*ids[:2], side['id']]:
            try:
                server.call('thread/read', {'threadId': tid, 'includeTurns': False})
            except probe.RPCError as error:
                assert f'thread not loaded: {tid}' in str(error)
            else:
                raise AssertionError(f'Deleted thread still readable: {tid}')
        assert server.call('thread/read', {'threadId': ids[2]})['thread']['id'] == ids[2]
    finally:
        server.close()


if __name__ == '__main__':
    if sys.argv[1] == 'app-server':
        fake()
    else:
        scratch = Path('.tmp').resolve()
        scratch.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='tracr-discard-', dir=scratch) as directory:
            root = Path(directory)
            binary = str(Path(sys.argv[1]).resolve())
            if len(sys.argv) == 3:
                check_native(root, binary, str(Path(sys.argv[2]).resolve()))
            else:
                check_fake(root, binary)
        print('worktree discard: passed')
