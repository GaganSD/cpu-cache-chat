#!/usr/bin/env python3
import argparse
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

path = Path(__file__).resolve().parents[1] / 'launch.py'
spec = importlib.util.spec_from_file_location('launch', path)
launch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launch)


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.old_state = launch.STATE
        launch.STATE = Path(self.tmp.name)
        self.addCleanup(setattr, launch, 'STATE', self.old_state)
        self.calls = []

    def docker(self, *args, **kwargs):
        self.calls.append(args)
        if args[:2] == ('info', '--format'):
            return '4'
        if args[:2] == ('image', 'inspect'):
            return 'sha256:' + 'a' * 64
        if args[:2] in (('ps', '-aq'), ('network', 'ls')):
            return ''
        return 'ok'

    def args(self, command='chat', provider=None):
        return argparse.Namespace(command=command, provider=provider, channel='cpu', cpu=0,
                                  image=None, skip_preflight=True)

    def test_manual_and_stop(self):
        with patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'):
            launch.launch(self.args())
            files = list(launch.STATE.glob('*.json'))
            self.assertEqual(len(files), 1)
            data = json.loads(files[0].read_text())
            self.assertEqual(data['preset'], 'reference-50ms')
            self.assertEqual(data['image_id'], 'sha256:' + 'a' * 64)
            self.assertEqual(data['status'], 'running')
            self.assertEqual(data['probe'], 'skipped')
            self.assertEqual(data['events_path'], '/tmp/game.events.jsonl')
            self.assertIn('binary_version', data)
            self.assertEqual(len([c for c in self.calls if c[0] == 'create']), 2)
            for c in [c for c in self.calls if c[0] == 'create']:
                self.assertIn('--read-only', c)
                self.assertIn('sha256:' + 'a' * 64, c)
                self.assertIn('none', c)
                self.assertIn('--events-path', c)
                self.assertIn('/tmp/game.events.jsonl', c)
            launch.stop(data['run_id'])
            data = json.loads(files[0].read_text())
            self.assertEqual(data['status'], 'stopped')
            self.assertIsNotNone(data['end'])
            self.assertTrue(any(f'{launch.LABEL}={data["run_id"]}' in x for c in self.calls for x in c))
            # Stopping again is idempotent and retains the manifest.
            launch.stop(data['run_id'])
            self.assertTrue(files[0].exists())

    def test_hosted_and_secret_absence(self):
        with patch.dict(os.environ, {'CCC_HOSTED_OPT_IN': '1', 'OPENCODE_GO_API_KEY': 'secret-value'}), \
             patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'), \
             patch.object(launch, 'verify_peer_paths', return_value={'ipv6': 'disabled'}):
            launch.launch(self.args('agents', 'opencode-go'))
            text = next(launch.STATE.glob('*.json')).read_text()
            self.assertNotIn('secret-value', text + repr(self.calls))
            data = json.loads(text)
            self.assertEqual(data['model'], 'deepseek-v4.1-flash')
            self.assertEqual(data['transcript_path'], '/tmp/agent-transcript.log')
            self.assertEqual(data['peer_path_test'], {'ipv6': 'disabled'})
            self.assertEqual(len([c for c in self.calls if c[:2] == ('network', 'create')]), 2)
            for c in [c for c in self.calls if c[0] == 'create']:
                self.assertIn('CCC_AGENT_TRANSCRIPT_PATH=/tmp/agent-transcript.log', c)
                self.assertIn('CCC_AGENT_DEBUG=1', c)

    def test_gateway_alias_and_model(self):
        with patch.dict(os.environ, {'CCC_HOSTED_OPT_IN': '1', 'AI_GATEWAY_API_KEY': '',
                                     'VERCEL_AI_GATEWAY_API_KEY': 'gateway-secret'}), \
             patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'), \
             patch.object(launch, 'verify_peer_paths', return_value={'ipv6': 'disabled'}):
            launch.launch(self.args('agents', 'vercel-ai-gateway'))
            text = next(launch.STATE.glob('*.json')).read_text()
            self.assertEqual(json.loads(text)['model'], 'reflection/beam-free')
            self.assertNotIn('gateway-secret', text + repr(self.calls))

    def test_preflight_failure_retains_manifest(self):
        args = self.args()
        args.skip_preflight = False
        with patch.object(launch, 'docker', self.docker), \
             patch.object(launch, 'preflight', side_effect=RuntimeError('probe failure')):
            with self.assertRaisesRegex(RuntimeError, 'probe failure'):
                launch.launch(args)
            files = list(launch.STATE.glob('*.json'))
            self.assertEqual(len(files), 1)
            data = json.loads(files[0].read_text())
            self.assertEqual(data['status'], 'failed')
            self.assertEqual(data['probe'], 'pending')
            self.assertIn('probe failure', data['error'])
            self.assertIsNotNone(data['end'])
            self.assertFalse(any(c[0] == 'create' for c in self.calls))
            self.assertTrue(any(c[:2] == ('ps', '-aq') for c in self.calls))

    def test_credentials_fail_before_docker(self):
        with patch.dict(os.environ, {'CCC_HOSTED_OPT_IN': '1', 'OPENCODE_GO_API_KEY': ''}), \
             patch.object(launch, 'docker', self.docker):
            with self.assertRaisesRegex(RuntimeError, 'OPENCODE_GO_API_KEY'):
                launch.launch(self.args('agents', 'opencode-go'))
            self.assertEqual(self.calls, [])

    def test_failure_cleanup_and_cpu_validation(self):
        def failing(*args, **kwargs):
            if args[0] == 'create':
                raise RuntimeError('injected')
            return self.docker(*args, **kwargs)
        with patch.object(launch, 'docker', failing):
            with self.assertRaisesRegex(RuntimeError, 'injected'):
                launch.launch(self.args())
            files = list(launch.STATE.glob('*.json'))
            self.assertEqual(len(files), 1)
            self.assertEqual(json.loads(files[0].read_text())['status'], 'failed')
            self.assertTrue(any(c[:2] == ('ps', '-aq') for c in self.calls))
            a = self.args()
            a.cpu = 4
            with self.assertRaisesRegex(RuntimeError, 'unavailable'):
                launch.launch(a)

    def test_preflight_compares_receiver_blind_bytes(self):
        class Process:
            def __init__(self, cmd, **_kwargs):
                self.role = cmd[-1]
                self.returncode = 0
            def communicate(self, timeout):
                if self.role == 'a':
                    return (json.dumps({'schema_version': 1, 'role': 'A', 'requested_bits': 32, 'payload_hex': '01020304'}), '')
                return (json.dumps({'schema_version': 1, 'requested_bits': 32, 'requested_trials': 1,
                                    'attempted_trials': 1, 'completed_trials': 1, 'usable': False,
                                    'trials': [{'outcome': 'decoded', 'decoded_bytes': 4,
                                                'crc_valid': True, 'decoded_hex': '01020304'}]}), '')
            def poll(self):
                return 0
            def wait(self):
                return 0
        with patch.object(launch.subprocess, 'Popen', Process), \
             patch.object(launch.time, 'sleep'), patch.object(launch, 'cleanup') as cleanup:
            launch.preflight('a' * 32, 'sha256:' + 'a' * 64, 'cache', 0)
            cleanup.assert_called_once_with('a' * 32)

    def test_live_controls_and_cleanup(self):
        def run_case(outcomes, expected_error=None, fail_create=False):
            calls = []
            responses = iter(outcomes)
            def fake(*args, **kwargs):
                calls.append(args)
                if args[:2] == ('network', 'inspect'):
                    return json.dumps([{'EnableIPv6': False}])
                if args[0] == 'inspect':
                    name = args[-1]
                    net = 'net-a' if name.endswith('-a') else 'net-b'
                    return json.dumps({net: {'IPAddress': '172.20.0.2'}})
                if args[0] == 'create' and fail_create and 'control-positive-b' in ' '.join(args):
                    raise RuntimeError('create failed')
                if args[:2] == ('start', '-a'):
                    return next(responses)
                return 'ok'
            with patch.object(launch, 'docker', fake), patch.object(launch.time, 'sleep'):
                if expected_error:
                    with self.assertRaisesRegex(RuntimeError, expected_error):
                        launch.verify_peer_paths('a' * 32, 'image', 0, ['net-a', 'net-b'])
                else:
                    result = launch.verify_peer_paths('a' * 32, 'image', 0, ['net-a', 'net-b'])
                    self.assertEqual(result['ipv6'], 'disabled')
            creates = [c[c.index('--name') + 1] for c in calls if c[0] == 'create' and not (fail_create and 'control-positive-b' in ' '.join(c))]
            removes = [c[-1] for c in calls if c[:2] == ('rm', '-f')]
            self.assertEqual(removes, list(reversed(creates)))
        run_case(['CONNECTED', 'CONNECTED', 'ENETUNREACH', 'ETIMEDOUT'])
        run_case(['ECONNREFUSED'], 'positive')
        run_case(['CONNECTED', 'CONNECTED', 'ECONNREFUSED', 'ETIMEDOUT'], 'not explicitly denied')
        run_case(['CONNECTED'], 'create failed', True)

    def test_ipv6_refused_and_cleanup_error(self):
        with patch.object(launch, 'docker', return_value='[{"EnableIPv6": true}]'):
            with self.assertRaisesRegex(RuntimeError, 'IPv6'):
                launch.verify_peer_paths('a' * 32, 'image', 0, ['net-a', 'net-b'])
        calls = []
        def fake(*args, **kwargs):
            calls.append(args)
            if args[:2] == ('network', 'inspect'):
                return '[{"EnableIPv6": false}]'
            if args[0] == 'inspect':
                raise RuntimeError('inspect failure')
            if args[0] == 'rm':
                raise RuntimeError('remove failure')
            return 'ok'
        with patch.object(launch, 'docker', fake):
            with self.assertRaisesRegex(RuntimeError, 'cleanup incomplete'):
                launch.verify_peer_paths('a' * 32, 'image', 0, ['net-a', 'net-b'])
            self.assertTrue(any(c[0] == 'rm' for c in calls))

    def test_invalid_stop(self):
        with self.assertRaisesRegex(RuntimeError, 'invalid run ID'):
            launch.stop('../bad')

    def test_stop_ownership_checks(self):
        run = 'd' * 32
        path = launch.STATE / (run + '.json')
        path.write_text(json.dumps({'run_id': run, 'label': 'other-label', 'status': 'running'}))
        with self.assertRaisesRegex(RuntimeError, 'ownership'):
            launch.stop(run)
        path.write_text(json.dumps({'run_id': 'e' * 32, 'label': launch.LABEL}))
        with self.assertRaisesRegex(RuntimeError, 'ownership'):
            launch.stop(run)

    def hosted_manifest(self, run):
        path = launch.STATE / (run + '.json')
        path.write_text(json.dumps({'run_id': run, 'label': launch.LABEL, 'mode': 'agents',
                                    'status': 'running'}))

    def test_stop_captures_redacts_before_removal(self):
        run = 'b' * 32
        self.hosted_manifest(run)
        def fake(*args, **kwargs):
            self.calls.append(args)
            if args[:2] == ('ps', '-aq'):
                return 'cid-a\ncid-b'
            if args[:2] == ('network', 'ls'):
                return ''
            if args[0] == 'inspect':
                return 'ccc-' + args[-1]
            if args[0] == 'cp':
                if 'game.events.jsonl' in args[1]:
                    Path(args[-1]).write_text('{"type":"move"}\n')
                    return ''
                if 'agent-transcript.log' in args[1] and 'cid-b' in args[1]:
                    Path(args[-1]).write_text('raw model transcript\n')
                    return ''
                return None
            return 'ok'
        logs = ('session Bearer tok-abcdef123 api_key=hunter2secret live-secret-value\n', 'debug err\n')
        with patch.dict(os.environ, {'OPENCODE_GO_API_KEY': 'live-secret-value'}), \
             patch.object(launch, 'docker', fake), \
             patch.object(launch, 'read_container_logs', return_value=logs):
            launch.stop(run)
        data = json.loads((launch.STATE / (run + '.json')).read_text())
        self.assertEqual(data['status'], 'stopped')
        self.assertEqual(data['evidence']['ccc-cid-a']['transcript'], 'missing')
        self.assertEqual(data['evidence']['ccc-cid-b']['transcript'], 'captured')
        self.assertTrue(data['incomplete'])
        self.assertTrue(any('transcript missing' in r for r in data['incomplete_reasons']))
        # docker cp of every artifact precedes any removal of that container.
        last_cp = max(i for i, c in enumerate(self.calls) if c[0] == 'cp')
        first_rm = min(i for i, c in enumerate(self.calls) if c[:2] == ('rm', '-f'))
        self.assertLess(last_cp, first_rm)
        out = (launch.STATE / run / 'logs' / 'ccc-cid-a.stdout').read_text()
        self.assertTrue(out.startswith('REVIEW BEFORE SHARING'))
        for secret in ('hunter2secret', 'live-secret-value', 'tok-abcdef123'):
            self.assertNotIn(secret, out)
        self.assertIn('[REDACTED]', out)
        # Missing artifacts are recorded, never invented.
        self.assertFalse((launch.STATE / run / 'transcripts' / 'ccc-cid-a.log').exists())
        self.assertTrue((launch.STATE / run / 'transcripts' / 'ccc-cid-b.log').exists())
        self.assertTrue((launch.STATE / run / 'events' / 'ccc-cid-a.jsonl').exists())

    def test_truncated_transcript_marks_incomplete(self):
        run = 'c' * 32
        self.hosted_manifest(run)
        def fake(*args, **kwargs):
            self.calls.append(args)
            if args[:2] == ('ps', '-aq'):
                return 'cid-a'
            if args[:2] == ('network', 'ls'):
                return ''
            if args[0] == 'inspect':
                return 'ccc-' + args[-1]
            if args[0] == 'cp':
                Path(args[-1]).write_bytes(b'x' * launch.TRANSCRIPT_QUOTA)
                return ''
            return 'ok'
        with patch.object(launch, 'docker', fake), \
             patch.object(launch, 'read_container_logs', return_value=('', '')), \
             patch.object(launch, 'TRANSCRIPT_QUOTA', 8):
            launch.stop(run)
        data = json.loads((launch.STATE / (run + '.json')).read_text())
        self.assertEqual(data['evidence']['ccc-cid-a']['transcript'], 'truncated')
        self.assertTrue(any('transcript truncated' in r for r in data['incomplete_reasons']))

    def test_run_store_quota(self):
        with patch.object(launch, 'RUN_QUOTA', 256):
            store = launch.RunStore('f' * 32)
            room = 256 - len(launch.ARTIFACT_HEADER.encode())
            self.assertTrue(store.write('a', 'x' * room))
            self.assertFalse(store.write('b', 'y'))
            self.assertTrue(store.incomplete)
            total = sum(p.stat().st_size for p in store.dir.rglob('*') if p.is_file())
            self.assertLessEqual(total, 256)
            store2 = launch.RunStore('0' * 32)
            self.assertFalse(store2.write('a', 'z' * 1024))
            total = sum(p.stat().st_size for p in store2.dir.rglob('*') if p.is_file())
            self.assertLessEqual(total, 256)

    def test_retention_budget_refuses_launch(self):
        for i in range(launch.MAX_RETAINED_RUNS):
            (launch.STATE / f'{i:032x}.json').write_text('{}')
        unknown = launch.STATE / 'keep-me.txt'
        unknown.write_text('unrelated user file')
        with patch.object(launch, 'docker', self.docker):
            with self.assertRaisesRegex(RuntimeError, 'retention budget'):
                launch.launch(self.args())
            self.assertEqual(self.calls, [])
        self.assertTrue(unknown.exists())
        for p in launch.STATE.glob('*.json'):
            p.unlink()
        run_dir = launch.STATE / ('1' * 32)
        run_dir.mkdir()
        (run_dir / 'big').write_bytes(b'x' * 64)
        (launch.STATE / ('1' * 32 + '.json')).write_text('{}')
        with patch.object(launch, 'docker', self.docker), \
             patch.object(launch, 'RETENTION_BUDGET', 8):
            with self.assertRaisesRegex(RuntimeError, 'retention budget'):
                launch.launch(self.args())
            self.assertEqual(self.calls, [])
        self.assertTrue(unknown.exists())

    def test_redact(self):
        with patch.dict(os.environ, {'OPENCODE_GO_API_KEY': 'sk-live-123456'}):
            out = launch.redact('key sk-live-123456; Bearer abc.def-123; api_key=zz9/secret; ok')
        for secret in ('sk-live-123456', 'abc.def-123', 'zz9/secret'):
            self.assertNotIn(secret, out)
        self.assertIn('ok', out)
        self.assertEqual(out.count('[REDACTED]'), 3)


if __name__ == '__main__':
    unittest.main()
