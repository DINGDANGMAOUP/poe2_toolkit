#!/usr/bin/env python3
"""Publish immutable packages and atomically advance the signed beta update feed.

Only the publish/refresh job receives the Ed25519 secret. No third-party Python
packages are needed; OpenSSL signs the same domain-separated bytes as Rust.
"""
import argparse
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
CONFIG = json.loads((ROOT / 'config/distribution.json').read_text())
TARGETS = ('windows-x86_64', 'macos-aarch64', 'macos-x86_64')
DOMAIN = b'poe2-toolkit/application/v1\0'
OPENSSL = os.environ.get('OPENSSL', 'openssl')


def version_tuple(value):
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', value):
        raise ValueError('Release versions must be x.y.z; beta releases use the beta channel')
    return tuple(map(int, value.split('.')))


def validate_tag(tag, version):
    version_tuple(version)
    if tag != 'v' + version:
        raise ValueError(f'Tag {tag!r} must match Cargo workspace version v{version}')


def run(*args, **kwargs):
    return subprocess.check_output(list(args), **kwargs)


def api(path, body=None, missing=False, method=None):
    command = ['gh', 'api', f'repos/{CONFIG["repository"]}/{path}']
    data = None
    if body is not None:
        command += ['--input', '-']
        data = json.dumps(body).encode()
    if method:
        command += ['--method', method]
    result = subprocess.run(command, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        if missing and b'HTTP 404' in result.stderr:
            return None
        raise RuntimeError(result.stderr.decode())
    return json.loads(result.stdout) if result.stdout else None


def encode(payload):
    return json.dumps(payload, ensure_ascii=False, separators=(',', ':'))


def verify_envelope(envelope):
    payload = envelope['payload']
    signature = bytes.fromhex(envelope['signature_hex'])
    if len(payload.encode()) > 256 * 1024 or len(signature) != 64:
        raise ValueError('Invalid signed envelope length')
    public_der = bytes.fromhex('302a300506032b6570032100' + CONFIG['public_key_hex'])
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / 'public.der').write_bytes(public_der)
        (tmp / 'message').write_bytes(DOMAIN + payload.encode())
        (tmp / 'signature').write_bytes(signature)
        run(OPENSSL, 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER', '-inkey', str(tmp / 'public.der'),
            '-rawin', '-in', str(tmp / 'message'), '-sigfile', str(tmp / 'signature'), stderr=subprocess.PIPE)
    release = json.loads(payload)
    validate_release(release)
    return release


def validate_release(release):
    asset = release['asset']
    target = release['target']
    version_tuple(asset['Version'])
    if (release['protocol'] != 1 or release['channel'] != CONFIG['channel']
            or target not in TARGETS or release['sequence'] <= 0
            or asset['PackageId'] != 'POE2Toolkit' or asset['Type'] != 'Full'
            or not 0 < asset['Size'] <= 2 * 1024**3
            or not re.fullmatch(r'[a-fA-F0-9]{64}', asset['SHA256'])
            or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,193}\.nupkg', asset['FileName'])
            or not asset['FileName'].startswith(target + '-')):
        raise ValueError('Invalid package identity, platform or integrity metadata')
    expected = f'https://github.com/{CONFIG["repository"]}/releases/download/v{asset["Version"]}/{asset["FileName"]}'
    if release['package_url'] != expected:
        raise ValueError('Package must be pinned to this repository and version tag')


def sign(payload, key):
    validate_release(payload)
    text = encode(payload)
    with tempfile.TemporaryDirectory() as tmp:
        message = Path(tmp) / 'message'
        message.write_bytes(DOMAIN + text.encode())
        signature = run(OPENSSL, 'pkeyutl', '-sign', '-rawin', '-inkey', str(key), '-in', str(message))
    result = {'payload': text, 'signature_hex': signature.hex()}
    # Also proves the secret corresponds to the public key embedded in the app.
    verify_envelope(result)
    return result


def feed_state():
    ref = api('git/ref/heads/updates', missing=True)
    if ref is None:
        return None, {}
    head = ref['object']['sha']
    result = {}
    for target in TARGETS:
        file = api(f'contents/{CONFIG["channel"]}/{target}.json?ref={head}')
        envelope = json.loads(base64.b64decode(file['content']))
        release = verify_envelope(envelope)
        if release['target'] != target:
            raise ValueError('Feed path/target mismatch')
        result[target] = release
    return head, result


