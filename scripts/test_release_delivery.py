"""Release failures must leave no public, incomplete or cross-commit install set."""
import base64
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


manifest = load('release-manifest')
publisher = load('publish-release')
SHA = 'a' * 40
SIGNATURE = base64.b64encode(('untrusted comment: fixture\n' + base64.b64encode(b'x' * 74).decode()
    + '\ntrusted comment: fixture\n' + base64.b64encode(b'y' * 64).decode() + '\n').encode()).decode()
PUBLIC_KEY = base64.b64encode(b'untrusted comment: fixture\nkey\n').decode()


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.artifacts = self.root / 'artifacts'
        self.output = self.root / 'output'
        for platform, patterns in manifest.REQUIRED.items():
            directory = self.artifacts / f'lumorpa-{platform}'
            directory.mkdir(parents=True)
            for pattern in patterns:
                path = directory / pattern.replace('*', platform + '-0.1.0')
                path.write_bytes(b'installer fixture ' + platform.encode())
                path.with_name(path.name + '.sig').write_text(SIGNATURE)
            manifest.stamp(directory, platform, SHA, '0.1.0')

    def assemble(self):
        with patch.object(manifest.subprocess, 'run') as verify:
            manifest.assemble(self.artifacts, self.output, 'v0.1.0', 'test/LumoRPA', SHA, PUBLIC_KEY)
            return verify

    def test_all_targets_use_verified_payloads_and_immutable_urls(self):
        verify = self.assemble()
        self.assertEqual(verify.call_count, 4)
        for call in verify.call_args_list:
            self.assertEqual(call.args[0][:2], ['minisign', '-Vm'])
            self.assertTrue(call.kwargs['check'])
        feed = json.loads((self.output / 'latest.json').read_text())
        self.assertEqual(set(feed['platforms']), {'darwin-x86_64', 'darwin-aarch64', 'windows-x86_64', 'linux-x86_64', 'linux-aarch64'})
        self.assertTrue(all('/download/v0.1.0/' in item['url'] for item in feed['platforms'].values()))
        checksums = (self.output / 'SHA256SUMS').read_text()
        self.assertIn(manifest.file_digest(self.output / 'latest.json') + '  latest.json', checksums)

    def test_wrong_commit_or_changed_payload_rejects_without_output(self):
        directory = self.artifacts / 'lumorpa-linux-x86_64'
        with self.subTest('commit'):
            manifest.stamp(directory, 'linux-x86_64', 'b' * 40, '0.1.0')
            with self.assertRaisesRegex(ValueError, 'commit/platform'):
                self.assemble()
            self.assertFalse(self.output.exists())
        manifest.stamp(directory, 'linux-x86_64', SHA, '0.1.0')
        with self.subTest('tamper'):
            next(directory.glob('*.AppImage')).write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'changed or empty'):
                self.assemble()
            self.assertFalse(self.output.exists())

    def test_missing_platform_and_missing_signature_reject(self):
        directory = self.artifacts / 'lumorpa-windows-x86_64'
        provenance = directory / 'provenance-windows-x86_64.json'
        provenance.unlink()
        with self.assertRaises(FileNotFoundError):
            self.assemble()
        next(directory.glob('*.exe.sig')).unlink()
        manifest.stamp(directory, 'windows-x86_64', SHA, '0.1.0')
        with self.assertRaisesRegex(ValueError, 'signature'):
            self.assemble()
        self.assertFalse(self.output.exists())

    def test_wrong_signature_or_version_rejects(self):
        with patch.object(manifest.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'minisign')):
            with self.assertRaises(subprocess.CalledProcessError):
                manifest.assemble(self.artifacts, self.output, 'v0.1.0', 'test/LumoRPA', SHA, PUBLIC_KEY)
        self.assertFalse(self.output.exists())
        manifest.stamp(self.artifacts / 'lumorpa-linux-x86_64', 'linux-x86_64', SHA, '0.2.0')
        with self.assertRaisesRegex(ValueError, 'versions'):
            self.assemble()

    def test_optional_platform_must_match_commit_too(self):
        optional = self.artifacts / 'lumorpa-linux-loongarch64'
        optional.mkdir()
        (optional / 'loongarch64.tar.gz').write_bytes(b'cli')
        manifest.stamp(optional, 'linux-loongarch64', 'b' * 40, '0.1.0')
        with self.assertRaisesRegex(ValueError, 'optional platform provenance'):
            self.assemble()

    def test_dev_snapshot_has_no_update_feed_or_signing_dependency(self):
        with patch.object(manifest.subprocess, 'run', side_effect=AssertionError('must not verify unsigned snapshots')):
            manifest.assemble(self.artifacts, self.output, 'dev-latest', 'test/LumoRPA', SHA)
        self.assertFalse((self.output / 'latest.json').exists())
        self.assertTrue((self.output / 'SHA256SUMS').is_file())

    def test_commit_timestamp_keeps_manifest_and_checksums_identical_on_retry(self):
        with patch.dict(os.environ, {'SOURCE_DATE_EPOCH': '1795123456'}):
            self.assemble()
            feed = (self.output / 'latest.json').read_bytes()
            checksums = (self.output / 'SHA256SUMS').read_bytes()
            shutil.rmtree(self.output)
            self.assemble()
        self.assertEqual((self.output / 'latest.json').read_bytes(), feed)
        self.assertEqual((self.output / 'SHA256SUMS').read_bytes(), checksums)

    def test_tag_must_match_both_application_version_sources(self):
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0"\n')
        config = self.root / 'apps/desktop/src-tauri/tauri.conf.json'
        config.parent.mkdir(parents=True)
        config.write_text('{"version":"0.1.0"}')
        with self.assertRaisesRegex(ValueError, 'must equal'):
            manifest.validate_version('v0.2.0', self.root, require_keys=False)
        config.write_text('{"version":"0.2.0"}')
        with self.assertRaisesRegex(ValueError, 'differs'):
            manifest.validate_version('v0.1.0', self.root, require_keys=False)


