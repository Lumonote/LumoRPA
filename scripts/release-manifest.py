#!/usr/bin/env python3
"""Validate one commit's installer set and assemble a Tauri static update feed."""
import argparse
import base64
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib
from urllib.parse import quote

REQUIRED = {
    'macos-universal': ['*.dmg', '*.app.tar.gz', 'lumorpa-*-darwin-universal.tar.gz'],
    'windows-x86_64': ['*.exe', '*.msi', 'lumorpa-*-windows-*.zip'],
    'linux-x86_64': ['*.deb', '*.rpm', '*.AppImage', 'lumorpa-*-linux-x86_64.tar.gz'],
    'linux-aarch64': ['*.deb', '*.rpm', '*.AppImage', 'lumorpa-*-linux-aarch64.tar.gz'],
}
UPDATE_PAYLOAD = {
    'macos-universal': ('*.app.tar.gz', ['darwin-x86_64', 'darwin-aarch64']),
    'windows-x86_64': ('*.exe', ['windows-x86_64']),
    'linux-x86_64': ('*.AppImage', ['linux-x86_64']),
    'linux-aarch64': ('*.AppImage', ['linux-aarch64']),
}


def version_at(root):
    cargo = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    tauri = json.loads((root / 'apps/desktop/src-tauri/tauri.conf.json').read_text())['version']
    if cargo != tauri:
        raise ValueError(f'Cargo version {cargo} differs from Tauri version {tauri}')
    return cargo


def validate_version(tag, root, require_keys=True):
    version = version_at(root)
    if not re.fullmatch(r'v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', tag) or tag[1:] != version:
        raise ValueError(f'tag {tag!r} must equal v{version}; update Cargo and Tauri versions before tagging')
    if require_keys:
        for name in ['LUMO_UPDATER_PUBKEY', 'TAURI_SIGNING_PRIVATE_KEY']:
            if not os.environ.get(name, '').strip():
                raise ValueError(f'{name} is required for a tagged release')
    return version


def file_digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def stamp(directory, platform, sha, version):
    files = {p.name: {'bytes': p.stat().st_size, 'sha256': file_digest(p)}
             for p in sorted(directory.iterdir()) if p.is_file() and not p.name.startswith('provenance-')}
    if not files:
        raise ValueError(f'{platform}: no artifacts to stamp')
    (directory / f'provenance-{platform}.json').write_text(json.dumps({
        'platform': platform, 'commit': sha, 'version': version, 'files': files,
    }, indent=2) + '\n')


def signature_of(payload, public_key):
    signature = payload.with_name(payload.name + '.sig')
    if not signature.is_file() or signature.stat().st_size > 16384:
        raise ValueError(f'missing or oversized updater signature: {signature.name}')
    text = signature.read_text().strip()
    try:
        decoded = base64.b64decode(text, validate=True).decode('utf-8')
        lines = decoded.splitlines()
        if len(lines) < 4 or not lines[0].startswith('untrusted comment:') or not lines[2].startswith('trusted comment:'):
            raise ValueError('invalid minisign envelope')
        if len(base64.b64decode(lines[1], validate=True)) != 74 or len(base64.b64decode(lines[3], validate=True)) != 64:
            raise ValueError('invalid minisign signature length')
    except (ValueError, UnicodeError) as error:
        raise ValueError(f'invalid updater signature envelope: {signature.name}') from error
    try:
        key = base64.b64decode(public_key.strip(), validate=True).decode('utf-8')
    except (ValueError, UnicodeError) as error:
        raise ValueError('invalid updater public key') from error
    # Verify against the same public key embedded in the installer. An envelope
    # alone cannot detect a mismatched signing key or a modified payload.
    with tempfile.TemporaryDirectory(prefix='lumo-update-signature-') as temporary:
        key_path = Path(temporary, 'key.pub'); key_path.write_text(key)
        sig_path = Path(temporary, 'payload.minisig'); sig_path.write_text(decoded)
        subprocess.run(['minisign', '-Vm', str(payload), '-p', str(key_path), '-x', str(sig_path)],
                       check=True, capture_output=True, text=True)
    return text


