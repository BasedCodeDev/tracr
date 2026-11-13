"""Build, inspect and assemble Linux release assets. Never publishes or changes Git."""
import argparse
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    'x86_64-unknown-linux-musl': ('tracr-linux-x86_64', 'Advanced Micro Devices X86-64'),
    'aarch64-unknown-linux-musl': ('tracr-linux-aarch64', 'AArch64'),
}
COMMON = ('LICENSE', 'THIRD_PARTY_NOTICES.txt')


def run(command, **kwargs):
    return subprocess.check_output(command, text=True, **kwargs)


def version():
    return tomllib.loads((ROOT/'Cargo.toml').read_text())['package']['version']


def validate_tag(tag):
    if tag != 'v' + version():
        raise ValueError(f'tag {tag!r} must match Cargo version v{version()}')


def toolchain():
    pinned = tomllib.loads((ROOT/'rust-toolchain.toml').read_text())['toolchain']['channel']
    chosen = os.environ.get('TRACR_TOOLCHAIN', pinned)
    rustup = shutil.which('rustup') or str(Path(os.environ.get('CARGO_HOME', str(Path.home()/'.cargo')))/'bin/rustup')
    actual = run([rustup, 'run', chosen, 'rustc', '--version']).split()[1]
    if actual != pinned:
        raise ValueError(f'release requires Rust {pinned}; {chosen} provides {actual}')
    return [rustup, 'run', chosen, 'cargo']


def verify_binary(binary, target, expected_version):
    if target not in TARGETS:
        raise ValueError(f'unsupported target: {target}')
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f'missing or non-executable binary: {binary}')
    header = run(['readelf', '-h', str(binary)])
    if not re.search(r'Machine:\s*' + re.escape(TARGETS[target][1]) + r'\s*$', header, re.M):
        raise ValueError('binary architecture does not match target')
    program = run(['readelf', '-l', str(binary)])
    dynamic = run(['readelf', '-d', str(binary)])
    if re.search(r'\bINTERP\b', program) or '(NEEDED)' in dynamic:
        raise ValueError('release binary must be statically linked')
    actual = run([str(binary.resolve()), '--version']).strip()
    if actual != 'tracr ' + expected_version:
        raise ValueError(f'binary version mismatch: {actual}')


def notices(metadata):
    # Include the full resolved graph (including test dependencies) as a safe
    # superset; keep original copyright and license texts verbatim.
    sections = ['Dependency license notices (resolved Cargo graph; includes build/test dependencies).\n']
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        if package['name'] == 'tracr':
            continue
        folder = Path(package['manifest_path']).parent
        files = set()
        for entry in folder.iterdir():
            if entry.name.lower().startswith(('license', 'copying', 'notice', 'copyright')):
                files.update([entry] if entry.is_file() else (f for f in entry.rglob('*') if f.is_file()))
        if package.get('license_file'):
            files.add(folder/package['license_file'])
        if not files:
            raise ValueError(f'no license text found for {package["name"]}; review before release')
        sections.append(f'\n=== {package["name"]} {package["version"]} ({package.get("license") or "see license file"}) ===\n')
        for file in sorted(files):
            sections.extend([f'\n--- {file.relative_to(folder)} ---\n', file.read_text()])
    return '\n'.join(sections)


def runtime_notices(cargo):
    sysroot = Path(run(cargo[:-1] + ['rustc', '--print', 'sysroot']).strip())
    docs = sysroot/'share/doc/rust'
    copyright_file = docs/'COPYRIGHT-library.html'
    musl_copyright = Path('/usr/share/doc/musl/copyright')
    if not copyright_file.is_file() or not musl_copyright.is_file():
        raise ValueError('runtime license texts missing; install rust-docs and Ubuntu musl-tools/musl')
    class Text(HTMLParser):
        def __init__(self):
            super().__init__()
            self.parts = []
        def handle_data(self, data):
            self.parts.append(data)
    parser = Text()
    parser.feed(copyright_file.read_text())
    return ('\n=== Rust standard library notices ===\n' + ''.join(parser.parts)
            + '\n=== musl distribution copyright ===\n' + musl_copyright.read_text())


def checksums(folder, names):
    return ''.join(f'{hashlib.sha256((folder/name).read_bytes()).hexdigest()}  {name}\n' for name in sorted(names))


def verify_checksums(folder, names):
    expected = checksums(folder, names)
    if (folder/'SHA256SUMS').read_text() != expected:
        raise ValueError('checksum mismatch or unexpected checksum inventory')


