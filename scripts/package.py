#!/usr/bin/env python3
"""Build the native standalone and create a local portable platform package."""
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--skip-build', action='store_true')
args = parser.parse_args()
if not args.skip_build:
    subprocess.run(['cargo', 'build', '--release', '--locked'], cwd=root, check=True)
system = platform.system()
exe = root / 'target' / 'release' / ('iron-room.exe' if system == 'Windows' else 'iron-room')
if not exe.is_file():
    raise SystemExit(f'Binary not found: {exe}')
dist = root / 'dist'
dist.mkdir(exist_ok=True)
if system == 'Darwin':
    dest = dist / 'Iron Room.app'
    if dest.exists():
        shutil.rmtree(dest)
    bindir = dest / 'Contents' / 'MacOS'
    resources = dest / 'Contents' / 'Resources'
    bindir.mkdir(parents=True)
    resources.mkdir(parents=True)
    shutil.copy2(exe, bindir / 'iron-room')
    shutil.copytree(root / 'assets', resources / 'assets')
    shutil.copy2(root / 'LICENSE', resources / 'LICENSE')
    shutil.copy2(root / 'docs' / 'THIRD_PARTY.md', resources / 'THIRD_PARTY.md')
    licence_dir = resources / 'licences'
    import plistlib
    with (dest / 'Contents' / 'Info.plist').open('wb') as f:
        plistlib.dump({'CFBundleName': 'Iron Room', 'CFBundleDisplayName': 'Iron Room',
                      'CFBundleIdentifier': 'org.ironroom.soniclaw.standalone',
                      'CFBundleExecutable': 'iron-room', 'CFBundlePackageType': 'APPL',
                      'CFBundleShortVersionString': '0.1.0', 'CFBundleVersion': '1',
                      'NSHighResolutionCapable': True, 'LSMinimumSystemVersion': '11.0'}, f)
    subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(dest)], check=True)
else:
    dest = dist / f'iron-room-soniclaw-{system.lower()}-{platform.machine()}'
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir()
    shutil.copy2(exe, dest / exe.name)
    if system != 'Windows':
        os.chmod(dest / exe.name, 0o755)
    shutil.copytree(root / 'assets', dest / 'assets')
    for file in ['README.md', 'LICENSE']:
        shutil.copy2(root / file, dest / file)
    shutil.copy2(root / 'docs' / 'THIRD_PARTY.md', dest / 'THIRD_PARTY.md')
    licence_dir = dest / 'licences'
licence_dir.mkdir()
for name in ['LICENSE-MIT', 'LICENSE-APACHE']:
    shutil.copy2(root / 'vendor' / 'soniclaw' / name, licence_dir / ('SONICLAW-' + name))
print(dest)
