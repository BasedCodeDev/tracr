#!/usr/bin/env python3
"""Exercise the discard dialog and recovery against a faultable or real Codex."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
from uuid import UUID

from pty_support import Terminal


def fake_proxy(store):
    if not json.loads(store.read_text()).get('shared'):
        return
    stream, output = sys.stdin.buffer, sys.stdout.buffer
    stream.readline()
    headers = {}
    while line := stream.readline().strip():
        name, value = line.split(b':', 1)
        headers[name.lower()] = value.strip()
    accept = base64.b64encode(hashlib.sha1(headers[b'sec-websocket-key']
                              + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
    output.write(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                 b'Sec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
    output.flush()
    while header := stream.read(2):
        opcode, size = header[0] & 15, header[1] & 127
        if size in (126, 127):
            size = int.from_bytes(stream.read(2 if size == 126 else 8), 'big')
        mask = stream.read(4) if header[1] & 128 else None
        payload = stream.read(size)
        if opcode == 8:
            return
        if opcode != 1:
            continue
        if mask:
            payload = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        request = json.loads(payload)
        if 'id' not in request:
            continue
        state = json.loads(store.read_text())
        shared = state.get('shared', {})
        method = request['method']
        params = request.get('params', {})
        tid = params.get('threadId')
        result = {}
        if method == 'initialize':
            result = {'codexHome': str(store.parent)}
        elif method == 'thread/loaded/list':
            result = {'data': list(shared.get('loaded', {})), 'nextCursor': None}
        elif method == 'thread/read':
            result = {'thread': dict(state['chats'][tid], status=shared['loaded'][tid])}
        elif method == 'thread/backgroundTerminals/list':
            result = {'data': shared.get('terminals', []), 'nextCursor': None}
        elif method in ('thread/resume', 'thread/unsubscribe'):
            assert tid in shared['loaded'], request
            assert shared['loaded'][tid] == {'type': 'idle'}, request
            with (store.parent / 'release.log').open('a') as log:
                print(method, file=log)
            if method == 'thread/resume':
                assert params['config'] == {'thread_unload_delay_secs': 0}, request
                cwd = state['chats'][tid]['cwd']
                result = {'thread': state['chats'][tid], 'cwd': cwd, 'runtimeWorkspaceRoots': [cwd],
                          'sandbox': {'type': 'readOnly', 'networkAccess': False},
                          'model': 'probe', 'modelProvider': 'probe', 'approvalPolicy': 'never',
                          'activePermissionProfile': None}
            elif not shared.get('attached'):
                del shared['loaded'][tid]
                replacement = store.with_suffix('.replacement')
                replacement.write_text(json.dumps(state))
                replacement.replace(store)
        else:
            raise AssertionError(request)
        response = json.dumps({'id': request['id'], 'result': result}).encode()
        prefix = bytes([129, len(response)]) if len(response) < 126 else bytes([129, 126]) + struct.pack('!H', len(response))
        output.write(prefix + response)
        output.flush()


def fake():
    store = Path(os.environ['CODEX_HOME']) / 'threads.json'
    if 'proxy' in sys.argv:
        fake_proxy(store)
        return
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
    (main / '.gitignore').write_text('ignored.txt\n/.tmp/\n/.worktrees/\n')
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
            ('busy', {'status': {'type': 'idle'}}, 'Saved side chat · Idle'),
        ]:
            incident = json.loads(store.read_text())
            if scenario == 'unsupported':
                incident.update(change)
            elif scenario == 'outside':
                incident['chats'][spawned[0]].update(change)
            else:
                incident['chats'][side[0]].update(change)
            store.write_text(json.dumps(incident))
            t.send(b'\x1b[3~')
            t.expect(expected)
            t.send(b'\x1b')
            t.wait(lambda: expected not in t.text())
            store.write_text(json.dumps(state))

        sessions = home / 'sessions'
        sessions.mkdir()
        metadata = sessions / 'release-fixture.jsonl'
        metadata.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': parent[0]}}) + '\n'
                            + json.dumps({'type': 'turn_context', 'payload': {'active_permission_profile': None}}) + '\n')
        state['chats'][parent[0]]['path'] = str(metadata)
        idle = {'type': 'idle'}
        running = {'type': 'active', 'activeFlags': []}
        waiting = {'type': 'active', 'activeFlags': ['waitingOnUserInput']}
        release_log = home / 'release.log'

        def shared(loaded, **flags):
            store.write_text(json.dumps({**state, 'shared': {'loaded': loaded, **flags}}))

        def retry():
            t.send(b'\t')
            t.expect('>[ Retry ]')
            t.send(b'\r')
            t.expect('>[ Cancel ]')

        def release():
            t.send(b'\t\t')
            t.expect('>[ Release idle conversations ]')
            t.send(b'\r')

        shared({parent[0]: running, side[0]: waiting, spawned[0]: idle})
        t.send(b'\x1b[3~')
        t.expect('Feature conversation · Running')
        t.expect('Saved side chat · Waiting for input')
        t.send(b'\x1b[6~')
        t.expect('Archived child · Idle · Archived')
        assert not release_log.exists()
        t.send(b'\x1b[Z')
        t.expect('>[ Retry ]')  # Release is disabled for all three blockers.
        t.send(b'\t')
        t.expect('>[ Cancel ]')
        shared({})
        retry()
        t.expect('Type the branch name')
        t.send(b'\x1b')
        t.wait(lambda: 'Type the branch name' not in t.text())
        shared({parent[0]: idle}, terminals=[{'processId': 'p1', 'itemId': 'i1', 'command': 'probe', 'cwd': str(worktree)}])
        t.send(b'\x1b[3~')
        t.expect('Feature conversation · Idle')
        assert not release_log.exists()
        release()
        t.expect('Background terminals are still running')
        assert not release_log.exists()

        shared({parent[0]: idle}, attached=True)
        retry()
        t.expect('Feature conversation · Idle')
        release()
        t.expect('Still loaded. Close any Codex client')
        assert release_log.read_text().splitlines() == ['thread/resume', 'thread/unsubscribe']
        shared({parent[0]: idle})
        retry()
        t.expect('Feature conversation · Idle')
        release()
        t.expect('Type the branch name')
        assert release_log.read_text().splitlines() == ['thread/resume', 'thread/unsubscribe'] * 2
        assert worktree.exists() and not json.loads(store.read_text())['deleted']

        # Reattachment after review invalidates confirmation before any deletion.
        shared({parent[0]: running})
        confirm(t)
        t.expect('Feature conversation · Running')
        assert worktree.exists() and not json.loads(store.read_text())['deleted']
        t.send(b'\x1b')
        t.wait(lambda: 'Discard stopped' not in t.text())
        store.write_text(json.dumps(state))

        bare = worktree / '.tmp/real3/paginated/codex/.tmp/git-fixture'
        bare.mkdir(parents=True)
        git('init', '--bare', '-qb', 'main', cwd=bare)
        (bare / 'config').unlink()
        # Match the config-free bare repository that triggered the original block.
        assert git('rev-parse', '--is-bare-repository', cwd=bare) == 'true'
        nested = worktree / 'nested'
        nested.mkdir()
        git('init', '-qb', 'main', cwd=nested)
        git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
            'commit', '--allow-empty', '-qm', 'Nested history', cwd=nested)

        def expect_blocked(message):
            t.send(b'\x1b[3~')
            t.expect(message)
            t.send(b'\x1b')
            t.wait(lambda: message not in t.text())
            assert not json.loads(store.read_text())['deleted']

        linked = root / 'nested-linked'
        git('worktree', 'add', '-qb', 'linked', str(linked), cwd=nested)
        expect_blocked('Repository has linked worktrees')
        assert linked.is_dir()
        git('worktree', 'remove', str(linked), cwd=nested)

        external = root / 'external'
        external.mkdir()
        git('init', '--bare', '-qb', 'main', cwd=external)
        alternates = nested / '.git/objects/info/alternates'
        alternates.write_text(str(external / 'objects') + '\n')
        expect_blocked('Shared or alternate Git storage')
        alternates.unlink()

        storage_link = nested / '.git/objects/info/storage-link'
        storage_link.symlink_to(external / 'objects')
        expect_blocked('points outside the repository')
        storage_link.unlink()

        hook = nested / 'local-hook'
        hook.write_text('# Local test hook\n')
        (nested / '.git/hooks/local-hook').symlink_to('../../local-hook')

        linked_inside = worktree / 'linked-inside'
        git('worktree', 'add', '-qb', 'linked-inside', str(linked_inside), cwd=nested)
        expect_blocked('Linked checkout or external Git metadata')
        git('worktree', 'remove', str(linked_inside), cwd=nested)

        git('update-index', '--add', '--cacheinfo', f'160000,{git("rev-parse", "HEAD", cwd=nested)},submodule',
            cwd=nested)
        expect_blocked('Repository contains submodules')
        git('update-index', '--force-remove', 'submodule', cwd=nested)

        t.resize(40, 120)
        t.send(b'\x1b[3~')
        t.expect('Nested repositories and their Git history will be permanently deleted')
        t.expect('Bare repository .tmp/real3/paginated/codex/.tmp/git-fixture')
        t.expect('Repository nested')
        t.expect('Staged and modified')
        t.expect('Untracked')
        t.send(b'\x1b[6~')
        t.expect('Ignored')
        t.expect('3 Codex conversations remaining')
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
        assert (bare / 'HEAD').is_file() and (nested / '.git').is_dir()
        t.resize(40, 120)

        t.send(b'\x1b[3~')
        t.expect('Type the branch name')
        t.send(b'\x1b[6~\x1b[6~')
        t.expect('3 Codex conversations remaining')
        git('symbolic-ref', 'HEAD', 'refs/heads/changed-after-review', cwd=bare)
        confirm(t)
        t.expect('reviewed worktree or its conversations changed')
        assert worktree.exists() and not json.loads(store.read_text())['deleted']
        t.send(b'\x1b')
        t.wait(lambda: 'Discard stopped' not in t.text())

        t.send(b'\x1b[3~')
        t.expect('Type the branch name')
        t.send(b'\x1b[6~\x1b[6~')
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
        t.expect('Type the branch name')
        t.send(b'\x1b[6~\x1b[6~')
        t.expect('2 Codex conversations remaining')
        confirm(t)
        t.expect('Worktree, local branch, and conversations discarded')
        t.expect_action('Discard worktree', False)
        t.finish()
    finally:
        t.close()
    verify_git(main, worktree, git)
    assert (external / 'objects').is_dir()
    state = json.loads(store.read_text())
    assert state['deleted'] == [side[0], spawned[0], parent[0]]
    assert state['chats'] == dict([unrelated, extra])

    # Simulate a restart just after worktree removal, with the branch moved by
    # another checkout before the branch-deletion step.
    journal = root / 'state' / 'tracr' / 'worktree-discard' / 'current.json'
    record = json.loads(journal.read_text())
    record['phase'] = 'RemovingWorktree'
    record['approved']['checkout'].pop('nested_repositories')
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
        t.send(b'\x1b[3~')
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