def smoke(binary):
    binary = binary.resolve()
    result = json.loads(run([str(binary), 'list', '--fixture', str(ROOT/'tests/fixtures/sessions/catalog.json'), '--json']))
    if not result['complete'] or not result['sessions']:
        raise ValueError('release fixture inventory failed')
    run([str(binary), '--help'])
    for helper in ('terminal_check', 'folder_check', 'resume_check', 'edge_check', 'browse_check'):
        command = [sys.executable, str(ROOT/f'tests/fixtures/app-server/{helper}.py'), str(binary)]
        if helper == 'terminal_check':
            command.append(str(ROOT/'tests/fixtures/sessions/catalog.json'))
        subprocess.run(command, check=True, cwd=ROOT, timeout=300)


def build(args):
    output = args.output.resolve()
    if output.exists():
        raise ValueError(f'output already exists: {output}; use a fresh directory')
    cargo = toolchain()
    locked = ['--locked'] + (['--offline'] if args.offline else [])
    subprocess.run(cargo + ['build', '--release', '--target', args.target, '--target-dir', str(ROOT/'target'), *locked], cwd=ROOT, check=True)
    binary = ROOT/'target'/args.target/'release/tracr'
    verify_binary(binary, args.target, version())
    smoke(binary)
    metadata = json.loads(run(cargo + ['metadata', '--format-version', '1', '--filter-platform', args.target, *locked], cwd=ROOT))
    text = notices(metadata) + runtime_notices(cargo)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as temp:
        stage = Path(temp)/'assets'
        stage.mkdir()
        name = TARGETS[args.target][0]
        shutil.copy2(binary, stage/name)
        shutil.copy2(ROOT/'LICENSE', stage/'LICENSE')
        (stage/'THIRD_PARTY_NOTICES.txt').write_text(text)
        (stage/'SHA256SUMS').write_text(checksums(stage, (name, *COMMON)))
        stage.rename(output)
    print(f'Verified release assets: {output}')


def assemble(args):
    validate_tag(args.tag)
    output = args.output.resolve()
    if output.exists():
        raise ValueError('assembly output must not already exist')
    inputs = [args.input/TARGETS[target][0] for target in TARGETS]
    for folder, (name, _) in zip(inputs, TARGETS.values()):
        verify_checksums(folder, (name, *COMMON))
    if (inputs[0]/'LICENSE').read_bytes() != (inputs[1]/'LICENSE').read_bytes():
        raise ValueError('target license files differ')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as temp:
        stage = Path(temp)/'assets'; stage.mkdir()
        for folder, (name, _) in zip(inputs, TARGETS.values()):
            shutil.copy2(folder/name, stage/name)
        shutil.copy2(inputs[0]/'LICENSE', stage/'LICENSE')
        # Target-specific graphs can differ; retain both complete notice sets.
        (stage/'THIRD_PARTY_NOTICES.txt').write_text('\n'.join(
            f'=== {folder.name} ===\n' + (folder/'THIRD_PARTY_NOTICES.txt').read_text() for folder in inputs))
        (stage/'SHA256SUMS').write_text(checksums(stage, (*[v[0] for v in TARGETS.values()], *COMMON)))
        stage.rename(output)


def verify_download(args):
    name = TARGETS[args.target][0]
    names = [v[0] for v in TARGETS.values()] + list(COMMON)
    verify_checksums(args.input, names)
    # GitHub artifact transport does not preserve executable permissions.
    # Install into a disposable directory without touching the user's PATH.
    with tempfile.TemporaryDirectory(prefix='tracr-install-check-') as temp:
        installed = Path(temp)/'tracr'
        shutil.copyfile(args.input/name, installed)
        installed.chmod(0o755)
        verify_binary(installed, args.target, version())
        smoke(installed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    package = commands.add_parser('build')
    package.add_argument('--target', choices=TARGETS, required=True)
    package.add_argument('--offline', action='store_true')
    package.add_argument('--output', type=Path, required=True)
    package.set_defaults(action=build)
    merge = commands.add_parser('assemble')
    merge.add_argument('--tag', required=True)
    merge.add_argument('--input', type=Path, required=True)
    merge.add_argument('--output', type=Path, required=True)
    merge.set_defaults(action=assemble)
    verify = commands.add_parser('verify', help='verify combined downloaded assets on the native architecture')
    verify.add_argument('--target', choices=TARGETS, required=True)
    verify.add_argument('--input', type=Path, required=True)
    verify.set_defaults(action=verify_download)
    tag = commands.add_parser('check-tag')
    tag.add_argument('tag')
    tag.set_defaults(action=lambda args: validate_tag(args.tag))
    args = parser.parse_args()
    try:
        args.action(args)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'release: {error}\n')


if __name__ == '__main__':
    main()
