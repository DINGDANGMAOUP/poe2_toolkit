#!/usr/bin/env python3
"""Create a native Velopack installer and full update package on a clean runner."""
import argparse
import hashlib
import json
import platform
import plistlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib

TARGETS = {
    'windows-x86_64': ('x86_64-pc-windows-msvc', 'win-x64', '.exe'),
    'macos-aarch64': ('aarch64-apple-darwin', 'osx-arm64', ''),
    'macos-x86_64': ('x86_64-apple-darwin', 'osx-x64', ''),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--vpk', default='vpk')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    config = json.loads((root / 'config/distribution.json').read_text())
    version = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    triple, runtime, extension = TARGETS[args.target]
    native = ('windows' if sys.platform == 'win32' else 'macos') + '-' + (
        'aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64')
    if native != args.target:
        raise ValueError(f'Build on the native target: {native} != {args.target}')
    staging = root / 'dist' / f'input-{args.target}'
    output = root / 'dist' / args.target
    staging.mkdir(parents=True, exist_ok=False)
    output.mkdir(parents=True, exist_ok=False)
    for binary in ('poe2-toolkit', 'poe2ctl'):
        shutil.copy2(root / 'target/release' / (binary + extension), staging)
    subprocess.run([sys.executable, str(root / '.github/scripts/licenses.py'),
                    '--target', triple, '--metadata-output', str(staging / 'dependency-licenses.json')], check=True)
    shutil.copytree(root / 'dist/third-party-licenses', staging / 'third-party-licenses')
    for file in staging.rglob('*'):
        if file.is_file() and (file.suffix.lower() in ('.rs', '.py', '.ps1', '.toml')
                or any(part.lower() in ('tests', 'fixtures', 'benches', 'docs', '.github')
                       for part in file.relative_to(staging).parts)):
            raise ValueError(f'Development file in application bundle: {file.relative_to(staging)}')
    command = [args.vpk, 'pack', '--packId', 'POE2Toolkit', '--packVersion', version,
               '--packDir', str(staging), '--mainExe', 'poe2-toolkit' + extension,
               '--packTitle', 'PoE Toolkit', '--channel', config['channel'],
               '--runtime', runtime, '--outputDir', str(output), '--delta', 'None',
               '--icon', str(root / 'apps/desktop/assets/icons' / ('app.ico' if extension else 'app.icns')),
               '--yes', '--skip-updates']
    if extension:
        command += ['--framework', 'vcredist143-x64']
    else:
        plist = staging.parent / f'{args.target}-Info.plist'
        plist.write_bytes(plistlib.dumps({
            'CFBundleName': 'PoE Toolkit', 'CFBundleDisplayName': 'PoE Toolkit',
            'CFBundleIdentifier': 'dev.poe2toolkit.desktop', 'CFBundleExecutable': 'poe2-toolkit',
            'CFBundleIconFile': 'app.icns', 'CFBundlePackageType': 'APPL',
            'CFBundleShortVersionString': version, 'CFBundleVersion': version,
            'NSHighResolutionCapable': True, 'NSPrincipalClass': 'NSApplication',
            'LSMinimumSystemVersion': '14.0',
        }))
        command += ['--plist', str(plist)]
    subprocess.run(command, check=True)
    # Prefix every public asset to avoid collisions across architecture/channel feeds.
    feeds = list(output.glob('releases.*.json'))
    if len(feeds) != 1:
        raise ValueError('Expected one Velopack feed')
    feed = json.loads(feeds[0].read_text())
    for asset in feed['Assets']:
        old = output / asset['FileName']
        new = old.with_name(f'{args.target}-{old.name}')
        old.rename(new)
        asset['FileName'] = new.name
        if new.stat().st_size != asset['Size'] or hashlib.file_digest(new.open('rb'), 'sha256').hexdigest().lower() != asset['SHA256'].lower():
            raise ValueError('Velopack artifact integrity mismatch')
    for file in list(output.iterdir()):
        if file.is_file() and file.suffix.lower() in ('.exe', '.pkg', '.zip'):
            file.rename(file.with_name(f'{args.target}-{file.name}'))
    # Raw Velopack feeds are build inputs only. Applications consume the signed envelope.
    metadata = {'target': args.target, 'version': version, 'channel': config['channel'], 'Assets': feed['Assets']}
    (output / f'{args.target}-build.json').write_text(json.dumps(metadata, indent=2) + '\n')
    # Windows globbing is case-insensitive: RELEASES* also matches releases.beta.json.
    for file in set(feeds) | set(output.glob('RELEASES*')) | set(output.glob('assets.*.json')):
        file.unlink()
    # Exercise the binaries inside the deliverable, including updater discovery.
    # This does not install into the runner's Applications / user profile.
    archives = list(output.glob('*.zip'))
    if len(archives) != 1:
        raise ValueError('Expected exactly one portable bundle')
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        unpacked = temporary / 'unpacked'
        if extension:
            shutil.unpack_archive(archives[0], unpacked)
        else:
            subprocess.run(['ditto', '-x', '-k', str(archives[0]), str(unpacked)], check=True)
        binaries = list(unpacked.rglob('poe2ctl' + extension))
        if len(binaries) != 1:
            raise ValueError('CLI missing or duplicated in release bundle')
        status = json.loads(subprocess.check_output([
            str(binaries[0]), '--data-dir', str(temporary / 'state'), 'app-update-status']))
        if not status['configured'] or not status['installed'] or status['pending_version']:
            raise ValueError(f'Packaged updater is not ready: {status}')
        print('Application bundle and updater ready')
    print(f'Packaged {args.target} {version}: {output}')


if __name__ == '__main__':
    main()