class PackageDefaultsTests(unittest.TestCase):
    def test_tag_installer_gets_defaults_and_dev_clears_signing_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            scripts = Path(temporary, 'scripts')
            scripts.mkdir()
            wrapper = scripts / 'package-ci-release.sh'
            shutil.copyfile(Path(__file__).with_name(wrapper.name), wrapper)
            child = scripts / 'package-release.sh'
            child.write_text('#!/bin/sh\nexec /usr/bin/env\n')
            child.chmod(0o700)
            environment = {**os.environ, 'GITHUB_REF': 'refs/tags/v0.1.0', 'GITHUB_REF_NAME': 'v0.1.0',
                           'GITHUB_REPOSITORY': 'example/LumoRPA', 'LUMO_UPDATER_PUBKEY': 'public-fixture',
                           'TAURI_SIGNING_PRIVATE_KEY': 'private-fixture'}
            for name in ['LUMO_UPDATE_ENDPOINT', 'LUMO_UPDATE_BETA_ENDPOINT', 'LUMO_UPDATE_CHANNEL']:
                environment.pop(name, None)
            def config():
                output = subprocess.run(['bash', str(wrapper)], env=environment, text=True, capture_output=True, check=True).stdout
                return dict(line.split('=', 1) for line in output.splitlines() if '=' in line)
            tagged = config()
            self.assertEqual(tagged['LUMO_UPDATE_CHANNEL'], 'stable')
            self.assertEqual(tagged['LUMO_UPDATE_ENDPOINT'], 'https://github.com/example/LumoRPA/releases/latest/download/latest.json')
            self.assertEqual(tagged['LUMO_UPDATE_BETA_ENDPOINT'], 'https://github.com/example/LumoRPA/releases/download/beta-latest/latest.json')
            environment.update(GITHUB_REF='refs/tags/v0.1.0-beta.1', GITHUB_REF_NAME='v0.1.0-beta.1')
            self.assertEqual(config()['LUMO_UPDATE_CHANNEL'], 'beta')
            environment['GITHUB_REF'] = 'refs/heads/main'
            development = config()
            self.assertNotIn('LUMO_UPDATER_PUBKEY', development)
            self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY', development)
            self.assertNotIn('LUMO_UPDATE_ENDPOINT', development)


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.path = self.root / 'installer.exe'
        self.path.write_bytes(b'installer')
        self.feed = self.root / 'latest.json'
        self.feed.write_text('{"version":"0.1.0-beta.1"}')
        self.commands, self.releases, self.tags = [], {}, {}
        self.latest = None
        self.fail = lambda args: False
        self.bad_assets = None
        self.patch = patch.object(publisher, 'gh', side_effect=self.gh)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def gh(self, *args, check=True):
        self.commands.append(args)
        if self.fail(args):
            raise subprocess.CalledProcessError(1, args, 'simulated failure')
        data = {}
        if args[0] == 'api':
            if '--method' in args:
                endpoint = args[3]
                values = dict(args[index + 1].split('=', 1) for index, value in enumerate(args) if value in {'-f', '-F'})
                tag = endpoint.rsplit('/', 1)[-1] if args[2] == 'PATCH' else values['ref'].removeprefix('refs/tags/')
                self.tags[tag] = values['sha']
            elif '/commits/' in args[1]:
                sha = self.tags.get(args[1].split('/commits/')[1])
                if not sha: return subprocess.CompletedProcess(args, 1, '', 'HTTP 404: Not Found')
                data = {'sha': sha}
            elif '/git/refs/tags/' in args[1]:
                tag = args[1].split('/git/refs/tags/')[1]
                if tag not in self.tags: return subprocess.CompletedProcess(args, 1, '', 'HTTP 404: Not Found')
                data = {'object': {'sha': self.tags[tag]}}
            elif '/releases?per_page=100&page=' in args[1]:
                page = int(args[1].rsplit('=', 1)[1])
                data = [self.public_info(info) for info in list(self.releases.values())[(page - 1) * 100:page * 100]]
            else:
                tag = self.latest if args[1].endswith('/releases/latest') else args[1].split('/releases/tags/')[1]
                if tag not in self.releases: return subprocess.CompletedProcess(args, 1, '', 'HTTP 404: Not Found')
                info = self.releases[tag]
                if info['draft']: return subprocess.CompletedProcess(args, 1, '', 'HTTP 404: Not Found')
                data = self.public_info(info)
        elif args[:2] == ('release', 'create'):
            self.releases[args[2]] = {'draft': True, 'body': '', 'tag_name': args[2], 'assets': []}
        elif args[:2] == ('release', 'edit'):
            info = self.releases[args[2]]
            if '--draft=true' in args: info['draft'] = True
            if '--draft=false' in args: info['draft'] = False
            if '--latest=true' in args: self.latest = args[2]
            if '--notes-file' in args: info['body'] = Path(args[args.index('--notes-file') + 1]).read_text()
        elif args[:2] == ('release', 'upload'):
            assets = [{'name': Path(value).name, 'size': Path(value).stat().st_size,
                       'digest': publisher.digest(Path(value)), 'bytes': Path(value).read_bytes()}
                      for value in args[3:] if not value.startswith('--')]
            self.releases[args[2]]['assets'] = self.bad_assets if self.bad_assets is not None else assets
        elif args[:2] == ('release', 'delete-asset'):
            info = self.releases[args[2]]
            info['assets'] = [asset for asset in info['assets'] if asset['name'] != args[3]]
        elif args[:2] == ('release', 'download'):
            name = args[args.index('--pattern') + 1]
            asset = next(asset for asset in self.releases[args[2]]['assets'] if asset['name'] == name)
            Path(args[args.index('--output') + 1]).write_bytes(asset['bytes'])
        else:
            raise AssertionError(args)
        return subprocess.CompletedProcess(args, 0, json.dumps(data), '')

    @staticmethod
    def public_info(info):
        return {**info, 'assets': [{key: value for key, value in asset.items() if key != 'bytes'} for asset in info['assets']]}

    def publish(self, tag='v0.1.0', sha=SHA):
        if tag != 'dev-latest': self.tags.setdefault(tag, sha)
        if tag != 'dev-latest': self.feed.write_text(json.dumps({'version': tag.removeprefix('v')}))
        paths = [self.path, self.feed] if '-' in tag and tag != 'dev-latest' else [self.path]
        publisher.publish_set(tag, sha, paths)

    def fail_once(self, predicate):
        fired = False
        def fail(args):
            nonlocal fired
            if not fired and predicate(args):
                fired = True
                return True
            return False
        self.fail = fail

    def mutations(self, since=0):
        return [command for command in self.commands[since:] if command[0] != 'api' or '--method' in command]

    def test_only_complete_upload_is_made_public(self):
        self.publish()
        self.assertEqual(self.commands[-1], ('release', 'edit', 'v0.1.0', '--draft=false', '--latest=true'))
        self.assertLess(next(i for i, command in enumerate(self.commands) if command[:2] == ('release', 'upload')), len(self.commands) - 1)

    def test_missing_wrong_sized_or_wrong_digest_upload_never_becomes_public(self):
        for assets in [[], [{'name': self.path.name, 'size': 1}],
                       [{'name': self.path.name, 'size': self.path.stat().st_size, 'digest': 'sha256:wrong'}]]:
            with self.subTest(assets=assets):
                self.bad_assets = assets
                self.commands.clear()
                with self.assertRaises(ValueError): self.publish()
                self.assertFalse(any('--draft=false' in command for command in self.commands))

    def test_upload_failure_keeps_draft(self):
        self.fail_once(lambda args: args[:2] == ('release', 'upload'))
        with self.assertRaises(subprocess.CalledProcessError): self.publish()
        self.assertTrue(self.releases['v0.1.0']['draft'])

    def test_draft_lookup_searches_later_pages_and_reuses_existing_release(self):
        for index in range(100):
            tag = f'v0.0.{index}'
            self.releases[tag] = {'draft': False, 'body': '', 'tag_name': tag, 'assets': []}
        self.releases['v0.1.0'] = {'draft': True, 'body': None, 'tag_name': 'v0.1.0', 'assets': []}
        self.publish()
        self.assertFalse(self.releases['v0.1.0']['draft'])
        self.assertFalse(any(command[:2] == ('release', 'create') for command in self.commands))
        self.assertTrue(any(command[0] == 'api' and command[1].endswith('page=2') for command in self.commands))

    def test_draft_lookup_failure_fails_closed_before_mutation(self):
        self.fail_once(lambda args: args[0] == 'api' and '/releases?' in args[1])
        with self.assertRaises(subprocess.CalledProcessError): self.publish()
        self.assertEqual(self.mutations(), [])

    def test_legacy_beta_null_body_uses_manifest_version(self):
        self.publish('v0.1.0-beta.10', 'b' * 40)
        self.releases['beta-latest']['body'] = None
        self.publish('v0.1.0-beta.9')
        self.assertEqual(self.tags['beta-latest'], 'b' * 40)

    def test_same_published_version_is_verified_without_mutation(self):
        self.publish()
        start = len(self.commands)
        self.publish()
        self.assertEqual(self.mutations(start), [])

    def test_published_version_rejects_different_bytes_even_at_same_size(self):
        self.publish()
        self.path.write_bytes(b'INSTALLER')
        start = len(self.commands)
        with self.assertRaisesRegex(ValueError, 'content mismatch'): self.publish()
        self.assertEqual(self.mutations(start), [])

    def test_different_tag_commit_is_rejected_before_mutation(self):
        self.tags['v0.1.0'] = 'b' * 40
        with self.assertRaisesRegex(ValueError, 'commit differs'): self.publish()
        self.assertEqual(self.mutations(), [])

    def test_retargeted_version_tag_cannot_change_recorded_commit(self):
        self.publish()
        self.tags['v0.1.0'] = 'b' * 40
        start = len(self.commands)
        with self.assertRaisesRegex(ValueError, 'recorded version/commit'):
            self.publish(sha='b' * 40)
        self.assertEqual(self.mutations(start), [])

    def test_manifest_version_mismatch_is_rejected_before_mutation(self):
        self.tags['v0.2.0-beta.1'] = SHA
        with self.assertRaisesRegex(ValueError, 'manifest version differs'):
            publisher.publish_set('v0.2.0-beta.1', SHA, [self.path, self.feed])
        self.assertEqual(self.mutations(), [])

    def test_legacy_assets_without_digests_are_downloaded_and_verified(self):
        self.publish()
        self.releases['v0.1.0']['assets'][0].pop('digest')
        self.publish()
        self.assertTrue(any(command[:2] == ('release', 'download') for command in self.commands))
        self.releases['v0.1.0']['assets'][0]['bytes'] = b'INSTALLER'
        with self.assertRaisesRegex(ValueError, 'content mismatch'): self.publish()

    def test_rolling_tag_moves_only_after_upload_and_before_publication(self):
        self.tags['dev-latest'] = 'b' * 40
        self.publish('dev-latest')
        update = next(i for i, command in enumerate(self.commands) if command[:3] == ('api', '--method', 'PATCH'))
        upload = next(i for i, command in enumerate(self.commands) if command[:2] == ('release', 'upload'))
        self.assertGreater(update, upload)
        self.assertLess(update, len(self.commands) - 1)
        self.assertEqual(self.commands[-1][-2:], ('--draft=false', '--latest=false'))

    def test_missing_rolling_tag_creation_failure_never_publishes(self):
        self.fail_once(lambda args: args[:3] == ('api', '--method', 'POST'))
        with self.assertRaises(subprocess.CalledProcessError): self.publish('dev-latest')
        self.assertFalse(any('--draft=false' in command for command in self.commands))

    def test_beta_upload_failure_leaves_both_drafts_and_retries(self):
        tag = 'v0.1.0-beta.1'
        self.fail_once(lambda args: args[:3] == ('release', 'upload', 'beta-latest'))
        with self.assertRaises(subprocess.CalledProcessError): self.publish(tag)
        self.assertTrue(self.releases[tag]['draft'])
        self.assertTrue(self.releases['beta-latest']['draft'])
        self.publish(tag)
        self.assertFalse(self.releases[tag]['draft'])
        self.assertFalse(self.releases['beta-latest']['draft'])
        first_public = next(i for i, command in enumerate(self.commands) if '--draft=false' in command)
        self.assertTrue(any(command[:3] == ('api', '--method', 'POST') for command in self.commands[:first_public]))

    def test_beta_tag_failure_leaves_both_drafts_and_retries(self):
        tag = 'v0.1.0-beta.1'
        self.fail_once(lambda args: args[:3] == ('api', '--method', 'POST'))
        with self.assertRaises(subprocess.CalledProcessError): self.publish(tag)
        self.assertTrue(self.releases[tag]['draft'])
        self.assertTrue(self.releases['beta-latest']['draft'])
        self.publish(tag)
        self.assertEqual(self.tags['beta-latest'], SHA)
        self.assertFalse(self.releases['beta-latest']['draft'])

    def test_beta_feed_publication_failure_resumes_after_version_is_public(self):
        tag = 'v0.1.0-beta.1'
        self.fail_once(lambda args: args[:3] == ('release', 'edit', 'beta-latest') and '--draft=false' in args)
        with self.assertRaises(subprocess.CalledProcessError): self.publish(tag)
        self.assertFalse(self.releases[tag]['draft'])
        self.assertTrue(self.releases['beta-latest']['draft'])
        start = len(self.commands)
        self.publish(tag)
        self.assertFalse(self.releases['beta-latest']['draft'])
        self.assertFalse(any(command[:3] in {('release', 'edit', tag), ('release', 'upload', tag)} for command in self.commands[start:]))

    def test_completed_beta_retry_is_read_only(self):
        tag = 'v0.1.0-beta.1'
        self.publish(tag)
        start = len(self.commands)
        self.publish(tag)
        self.assertEqual(self.mutations(start), [])

    def test_version_publication_failure_retries_both_staged_beta_assets(self):
        tag = 'v0.1.0-beta.1'
        self.fail_once(lambda args: args[:3] == ('release', 'edit', tag) and '--draft=false' in args)
        with self.assertRaises(subprocess.CalledProcessError): self.publish(tag)
        self.assertTrue(self.releases[tag]['draft'])
        self.assertTrue(self.releases['beta-latest']['draft'])
        self.assertEqual(self.tags['beta-latest'], SHA)
        self.publish(tag)
        self.assertFalse(self.releases[tag]['draft'])
        self.assertFalse(self.releases['beta-latest']['draft'])

    def test_beta_partial_retry_rejects_changed_version_artifacts(self):
        tag = 'v0.1.0-beta.1'
        self.fail_once(lambda args: args[:3] == ('release', 'edit', 'beta-latest') and '--draft=false' in args)
        with self.assertRaises(subprocess.CalledProcessError): self.publish(tag)
        self.path.write_bytes(b'INSTALLER')
        with self.assertRaisesRegex(ValueError, 'content mismatch'): self.publish(tag)
        self.assertTrue(self.releases['beta-latest']['draft'])

    def test_older_stable_version_does_not_move_latest_back(self):
        self.publish('v0.2.0', 'b' * 40)
        self.publish('v0.1.0')
        self.assertEqual(self.latest, 'v0.2.0')
        self.assertFalse(self.releases['v0.1.0']['draft'])
        self.assertEqual(self.commands[-1][-1], '--latest=false')

    def test_older_beta_build_does_not_replace_the_channel(self):
        self.publish('v0.1.0-beta.10', 'b' * 40)
        old_body = self.releases['beta-latest']['body']
        self.publish('v0.1.0-beta.9')
        self.assertEqual(self.tags['beta-latest'], 'b' * 40)
        self.assertEqual(self.releases['beta-latest']['body'], old_body)
        self.assertFalse(self.releases['v0.1.0-beta.9']['draft'])

    def test_same_beta_version_cannot_change_commit(self):
        self.publish('v0.1.0-beta.1')
        self.tags['v0.1.0-beta.1'] = 'b' * 40
        start = len(self.commands)
        with self.assertRaisesRegex(ValueError, 'different commit'): self.publish('v0.1.0-beta.1', 'b' * 40)
        self.assertEqual(self.mutations(start), [])

    def test_channel_lookup_network_error_fails_closed(self):
        self.fail_once(lambda args: args[0] == 'api' and args[1].endswith('/releases/latest'))
        with self.assertRaises(subprocess.CalledProcessError): self.publish()
        self.assertEqual(self.mutations(), [])

    def test_semantic_version_order(self):
        versions = ['0.1.0-alpha', '0.1.0-alpha.1', '0.1.0-beta.9', '0.1.0-beta.10', '0.1.0', '0.2.0', '1.0.0']
        self.assertEqual(sorted(reversed(versions), key=publisher.version_key), versions)
        for invalid in ['v1.0', 'v1.0.0-beta.01', 'v1.0.0-beta..1', 'v1\u0660.0.0', 'not-a-version']:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError): publisher.version_key(invalid)


@unittest.skipUnless(shutil.which('minisign'), 'minisign integration runs in delivery CI')
class RealSignatureTests(unittest.TestCase):
    def test_generated_signature_accepts_original_and_rejects_tampering(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            public, secret = root / 'key.pub', root / 'key.key'
            payload = root / 'fixture.AppImage'
            payload.write_bytes(b'original updater payload')
            subprocess.run(['minisign', '-G', '-W', '-p', str(public), '-s', str(secret)], check=True, capture_output=True)
            raw_signature = root / 'payload.minisig'
            subprocess.run(['minisign', '-S', '-s', str(secret), '-m', str(payload), '-x', str(raw_signature)], check=True, capture_output=True)
            signature = base64.b64encode(raw_signature.read_bytes()).decode()
            payload.with_name(payload.name + '.sig').write_text(signature)
            key = base64.b64encode(public.read_bytes()).decode()
            self.assertEqual(manifest.signature_of(payload, key), signature)
            payload.write_bytes(b'tampered updater payload')
            with self.assertRaises(subprocess.CalledProcessError):
                manifest.signature_of(payload, key)


if __name__ == '__main__':
    unittest.main()
