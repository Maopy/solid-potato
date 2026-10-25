#!/usr/bin/env python3
"""Require all native archives and matching SHA-256 files before publishing."""
import argparse
import hashlib
from pathlib import Path
import tomllib


TARGETS = {'windows-x64': '.zip', 'macos-arm64': '.tar.gz'}


def verify_artifacts(directory, version, tag):
    if tag != f'v{version}':
        raise ValueError(f'Release tag {tag!r} does not match Cargo.toml version {version!r}')
    expected = {
        f'live-archive-moderation-{version}-{target}{extension}'
        for target, extension in TARGETS.items()
    }
    expected |= {f'{name}.sha256' for name in tuple(expected)}
    actual = {path.name for path in directory.iterdir()}
    if actual != expected:
        raise ValueError(f'Incomplete release: missing={sorted(expected - actual)}, '
                         f'unexpected={sorted(actual - expected)}')
    for name in sorted(expected):
        if name.endswith('.sha256'):
            continue
        archive = directory / name
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        recorded = (directory / f'{name}.sha256').read_text(encoding='utf-8').strip()
        if recorded != f'{checksum}  {name}':
            raise ValueError(f'SHA-256 mismatch: {name}')
    return sorted(expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    version = tomllib.loads((repo / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    if not (repo / 'docs' / f'moderation-release-{version}.md').is_file():
        raise SystemExit(f'Missing release notes for {version}')
    for name in verify_artifacts(args.directory, version, args.tag):
        print(f'Verified: {name}')


if __name__ == '__main__':
    main()
