import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'scripts'))
import release
import draft_release


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_manifest(self):
        release.validate_tag('v' + release.version())
        for tag in ('0.1.0', 'v999.0.0', '../main'):
            with self.assertRaises(ValueError):
                release.validate_tag(tag)

    def test_binary_validation(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp)/'tracr'
            with self.assertRaisesRegex(ValueError, 'unsupported'):
                release.verify_binary(binary, 'windows', '0.1.0')
            with self.assertRaisesRegex(ValueError, 'missing'):
                release.verify_binary(binary, next(iter(release.TARGETS)), '0.1.0')
            binary.write_text('fake'); binary.chmod(0o755)
            good = ['Machine: Advanced Micro Devices X86-64\n', 'LOAD', 'No dynamic section', 'tracr 0.1.0\n']
            for outputs, error in ((good, None), (['Machine: AArch64\n'], 'architecture'),
                    ([good[0], 'INTERP', good[2]], 'statically'),
                    ([good[0], 'LOAD', '(NEEDED) libc.so'], 'statically'),
                    (good[:3] + ['tracr 0.2.0'], 'version')):
                with patch.object(release, 'run', side_effect=outputs):
                    if error:
                        with self.assertRaisesRegex(ValueError, error):
                            release.verify_binary(binary, next(iter(release.TARGETS)), '0.1.0')
                    else:
                        release.verify_binary(binary, next(iter(release.TARGETS)), '0.1.0')

    def test_assembly_verifies_both_inputs_and_combines_notices(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name, _ in release.TARGETS.values():
                folder = root/name; folder.mkdir()
                (folder/name).write_bytes(b'binary')
                (folder/'LICENSE').write_text('MIT')
                (folder/'THIRD_PARTY_NOTICES.txt').write_text(name+' notices')
                (folder/'SHA256SUMS').write_text(release.checksums(folder, (name, *release.COMMON)))
            args = types.SimpleNamespace(input=root, output=root/'merged', tag='v'+release.version())
            release.assemble(args)
            names = [v[0] for v in release.TARGETS.values()] + list(release.COMMON)
            release.verify_checksums(args.output, names)
            self.assertIn('tracr-linux-aarch64 notices', (args.output/'THIRD_PARTY_NOTICES.txt').read_text())
            (args.output/names[0]).write_bytes(b'tampered')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                release.verify_checksums(args.output, names)
            args.output = root/'other'
            (root/names[0]/names[0]).write_bytes(b'tampered')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                release.assemble(args)
            self.assertFalse(args.output.exists())

    def test_notices_preserve_text_and_reject_missing_license(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            metadata = {'packages':[{'name':'dependency', 'version':'1', 'license':'MIT', 'manifest_path':str(folder/'Cargo.toml')}]}
            with self.assertRaisesRegex(ValueError, 'no license'):
                release.notices(metadata)
            (folder/'LICENSE').write_text('Copyright Example\nPermission granted.')
            self.assertIn('Copyright Example\nPermission granted.', release.notices(metadata))

    def test_publishing_refuses_public_release_and_api_failure(self):
        with patch.object(draft_release, 'verify_checksums'), patch.object(draft_release.subprocess, 'run') as mutation:
            with patch.object(draft_release.subprocess, 'check_output', return_value=json.dumps([[{'tag_name':'v'+release.version(), 'draft':False}]])):
                with self.assertRaisesRegex(ValueError, 'published'):
                    draft_release.publish('v'+release.version(), Path('/unused'))
            with patch.object(draft_release.subprocess, 'check_output', side_effect=subprocess.CalledProcessError(1, 'gh')):
                with self.assertRaises(subprocess.CalledProcessError):
                    draft_release.publish('v'+release.version(), Path('/unused'))
            mutation.assert_not_called()

    def test_draft_creation_and_rerun(self):
        for exists in (False, True):
            tag = 'v'+release.version()
            listing = [[{'tag_name':tag, 'draft':True}]] if exists else [[]]
            with patch.object(draft_release, 'verify_checksums'), \
                    patch.object(draft_release.subprocess, 'check_output', side_effect=[json.dumps(listing), '{"isDraft":true}']), \
                    patch.object(draft_release.subprocess, 'run') as mutation:
                draft_release.publish(tag, Path('/assets'))
                commands = [call.args[0] for call in mutation.call_args_list]
                self.assertEqual(len(commands), 1 if exists else 2)
                if not exists:
                    self.assertIn('--draft', commands[0]); self.assertIn('--prerelease', commands[0])
                self.assertEqual(commands[-1][:3], ['gh', 'release', 'upload'])

    def test_toolchain_must_match_pin(self):
        with patch.object(release, 'run', return_value='rustc 1.0.0 (old)'), \
                self.assertRaisesRegex(ValueError, 'requires Rust'):
            release.toolchain()

    def test_download_verification_installs_executable_in_temporary_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in [v[0] for v in release.TARGETS.values()] + list(release.COMMON):
                (root/name).write_text('downloaded')
            names = [v[0] for v in release.TARGETS.values()] + list(release.COMMON)
            (root/'SHA256SUMS').write_text(release.checksums(root, names))
            checked = []
            def inspect(binary, target, version):
                self.assertTrue(os.access(binary, os.X_OK))
                self.assertEqual(binary.read_text(), 'downloaded')
                checked.append(binary)
            with patch.object(release, 'verify_binary', side_effect=inspect), patch.object(release, 'smoke') as smoke:
                release.verify_download(types.SimpleNamespace(input=root, target=next(iter(release.TARGETS))))
                smoke.assert_called_once_with(checked[0])
            self.assertFalse(checked[0].exists())

    def test_release_published_during_upload_preparation_is_rejected(self):
        tag = 'v'+release.version()
        with patch.object(draft_release, 'verify_checksums'), \
                patch.object(draft_release.subprocess, 'check_output', side_effect=[
                    json.dumps([[{'tag_name':tag, 'draft':True}]]), '{"isDraft":false}']), \
                patch.object(draft_release.subprocess, 'run') as mutation:
            with self.assertRaisesRegex(ValueError, 'no longer a draft'):
                draft_release.publish(tag, Path('/assets'))
            mutation.assert_not_called()


if __name__ == '__main__':
    unittest.main()
