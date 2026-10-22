#!/usr/bin/env python3
"""Package the built CLI, inheriting the previous local release's config."""
from pathlib import Path
import argparse
import hashlib
import re
import shutil
import subprocess
import tarfile
import tempfile

RELEASE_NAME = re.compile(r'live-archive-moderation-(\d+)\.(\d+)\.(\d+)-macos-arm64')
CONFIGS = ['douyin.yml', 'wechat.yml', 'wechat-observe.yml', 'douyin-auto.yml',
           'wechat-auto.yml', 'controller.yml', 'douyin-second.yml',
           'gift-wording.example.yml', 'nickname-mapping.yml', 'timed-messages.example.yml', 'keyword-replies.example.yml', 'recording.example.yml', 'likes.example.yml', 'unified.yml']
SCRIPTS = ['run-douyin.sh', 'run-wechat.sh', 'run-douyin-auto.sh',
           'run-wechat-auto.sh', 'run-controller.sh', 'run.sh', '启动场控.command']


def previous_config(dist, version):
    """Pick the latest older semantic version, never an unrelated/future folder."""
    current = tuple(map(int, version.split('.')))
    if not dist.is_dir():
        return None
    candidates = []
    for directory in dist.iterdir():
        match = RELEASE_NAME.fullmatch(directory.name)
        if match and directory.is_dir() and (directory / 'config').is_dir():
            candidate = tuple(map(int, match.groups()))
            if candidate < current:
                candidates.append((candidate, directory / 'config'))
    return max(candidates)[1] if candidates else None


def inherit_config(source, output, validator, keep_backup=False):
    """Stage and validate first; preserve every previous config file and value."""
    target = output / 'config'
    with tempfile.TemporaryDirectory(prefix='.config-inherit-', dir=output) as temporary:
        # Keep the stage at the same depth as config, for relative file paths.
        staged = Path(temporary)
        shutil.copytree(target, staged, dirs_exist_ok=True, symlinks=True)
        shutil.copytree(source, staged, dirs_exist_ok=True, symlinks=True)
        validator(staged)
        backup = output / 'config.before-inherit'
        index = 1
        while backup.exists():
            backup = output / f'config.before-inherit.{index}'
            index += 1
        target.rename(backup)
        try:
            staged.rename(target)
        except BaseException:
            backup.rename(target)
            raise
        if not keep_backup:
            shutil.rmtree(backup)
    return backup if keep_backup else None


def migrate_unified(directory):
    """Enable the new Douyin transport only when the old config has no preference."""
    path = directory / "unified.yml"
    if path.exists() and not re.search(r"^douyin_userscript\s*:", path.read_text(), re.M):
        with path.open("a") as stream:
            stream.write("\n# 抖音用户脚本接入；视频号继续使用代理。\ndouyin_userscript:\n  enabled: true\n  port: 2025\n  key_file: ../../.live-archive-userscript-key\n")


def migrate_recording(directory):
    """Expose the external default without overwriting any recording preference."""
    example = directory / 'recording.example.yml'
    if example.exists():
        text = example.read_text().replace('  # output_dir: ../data/recordings', '  # 默认系统下载文件夹/直播录屏；自定义请填写绝对路径。\n  # output_dir: /Volumes/Media/直播录屏')
        example.write_text(text)
    for name in ['unified.yml', 'controller.yml']:
        path = directory / name
        if path.exists() and not re.search(r'^recording\s*:', path.read_text(), re.M):
            with path.open('a') as stream:
                stream.write('\n# 录屏默认保存到系统下载文件夹/直播录屏；自定义请填写绝对路径。\nrecording:\n  output_dir: null\n')


def migrate_likes(directory):
    """Expose requested presets only when the room has no existing preference."""
    for name, mode in [('douyin-auto.yml', 'clock'), ('wechat-auto.yml', 'continuous')]:
        path = directory / name
        if path.exists() and not re.search(r'^likes\s*:', path.read_text(), re.M):
            with path.open('a') as stream:
                stream.write('\n# 点赞独立于场控和录制；页面加载后默认关闭，在面板手动开启。\n'
                             f'likes:\n  enabled: true\n  mode: {mode}\n  interval_ms: 800\n'
                             '  every_minutes: 10\n  batch_count: 200\n'
                             '  missed: catch_up_once\n  target_selector: null\n')