def assemble(artifacts, output, tag, repository, sha, public_key=''):
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise ValueError('repository must be owner/name')
    tagged = tag != 'dev-latest'
    if tagged and not re.fullmatch(r'v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', tag):
        raise ValueError('invalid version tag')
    versions, files, platforms = set(), {}, {}
    for platform, patterns in REQUIRED.items():
        directory = artifacts / f'lumorpa-{platform}'
        provenance = directory / f'provenance-{platform}.json'
        data = json.loads(provenance.read_text())
        if data.get('commit') != sha or data.get('platform') != platform:
            raise ValueError(f'{platform}: artifact commit/platform does not match the release')
        versions.add(data['version'])
        declared = data.get('files', {})
        actual = {p.name for p in directory.iterdir() if p.is_file() and p != provenance}
        if set(declared) != actual:
            raise ValueError(f'{platform}: artifact set differs from provenance')
        for name, entry in declared.items():
            if Path(name).name != name:
                raise ValueError(f'{platform}: invalid artifact filename')
            path = directory / name
            if path.stat().st_size != entry['bytes'] or not entry['bytes'] or file_digest(path) != entry['sha256']:
                raise ValueError(f'{platform}: changed or empty artifact {name}')
        for pattern in patterns:
            # Unsigned development builds intentionally omit updater tarballs.
            if not tagged and pattern == '*.app.tar.gz':
                continue
            if not list(directory.glob(pattern)):
                raise ValueError(f'{platform}: missing required {pattern}')
        if tagged:
            pattern, targets = UPDATE_PAYLOAD[platform]
            payloads = list(directory.glob(pattern))
            if len(payloads) != 1:
                raise ValueError(f'{platform}: expected exactly one update payload {pattern}')
            payload = payloads[0]
            entry = {'signature': signature_of(payload, public_key),
                     'url': f'https://github.com/{repository}/releases/download/{quote(tag, safe="")}/{quote(payload.name, safe="")}' }
            platforms.update({target: entry for target in targets})
        for path in directory.iterdir():
            if path.is_file():
                if path.name in files:
                    raise ValueError(f'colliding platform artifact name: {path.name}')
                files[path.name] = path
    if len(versions) != 1 or (tagged and tag[1:] not in versions):
        raise ValueError(f'artifact versions {versions} do not match tag {tag}')
    optional = artifacts / 'lumorpa-linux-loongarch64'
    if optional.exists():
        provenance = optional / 'provenance-linux-loongarch64.json'
        record = json.loads(provenance.read_text())
        if record.get('commit') != sha or record.get('platform') != 'linux-loongarch64' or record.get('version') not in versions:
            raise ValueError('optional platform provenance differs from required platforms')
        actual = {p.name for p in optional.iterdir() if p.is_file() and p != provenance}
        if set(record.get('files', {})) != actual:
            raise ValueError('optional artifact set differs from provenance')
        for name, entry in record['files'].items():
            path = optional / name
            if Path(name).name != name or path.stat().st_size != entry['bytes'] or not entry['bytes'] or file_digest(path) != entry['sha256']:
                raise ValueError('optional artifact content differs from provenance')
        for path in optional.iterdir():
            if path.is_file():
                if path.name in files:
                    raise ValueError(f'colliding optional artifact name: {path.name}')
                files[path.name] = path
    output.mkdir(parents=True, exist_ok=False)
    for name, path in files.items():
        shutil.copy2(path, output / name)
    if tagged:
        # Keep the feed and SHA256SUMS identical when retrying publication of the
        # same artifacts. CI supplies the validated commit's timestamp.
        timestamp = os.environ.get('SOURCE_DATE_EPOCH')
        published = (datetime.datetime.fromtimestamp(int(timestamp), datetime.timezone.utc)
                     if timestamp is not None else datetime.datetime.now(datetime.timezone.utc))
        manifest = {'version': tag[1:], 'notes': f'LumoRPA {tag} ({sha[:12]})',
                    'pub_date': published.isoformat().replace('+00:00', 'Z'),
                    'platforms': platforms}
        (output / 'latest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (output / 'SHA256SUMS').write_text(''.join(f'{file_digest(path)}  {path.name}\n'
        for path in sorted(output.iterdir()) if path.is_file()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    check = sub.add_parser('validate-version'); check.add_argument('--tag', required=True)
    record = sub.add_parser('stamp'); record.add_argument('--directory', type=Path, default=Path('out'))
    record.add_argument('--platform', required=True)
    merge = sub.add_parser('assemble')
    merge.add_argument('--artifacts', type=Path, required=True); merge.add_argument('--output', type=Path, required=True)
    merge.add_argument('--tag', required=True); merge.add_argument('--repository', required=True); merge.add_argument('--sha', required=True)
    args = parser.parse_args(); root = Path(__file__).resolve().parent.parent
    try:
        if args.command == 'validate-version': validate_version(args.tag, root)
        elif args.command == 'stamp': stamp(args.directory, args.platform, os.environ['GITHUB_SHA'], version_at(root))
        else: assemble(args.artifacts, args.output, args.tag, args.repository, args.sha, os.environ.get('LUMO_UPDATER_PUBKEY', ''))
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'release validation failed: {error}\n')


if __name__ == '__main__': main()
