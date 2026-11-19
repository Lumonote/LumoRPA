#!/usr/bin/env python3
"""Stage verified releases, then publish with recoverable channel promotion."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import quote

API = 'repos/{owner}/{repo}'
METADATA = re.compile(r'<!-- lumo-release:(.*?) -->')


def gh(*args, check=True):
    return subprocess.run(['gh', *args], text=True, capture_output=True, check=check)


def api(path, missing_ok=False):
    result = gh('api', path, check=False)
    if result.returncode:
        if missing_ok and 'HTTP 404' in result.stderr:
            return None
        raise subprocess.CalledProcessError(result.returncode, result.args, result.stdout, result.stderr)
    return json.loads(result.stdout)


def release(tag):
    current = api(f'{API}/releases/tags/{quote(tag, safe="")}', missing_ok=True)
    if current:
        return current
    # The tag endpoint only finds published releases. Authenticated release
    # listings also contain drafts, including drafts left by a failed publish.
    page = 1
    while True:
        releases = api(f'{API}/releases?per_page=100&page={page}')
        for candidate in releases:
            if candidate['tag_name'] == tag:
                return candidate
        if len(releases) < 100:
            return None
        page += 1


def version_key(tag):
    match = re.fullmatch(r'v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z.-]+))?', tag)
    if not match:
        raise ValueError(f'invalid release version: {tag}')
    prerelease = match[4]
    identifiers = []
    if prerelease:
        for part in prerelease.split('.'):
            if not part or (part.isdigit() and len(part) > 1 and part.startswith('0')):
                raise ValueError(f'invalid prerelease version: {tag}')
            identifiers.append((0, int(part)) if part.isdigit() else (1, part))
    return (*map(int, match.group(1, 2, 3)), 0 if prerelease else 1, tuple(identifiers))


def metadata(info):
    match = METADATA.search(info.get('body') or '')
    return json.loads(match[1]) if match else None


def digest(path):
    with path.open('rb') as stream:
        return 'sha256:' + hashlib.file_digest(stream, 'sha256').hexdigest()


def download(tag, name, directory):
    target = Path(directory, name)
    gh('release', 'download', tag, '--pattern', name, '--output', str(target))
    return target


def verify_assets(tag, paths, info):
    expected = {path.name: path for path in paths}
    actual = {asset['name']: asset for asset in info['assets']}
    if set(actual) != set(expected):
        raise ValueError(f'{tag}: uploaded release is incomplete or has unexpected assets')
    for name, path in expected.items():
        asset = actual[name]
        if asset['size'] != path.stat().st_size:
            raise ValueError(f'{tag}: uploaded asset size mismatch: {name}')
        remote_digest = asset.get('digest')
        if not remote_digest:
            # Older GitHub assets lack a digest; verify downloaded bytes instead.
            with tempfile.TemporaryDirectory(prefix='lumo-release-verify-') as directory:
                remote_digest = digest(download(tag, name, directory))
        if remote_digest != digest(path):
            raise ValueError(f'{tag}: uploaded asset content mismatch: {name}')


def verify_tag(tag, sha):
    commit = api(f'{API}/commits/{quote(tag, safe="")}')
    if commit['sha'] != sha:
        raise ValueError(f'{tag}: Git tag commit differs from the validated artifacts')


def move_rolling_tag(tag, sha):
    reference = f'{API}/git/refs/tags/{tag}'
    if api(reference, missing_ok=True) is None:
        gh('api', '--method', 'POST', f'{API}/git/refs', '-f', f'ref=refs/tags/{tag}', '-f', f'sha={sha}')
    else:
        gh('api', '--method', 'PATCH', reference, '-f', f'sha={sha}', '-F', 'force=true')
    verify_tag(tag, sha)


def stage(tag, sha, paths, notes, *, rolling=False, prerelease=False, source_tag=None):
    if rolling and tag not in {'dev-latest', 'beta-latest'}:
        raise ValueError('unsupported rolling release tag')
    if not rolling:
        verify_tag(tag, sha)
    identity = {'commit': sha, 'source_tag': source_tag or tag}
    current = release(tag)
    if current and not rolling and metadata(current) not in (None, identity):
        raise ValueError(f'{tag}: recorded version/commit differs from the validated artifacts')
    if current and not current['draft']:
        if not rolling or metadata(current) == identity:
            # A failed channel publish may retry after the immutable version was
            # made public. Verify exact tag and bytes, without mutating it.
            verify_tag(tag, sha)
            verify_assets(tag, paths, current)
            return False
        gh('release', 'edit', tag, '--draft=true')
    elif not current:
        gh('release', 'create', tag, '--draft', '--target', sha, '--title', f'LumoRPA {tag}', '--notes', '')
    body_text = notes + '\n<!-- lumo-release:' + json.dumps(identity, sort_keys=True) + ' -->\n'
    with tempfile.TemporaryDirectory(prefix='lumo-release-notes-') as directory:
        body = Path(directory, 'notes.md'); body.write_text(body_text)
        gh('release', 'edit', tag, '--notes-file', str(body), f'--prerelease={str(prerelease).lower()}')
    gh('release', 'upload', tag, *map(str, paths), '--clobber')
    current = release(tag)
    expected = {path.name for path in paths}
    for asset in current['assets']:
        if asset['name'] not in expected:
            gh('release', 'delete-asset', tag, asset['name'], '--yes')
    verify_assets(tag, paths, release(tag))
    if rolling:
        move_rolling_tag(tag, sha)
    return True


def make_public(tag, latest=False):
    gh('release', 'edit', tag, '--draft=false', f'--latest={str(latest).lower()}')


def should_promote_beta(tag, sha):
    current = release('beta-latest')
    if not current:
        return True
    identity = metadata(current)
    if identity:
        current_tag = identity['source_tag']
    elif any(asset['name'] == 'latest.json' for asset in current['assets']):
        with tempfile.TemporaryDirectory(prefix='lumo-beta-version-') as directory:
            current_tag = json.loads(download('beta-latest', 'latest.json', directory).read_text())['version']
    elif current['draft']:
        return True  # A newly created draft may have failed before its first upload.
    else:
        raise ValueError('published beta channel has no verifiable version')
    incoming, previous = version_key(tag), version_key(current_tag)
    if incoming == previous:
        if identity and identity['commit'] != sha:
            raise ValueError('beta version already belongs to a different commit')
        if not current['draft']:
            verify_tag('beta-latest', sha)
    return incoming >= previous


def should_promote_stable(tag):
    current = api(f'{API}/releases/latest', missing_ok=True)
    return current is None or version_key(tag) >= version_key(current['tag_name'])


def publish_set(tag, sha, paths):
    rolling = tag == 'dev-latest'
    beta = not rolling and '-' in tag
    if not rolling:
        version_key(tag)
        feed = next((path for path in paths if path.name == 'latest.json'), None)
        if feed and json.loads(feed.read_text()).get('version') != tag.removeprefix('v'):
            raise ValueError('update manifest version differs from release tag')
    notes = f'Commit: `{sha}`\n\n'
    notes += ('Unofficial, unsigned development snapshot. No automatic update feed is published.\n' if rolling else
              'All supported desktop platforms passed the same-commit release gates. macOS signing and notarization were verified.\n')
    promote_beta = beta and should_promote_beta(tag, sha)
    promote_stable = not rolling and not beta and should_promote_stable(tag)
    version_staged = stage(tag, sha, paths, notes, rolling=rolling, prerelease=rolling or beta)
    feed_staged = False
    if promote_beta:
        feed = next((path for path in paths if path.name == 'latest.json'), None)
        if not feed:
            raise ValueError('beta release requires latest.json')
        # Complete both uploads and both tags before either draft becomes public.
        feed_staged = stage('beta-latest', sha, [feed], f'Beta update feed for {tag}; commit `{sha}`.\n',
                            rolling=True, prerelease=True, source_tag=tag)
    if version_staged:
        make_public(tag, latest=promote_stable)
    if feed_staged:
        make_public('beta-latest')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--tag', required=True); parser.add_argument('--sha', required=True)
    args = parser.parse_args()
    paths = sorted(path for path in args.assets.iterdir() if path.is_file())
    if not paths or not (args.assets / 'SHA256SUMS').is_file():
        parser.error('validated artifact set and SHA256SUMS are required')
    publish_set(args.tag, args.sha, paths)


if __name__ == '__main__': main()
