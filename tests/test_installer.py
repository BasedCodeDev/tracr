"""Exercise the real POSIX installer with isolated homes and stubbed HTTPS downloads."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = (ROOT/'install.sh').read_text()


class InstallerTests(unittest.TestCase):
    def exercise(self, architecture='x86_64', system='Linux', scenario='success', custom=False):
        with tempfile.TemporaryDirectory(prefix='tracr-installer-') as temp:
            root = Path(temp)
            home = root/'home'; home.mkdir()
            bin_dir = home/('custom bin' if custom else '.local/bin')
            bin_dir.mkdir(parents=True)
            destination = bin_dir/'tracr'
            destination.write_text('existing installation')
            downloads = root/'downloads'; downloads.mkdir()
            binary = b'#!/bin/sh\nprintf "tracr 0.2.0\\n"\n'
            if scenario == 'wrong-version': binary = binary.replace(b'0.2.0', b'9.0.0')
            assets = {'tracr-linux-x86_64':binary, 'tracr-linux-aarch64':binary,
                      'LICENSE':b'MIT notice', 'THIRD_PARTY_NOTICES.txt':b'Dependency notices'}
            for name, data in assets.items(): (downloads/name).write_bytes(data)
            sums = ''.join(hashlib.sha256(data).hexdigest()+'  '+name+'\n' for name, data in assets.items())
            if scenario == 'duplicate': sums += sums.splitlines()[0]+'\n'
            if scenario == 'missing-checksum': sums = '\n'.join(sums.splitlines()[1:])+'\n'
            if scenario == 'malformed': sums = sums.replace(hashlib.sha256(binary).hexdigest(), 'z'*64)
            (downloads/'SHA256SUMS').write_text(sums)
            if scenario == 'corrupt': (downloads/'tracr-linux-x86_64').write_bytes(b'tampered')
            mocks = root/'mocks'; mocks.mkdir()
            (mocks/'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; esac\n')
            (mocks/'curl').write_text(f'''#!{sys.executable}
import os,sys,pathlib,shutil
args=sys.argv[1:]
url=next(a for a in args if a.startswith('https://'))
assert url.startswith('https://github.com/BasedCodeDev/tracr/releases/download/v0.2.0/')
assert '--proto' in args and '--proto-redir' in args
with open(os.environ['TEST_LOG'],'a') as f: f.write(url+'\\n')
if os.environ['TEST_SCENARIO']=='unavailable': sys.exit(22)
shutil.copyfile(pathlib.Path(os.environ['TEST_DOWNLOADS'])/url.rsplit('/',1)[1], args[args.index('-o')+1])
''')
            for path in mocks.iterdir(): path.chmod(0o755)
            tmp = root/'tmp'; tmp.mkdir()
            env = {**os.environ, 'HOME':str(home), 'XDG_DATA_HOME':str(home/'data'),
                   'PATH':str(mocks)+':/usr/bin:/bin', 'TMPDIR':str(tmp),
                   'TEST_ARCH':architecture, 'TEST_OS':system, 'TEST_DOWNLOADS':str(downloads),
                   'TEST_LOG':str(root/'requests'), 'TEST_SCENARIO':scenario}
            env.pop('TRACR_VERSION', None); env.pop('TRACR_INSTALL_DIR', None)
            if custom: env['TRACR_INSTALL_DIR'] = str(bin_dir)
            if scenario == 'bad-version': env['TRACR_VERSION'] = '../../main'
            # Feed on stdin just like curl | sh; never use the developer's home.
            result = subprocess.run(['/bin/sh'], input=INSTALLER, text=True, capture_output=True, env=env, timeout=10)
            if scenario == 'success' and system == 'Linux' and architecture in ('x86_64', 'amd64', 'aarch64', 'arm64'):
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(destination.read_bytes(), binary)
                self.assertTrue(os.access(destination, os.X_OK))
                self.assertEqual((home/'data/tracr/LICENSE').read_bytes(), assets['LICENSE'])
                self.assertIn('PATH', result.stdout)
                expected = 'aarch64' if architecture in ('aarch64', 'arm64') else 'x86_64'
                self.assertIn('tracr-linux-'+expected, (root/'requests').read_text())
            else:
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(destination.read_text(), 'existing installation')
                self.assertFalse((home/'data/tracr').exists())
            self.assertEqual(list(tmp.iterdir()), [])
            self.assertEqual(list(bin_dir.glob('.tracr-install.*')), [])
            return result

    def test_platforms_and_custom_directory(self):
        for architecture in ('x86_64', 'amd64', 'aarch64', 'arm64'):
            with self.subTest(architecture=architecture):
                self.exercise(architecture=architecture, custom=True)
        self.exercise()

    def test_unsupported_platforms(self):
        self.exercise(system='Darwin')
        self.exercise(architecture='riscv64')

    def test_failed_downloads_or_verification_preserve_existing_binary(self):
        for scenario in ('unavailable','corrupt','duplicate','missing-checksum','malformed','wrong-version','bad-version'):
            with self.subTest(scenario=scenario):
                self.exercise(scenario=scenario)

    def test_default_preview_matches_cargo_version(self):
        version = tomllib.loads((ROOT/'Cargo.toml').read_text())['package']['version']
        self.assertIn('version=${TRACR_VERSION:-v'+version+'}', INSTALLER)


if __name__ == '__main__':
    unittest.main()