def validate_configs(binary, directory):
    migrate_unified(directory)
    migrate_recording(directory)
    migrate_likes(directory)
    example = directory / 'timed-messages.example.yml'
    if example.exists():
        text = example.read_text()
        if not re.search(r'^  min_comments\s*:', text, re.M):
            text = re.sub(r'^(timed_messages:\s*\n)', r'\1  # 时间与评论条数同时满足才发；0 保持只按时间，按需改为 5 等数值。\n  min_comments: 0\n', text, count=1, flags=re.M)
            example.write_text(text)
    for name in ['douyin.yml', 'douyin-auto.yml', 'douyin-second.yml',
                 'wechat-observe.yml', 'wechat.yml', 'wechat-auto.yml']:
        path = directory / name
        if not path.exists():
            continue
        result = subprocess.run([binary, 'moderation', 'check', '--config', path],
                                capture_output=True, text=True)
        placeholder = name in ['wechat.yml', 'wechat-auto.yml'] and bool(re.search(
            r"^\s*recipient_user_id:\s*(?:''|\"\")\s*(?:#.*)?$", path.read_text(), re.M))
        if result.returncode and not (placeholder and 'recipient_user_id' in result.stderr):
            raise RuntimeError(f'Configuration check failed: {name}\n{result.stderr}')
    for name in ['controller.yml','unified.yml']:
        controller=directory/name
        if controller.exists():
            subprocess.run([binary, 'moderation', 'check-controller', '--config', controller],check=True)


def release_assets(repo, version):
    assets = {
        'live-archive-core': repo / 'target/release/live-archive-core',
        'LICENSE': repo / 'LICENSE',
        'tools/read_moderation_raw.py': repo / 'tools/read_moderation_raw.py',
        'THIRD_PARTY_NOTICES.md': repo / 'THIRD_PARTY_NOTICES.md',
        '使用说明.md': repo / f'docs/moderation-release-{version}.md',
    }
    assets.update({f'config/{name}': repo / 'release' / name for name in CONFIGS})
    assets.update({name: repo / 'release' / name for name in SCRIPTS})
    for name in ['moderation-release-0.2.0.md', 'moderation-release-0.2.1.md',
                 'moderation-release-0.2.2.md', 'moderation-release-0.2.3.md',
                 'moderation-release-0.2.4.md', 'moderation-release-0.2.5.md',
                 'moderation-release-0.2.6.md', 'moderation-release-0.2.7.md',
                 'moderation-release-0.3.0.md', 'moderation-release-0.3.1.md',
                 'moderation-release-0.3.2.md', 'moderation-release-0.3.3.md',
                 'moderation-release-0.3.4.md', 'moderation-release-0.3.5.md',
                 'moderation-release-0.3.6.md', 'moderation-release-0.3.7.md',
                 'moderation-release-0.3.8.md', 'moderation-release-0.3.9.md', 'moderation-release-0.3.10.md', 'moderation-release-0.3.11.md', 'moderation-release-0.3.12.md', 'moderation-release-0.3.13.md', 'moderation-release-0.3.14.md', 'moderation-release-0.3.15.md', 'moderation-unified.md',
                 'moderation-keywords.md', 'moderation-recording.md', 'moderation-likes.md',
                 f'moderation-release-{version}.md', 'moderation-next-version.md',
                 'moderation-welcome.md']:
        assets[f'docs/{name}'] = repo / 'docs' / name
    return assets


def archive_release(output, assets):
    # Inherit custom/nested config files as well as the standard filenames.
    names = set(assets)
    names.update(str(path.relative_to(output)) for path in (output / 'config').rglob('*')
                 if path.is_file() or path.is_symlink())
    archive = output.parent / f'{output.name}.tar.gz'
    with tarfile.open(archive, 'w:gz') as bundle:
        for relative in sorted(names):
            bundle.add(output / relative, arcname=f'{output.name}/{relative}', recursive=False)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    Path(str(archive) + '.sha256').write_text(f'{checksum}  {archive.name}\n')
    print(archive)
    print(f'SHA256: {checksum}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inherit-config-only', action='store_true',
                        help='repair this existing release and rebuild its archive; keep a config backup')
    parser.add_argument('--config-from', type=Path, help='inherit the config directory of the currently running release')
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    version = re.search(r'^version = "([^"]+)"', (repo / 'Cargo.toml').read_text(), re.M)[1]
    output = repo / 'dist' / f'live-archive-moderation-{version}-macos-arm64'
    source = args.config_from.resolve() if args.config_from else previous_config(output.parent, version)
    if source is not None and not source.is_dir():
        raise SystemExit(f'Config source does not exist: {source}')
    assets = release_assets(repo, version)
    if args.inherit_config_only:
        if not (output / 'config').is_dir() or source is None:
            raise SystemExit('An existing release and a previous config directory are required')
    else:
        if output.exists():
            raise SystemExit(f'Release directory already exists; choose a fresh version: {output}')
        output.mkdir(parents=True)
        for relative, original in assets.items():
            destination = output / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original, destination)
            if relative.endswith(('.sh','.command')) or relative == 'live-archive-core':
                destination.chmod(0o755)
    binary = output / 'live-archive-core'
    assert subprocess.check_output([binary, '--version'], text=True).strip().endswith(version)
    if source is not None:
        backup = inherit_config(source, output, lambda directory: validate_configs(binary, directory),
                                keep_backup=args.inherit_config_only)
        print(f'Inherited config: {source}')
        if backup:
            print(f'Previous destination config backup: {backup}')
    else:
        validate_configs(binary, output / 'config')
    archive_release(output, assets)


if __name__ == '__main__':
    main()