def envelopes(releases, previous, key):
    now = datetime.now(timezone.utc)
    sequence = max(time.time_ns() // 1_000_000, max((r['sequence'] for r in previous.values()), default=0) + 1)
    result = {}
    if set(releases) != set(TARGETS):
        raise ValueError('All supported targets must be published together')
    if len({r['asset']['Version'] for r in releases.values()}) != 1:
        raise ValueError('Target versions differ')
    for target, release in releases.items():
        if target in previous:
            old = previous[target]['asset']
            new = release['asset']
            if version_tuple(new['Version']) < version_tuple(old['Version']):
                raise ValueError('Refusing to roll back the update feed')
            if new['Version'] == old['Version'] and new != old:
                raise ValueError('Published versions are immutable; increment Cargo version')
        result[target] = sign(dict(release, sequence=sequence,
            published_at=(now - timedelta(minutes=1)).isoformat(),
            expires_at=(now + timedelta(days=90)).isoformat()), key)
    return result


def advance_feed(head, signed):
    tree = []
    for target, envelope in signed.items():
        blob = api('git/blobs', {'content': json.dumps(envelope, ensure_ascii=False, indent=2) + '\n', 'encoding': 'utf-8'})
        tree.append({'path': f'{CONFIG["channel"]}/{target}.json', 'mode': '100644', 'type': 'blob', 'sha': blob['sha']})
    body = {'tree': tree}
    if head:
        body['base_tree'] = api(f'git/commits/{head}')['tree']['sha']
    tree_sha = api('git/trees', body)['sha']
    commit = api('git/commits', {'message': 'Refresh signed application update feed',
                               'tree': tree_sha, 'parents': [head] if head else []})
    if head:
        api('git/refs/heads/updates', {'sha': commit['sha'], 'force': False}, method='PATCH')
    else:
        api('git/refs', {'ref': 'refs/heads/updates', 'sha': commit['sha']})
    print(f'All {len(signed)} signed feeds advanced atomically: {commit["sha"]}')


def from_artifacts(directory, version):
    releases = {}
    for target in TARGETS:
        metadata = json.loads((directory / f'{target}-build.json').read_text())
        if (metadata['target'], metadata['version'], metadata['channel']) != (target, version, CONFIG['channel']):
            raise ValueError('Build metadata does not match this release')
        assets = [a for a in metadata['Assets'] if a['Type'] == 'Full']
        if len(assets) != 1:
            raise ValueError('Expected exactly one full update package')
        asset = assets[0]
        release = dict(protocol=1, channel=CONFIG['channel'], sequence=1, target=target, asset=asset,
                       package_url=f'https://github.com/{CONFIG["repository"]}/releases/download/v{version}/{asset["FileName"]}')
        validate_release(release)
        if asset['Version'] != version:
            raise ValueError('Package version mismatch')
        path = directory / asset['FileName']
        with path.open('rb') as file:
            digest = hashlib.file_digest(file, 'sha256').hexdigest()
        if path.stat().st_size != asset['Size'] or digest.lower() != asset['SHA256'].lower():
            raise ValueError('Build package failed integrity validation')
        releases[target] = release
    return releases


def published_release(tag):
    release = api(f'releases/tags/{tag}', missing=True)
    return release if release and not release['draft'] else None


def from_published(tag, release):
    with tempfile.TemporaryDirectory() as tmp:
        run('gh', 'release', 'download', tag, '--repo', CONFIG['repository'], '--pattern', '*-signed.json', '--dir', tmp)
        releases = {target: verify_envelope(json.loads((Path(tmp) / f'{target}-signed.json').read_text())) for target in TARGETS}
    assets = {a['name']: a['size'] for a in release['assets']}
    for target, record in releases.items():
        validate_tag(tag, record['asset']['Version'])
        if record['target'] != target or assets.get(record['asset']['FileName']) != record['asset']['Size']:
            raise ValueError('Published package missing or size changed')
    return releases


def publish(directory, version, head, previous, key):
    tag = 'v' + version
    existing = published_release(tag)
    if existing:
        # A retry after publishing must never replace public binaries or envelopes.
        signed = envelopes(from_published(tag, existing), previous, key)
        advance_feed(head, signed)
        return
    signed = envelopes(from_artifacts(directory, version), previous, key)
    for target, envelope in signed.items():
        (directory / f'{target}-signed.json').write_text(json.dumps(envelope, ensure_ascii=False, indent=2) + '\n')
    signatures = {f'{target}-signed.json' for target in TARGETS}
    prefixes = tuple(f'{target}-' for target in TARGETS)
    files = sorted(p for p in directory.iterdir() if p.is_file() and (
        p.name in signatures
        or (p.name.startswith(prefixes) and p.suffix.lower() in ('.exe', '.pkg', '.zip', '.nupkg'))
    ))
    sums = []
    for path in files:
        with path.open('rb') as file:
            sums.append(f'{hashlib.file_digest(file, "sha256").hexdigest()}  {path.name}')
    checksums = directory / 'SHA256SUMS'
    checksums.write_text('\n'.join(sums) + '\n')
    files.append(checksums)
    if api(f'releases/tags/{tag}', missing=True) is None:
        notes = directory / 'release-notes.md'
        notes.write_text(('PoE Toolkit 首个内测版本。' if version == '0.1.0' else f'PoE Toolkit {version} 内测版本。') + '''\n\n- PoE1 / PoE2 工作区与客户端自动识别。\n- 国服、国际服行情及物品价格标注。\n- 补丁预览、应用、撤销和备份恢复。\n- Windows / macOS 系统材质、深浅色与托盘。\n- 应用内检查更新、下载与重启安装。\n\n### 安装\n\n- Windows：下载 `windows-x86_64-…-Setup.exe`。\n- Apple Silicon：下载 `macos-aarch64-….pkg`。\n- Intel Mac：下载 `macos-x86_64-….pkg`。\n- ZIP 为便携包，请完整解压后运行。\n\n首次使用请退出游戏，选择客户端与赛季，刷新行情并预览变更后应用。\n''')
        run('gh', 'release', 'create', tag, '--repo', CONFIG['repository'], '--verify-tag', '--draft', '--prerelease',
            '--title', f'PoE Toolkit {version} · {"Beta 1" if version == "0.1.0" else "Beta"}', '--notes-file', str(notes))
    run('gh', 'release', 'upload', tag, '--repo', CONFIG['repository'], '--clobber', *map(str, files))
    run('gh', 'release', 'edit', tag, '--repo', CONFIG['repository'], '--draft=false')
    # Point users to it only after every installer and signed package is public.
    advance_feed(head, signed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('validate', 'publish', 'refresh'))
    parser.add_argument('--artifacts', type=Path, default=ROOT / 'dist/release')
    args = parser.parse_args()
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    if args.mode != 'refresh':
        validate_tag(os.environ.get('GITHUB_REF_NAME', 'v' + version), version)
    if os.environ.get('GITHUB_REPOSITORY', CONFIG['repository']) != CONFIG['repository']:
        raise ValueError('Official publication is restricted to the configured repository')
    if args.mode == 'validate':
        print(f'Validated release v{version}')
        return
    head, previous = feed_state()
    if args.mode == 'refresh' and not previous:
        print('No release feed yet; nothing to renew')
        return
    secret = os.environ.pop('APP_UPDATE_SIGNING_KEY', '')
    if not secret:
        raise ValueError('APP_UPDATE_SIGNING_KEY GitHub secret is not configured')
    with tempfile.TemporaryDirectory() as tmp:
        key = Path(tmp) / 'signing.pem'
        fd = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'w') as file:
            file.write(secret)
        if args.mode == 'publish':
            publish(args.artifacts, version, head, previous, key)
        else:
            versions = {r['asset']['Version'] for r in previous.values()}
            if len(versions) != 1:
                raise ValueError('Cannot renew a mixed-version feed')
            tag = 'v' + versions.pop()
            release = published_release(tag)
            if release is None:
                raise ValueError('Cannot renew an unpublished or removed release')
            advance_feed(head, envelopes(from_published(tag, release), previous, key))


if __name__ == '__main__':
    main()
