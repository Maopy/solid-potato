"""Config migration must preserve user settings, custom files, and rollback."""
import importlib.util
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release_package', Path(__file__).resolve().parents[1] / 'release/package.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class ReleaseConfigInheritance(unittest.TestCase):
    def test_recipient_migration_preserves_manual_targets_and_explicit_opt_out(self):
        for fields in ["  recipient_user_id: manual-host\n",
                       "  recipient_from_page: false\n  recipient_user_id: ''\n"]:
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                path = directory / 'wechat-auto.yml'
                original = 'candidate_preview:\n  enabled: true\n' + fields
                path.write_text(original, encoding='utf-8')
                package.migrate_wechat_recipient(directory)
                self.assertEqual(path.read_text(encoding='utf-8'), original)

    def test_packaging_never_suppresses_a_runtime_recipient_validation_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / 'wechat-auto.yml').write_text(
                "candidate_preview:\n  enabled: true\n  recipient_user_id: ''\n"
                "  recipient_from_page: false\nlikes: {}\n", encoding='utf-8')
            with patch.object(package.subprocess, 'run', return_value=SimpleNamespace(
                    returncode=1, stderr='candidate_preview requires an explicit valid recipient_user_id')):
                with self.assertRaisesRegex(RuntimeError, 'recipient_user_id'):
                    package.validate_configs('/unused', directory)

    def test_controller_validation_does_not_replace_shipped_empty_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / 'config'
            directory.mkdir()
            room = directory / 'wechat-auto.yml'
            original = "candidate_preview:\n  enabled: true\n  recipient_user_id: ''\nlikes:\n  enabled: false\n"
            room.write_text(original, encoding='utf-8')
            (directory / 'unified.yml').write_text('version: 2\n', encoding='utf-8')
            checked = []
            def validate(arguments, **kwargs):
                if 'check-controller' in arguments:
                    staged = Path(arguments[-1]).parent
                    self.assertEqual(staged, directory)
                    self.assertIn('recipient_from_page: true',
                                  (staged / room.name).read_text(encoding='utf-8'))
                    checked.append(staged)
                return SimpleNamespace(returncode=0, stderr='')
            with patch.object(package.subprocess, 'run', side_effect=validate):
                package.validate_configs('/unused', directory)
            self.assertEqual(len(checked), 1)
            self.assertTrue(checked[0].exists())
            self.assertEqual(room.read_text(encoding='utf-8'), original.replace("candidate_preview:\n", "candidate_preview:\n  recipient_from_page: true\n"))

    def test_platform_selection_does_not_inherit_mac_paths_on_windows(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            for name in ['0.3.10-windows-x64', '0.3.11-macos-arm64', '0.3.12-windows-arm64']:
                (dist / f'live-archive-moderation-{name}/config').mkdir(parents=True)
            self.assertEqual(package.previous_config(dist, '0.3.13', 'windows-x64').parent.name,
                             'live-archive-moderation-0.3.10-windows-x64')
            self.assertIsNone(package.previous_config(dist, '0.3.13', 'linux-x64'))

    def test_windows_assets_use_exe_and_cmd_entrypoints(self):
        repo = Path(__file__).resolve().parents[1]
        assets = package.release_assets(repo, '0.3.65', 'windows-x64')
        self.assertIn('live-archive-core.exe', assets)
        self.assertIn('启动场控.cmd', assets)
        self.assertIn('run.cmd', assets)
        self.assertNotIn('run.sh', assets)
        self.assertNotIn('live-archive-core', assets)

    def test_windows_zip_preserves_chinese_paths_and_excludes_runtime_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / '中文 发布包'
            (output / 'config/自定义').mkdir(parents=True)
            (output / 'config/自定义/设置.yml').write_text('主播: 测试', encoding='utf-8')
            (output / 'data').mkdir()
            (output / 'data/private.db').write_bytes(b'private')
            (output / 'live-archive-core.exe').write_bytes(b'binary')
            package.archive_release(output, {'live-archive-core.exe': None}, 'windows-x64')
            archive_path = output.with_suffix('.zip')
            with zipfile.ZipFile(archive_path) as archive:
                self.assertEqual(set(archive.namelist()), {
                    '中文 发布包/live-archive-core.exe', '中文 发布包/config/自定义/设置.yml'})
                self.assertEqual(archive.read('中文 发布包/config/自定义/设置.yml').decode('utf-8'), '主播: 测试')
            self.assertTrue(Path(str(archive_path) + '.sha256').is_file())

    def test_previous_version_is_numeric_and_excludes_current_future_and_other_folders(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            for version in ['0.3.7', '0.3.9', '0.3.10', '0.3.11', '0.4.0']:
                (dist / f'live-archive-moderation-{version}-macos-arm64/config').mkdir(parents=True)
            (dist / 'unrelated/config').mkdir(parents=True)
            self.assertEqual(package.previous_config(dist, '0.3.11').parent.name,
                             'live-archive-moderation-0.3.10-macos-arm64')
            self.assertIsNone(package.previous_config(dist, '0.1.0'))
            self.assertIsNone(package.previous_config(dist / 'not-created-yet', '0.3.11'))

    def test_recording_default_is_exposed_and_custom_paths_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            config = Path(temporary)
            (config / 'unified.yml').write_text('version: 2\n')
            custom = 'version: 2\nrecording:\n  output_dir: /external/videos\n  segment_seconds: 600\n'
            (config / 'controller.yml').write_text(custom)
            package.migrate_recording(config)
            first = (config / 'unified.yml').read_text()
            self.assertIn('output_dir: null', first)
            self.assertEqual((config / 'controller.yml').read_text(), custom)
            package.migrate_recording(config)
            self.assertEqual((config / 'unified.yml').read_text(), first)

    def test_user_values_and_nested_files_are_inherited_and_new_files_are_retained(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            source = base / 'old/config'
            source.mkdir(parents=True)
            (source / 'wechat-auto.yml').write_text('timed_messages:\n  enabled: true\n  interval_seconds: 30\n')
            (source / 'aliases').mkdir()
            (source / 'aliases/custom.yml').write_text('custom nickname mapping')
            output = base / 'new'
            (output / 'config').mkdir(parents=True)
            original = 'timed_messages:\n  enabled: false\n'
            (output / 'config/wechat-auto.yml').write_text(original)
            (output / 'config/new-feature.example.yml').write_text('new default file')
            validated = []
            def validate(staged):
                self.assertEqual((staged / 'wechat-auto.yml').read_bytes(), (source / 'wechat-auto.yml').read_bytes())
                validated.append(staged)
            backup = package.inherit_config(source, output, validate, keep_backup=True)
            self.assertEqual(len(validated), 1)
            self.assertEqual((backup / 'wechat-auto.yml').read_text(), original)
            self.assertTrue((output / 'config/new-feature.example.yml').exists())
            self.assertEqual((output / 'config/aliases/custom.yml').read_text(), 'custom nickname mapping')
            self.assertEqual((output / 'config/wechat-auto.yml').read_bytes(), (source / 'wechat-auto.yml').read_bytes())

    def test_failed_validation_keeps_destination_untouched(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            source = base / 'old'; source.mkdir()
            (source / 'room.yml').write_text('bad previous configuration')
            output = base / 'new'; (output / 'config').mkdir(parents=True)
            (output / 'config/room.yml').write_text('working destination')
            def reject(staged):
                raise ValueError('invalid inherited configuration')
            with self.assertRaises(ValueError):
                package.inherit_config(source, output, reject, keep_backup=True)
            self.assertEqual((output / 'config/room.yml').read_text(), 'working destination')
            self.assertEqual(list(output.iterdir()), [output / 'config'])

    def test_userscript_migration_preserves_other_settings_and_explicit_preference(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = directory / 'unified.yml'
            original = 'version: 2\nstart_enabled: false\n# 用户文案\n'
            path.write_text(original)
            package.migrate_unified(directory)
            self.assertTrue(path.read_text().startswith(original))
            self.assertIn('key_file: ../../.live-archive-userscript-key', path.read_text())
            once = path.read_text()
            package.migrate_unified(directory)
            self.assertEqual(path.read_text(), once)
            path.write_text(original + 'douyin_userscript:\n  enabled: false\n')
            package.migrate_unified(directory)
            self.assertEqual(path.read_text(), original + 'douyin_userscript:\n  enabled: false\n')

    def test_timed_example_adds_comment_threshold_without_overwriting_user_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = directory / 'timed-messages.example.yml'
            original = 'timed_messages:\n  interval_seconds: 30\n  messages: [custom]\n'
            path.write_text(original)
            package.validate_configs('/unused', directory)
            self.assertIn('  min_comments: 0\n', path.read_text())
            self.assertIn('  interval_seconds: 30\n  messages: [custom]\n', path.read_text())
            once = path.read_text()
            package.validate_configs('/unused', directory)
            self.assertEqual(path.read_text(), once)
            path.write_text(original + '  min_comments: 7\n')
            package.validate_configs('/unused', directory)
            self.assertEqual(path.read_text(), original + '  min_comments: 7\n')

    def test_likes_presets_are_independent_and_existing_preferences_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            dy = directory / 'douyin-auto.yml'
            wx = directory / 'wechat-auto.yml'
            dy.write_text('sending:\n  automatic: false\n')
            custom = 'likes:\n  enabled: false\n  mode: continuous\n  interval_ms: 2000\n'
            wx.write_text(custom)
            package.migrate_likes(directory)
            self.assertTrue(dy.read_text().startswith('sending:\n  automatic: false\n'))
            self.assertIn('mode: clock', dy.read_text())
            self.assertIn('batch_count: 200', dy.read_text())
            self.assertEqual(wx.read_text(), custom)
            once = dy.read_text()
            package.migrate_likes(directory)
            self.assertEqual(dy.read_text(), once)

    def test_archive_includes_inherited_custom_files_and_excludes_backups_and_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'release'
            (output / 'config/custom').mkdir(parents=True)
            (output / 'config/custom/settings.yml').write_text('user settings')
            (output / 'config.before-inherit').mkdir()
            (output / 'config.before-inherit/old.yml').write_text('old defaults')
            (output / 'data').mkdir()
            (output / 'data/journal.sqlite3').write_bytes(b'user data')
            (output / 'binary').write_bytes(b'binary')
            package.archive_release(output, {'binary': None})
            with tarfile.open(output.parent / 'release.tar.gz') as archive:
                self.assertEqual(set(archive.getnames()), {'release/binary', 'release/config/custom/settings.yml'})
                self.assertEqual(archive.extractfile('release/config/custom/settings.yml').read(), b'user settings')


if __name__ == '__main__':
    unittest.main()
