"""Publishing must wait for a complete, consistent set of native packages."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    'verify_artifacts', Path(__file__).resolve().parents[1] / 'release/verify_artifacts.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseArtifacts(unittest.TestCase):
    def stage(self, directory):
        for target, extension in release.TARGETS.items():
            name = f'live-archive-moderation-1.2.3-{target}{extension}'
            archive = directory / name
            archive.write_bytes(target.encode())
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            (directory / f'{name}.sha256').write_text(f'{digest}  {name}\n', encoding='utf-8')

    def test_complete_release_passes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.stage(directory)
            self.assertEqual(len(release.verify_artifacts(directory, '1.2.3', 'v1.2.3')), 4)

    def test_missing_mac_or_checksum_blocks_publication(self):
        for name in ['live-archive-moderation-1.2.3-macos-arm64.tar.gz',
                     'live-archive-moderation-1.2.3-windows-x64.zip.sha256']:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.stage(directory)
                (directory / name).unlink()
                with self.assertRaisesRegex(ValueError, 'Incomplete release'):
                    release.verify_artifacts(directory, '1.2.3', 'v1.2.3')

    def test_modified_archive_blocks_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.stage(directory)
            (directory / 'live-archive-moderation-1.2.3-windows-x64.zip').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'SHA-256 mismatch'):
                release.verify_artifacts(directory, '1.2.3', 'v1.2.3')

    def test_wrong_version_and_unexpected_files_block_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.stage(directory)
            with self.assertRaisesRegex(ValueError, 'does not match'):
                release.verify_artifacts(directory, '1.2.3', 'v1.2.4')
            (directory / 'private-config.yml').write_text('local config', encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'unexpected='):
                release.verify_artifacts(directory, '1.2.3', 'v1.2.3')


if __name__ == '__main__':
    unittest.main()
