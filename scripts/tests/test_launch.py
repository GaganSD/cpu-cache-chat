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
            self.assertEqual(len([c for c in self.calls if c[0] == 'create']), 2)
            for c in [c for c in self.calls if c[0] == 'create']:
                self.assertIn('--read-only', c)
                self.assertIn('sha256:' + 'a' * 64, c)
                self.assertIn('none', c)
            launch.stop(data['run_id'])
            self.assertFalse(files[0].exists())
            self.assertTrue(any(f'{launch.LABEL}={data["run_id"]}' in x for c in self.calls for x in c))

    def test_hosted_and_secret_absence(self):
        with patch.dict(os.environ, {'CCC_HOSTED_OPT_IN': '1', 'OPENCODE_GO_API_KEY': 'secret-value'}), \
             patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'), \
             patch.object(launch, 'verify_peer_paths', return_value={'ipv6': 'disabled'}):
            launch.launch(self.args('agents', 'opencode-go'))
            text = next(launch.STATE.glob('*.json')).read_text()
            self.assertNotIn('secret-value', text + repr(self.calls))
            self.assertEqual(json.loads(text)['model'], 'deepseek-v4.1-flash')
            self.assertEqual(len([c for c in self.calls if c[:2] == ('network', 'create')]), 2)

    def test_gateway_alias_and_model(self):
        with patch.dict(os.environ, {'CCC_HOSTED_OPT_IN': '1', 'AI_GATEWAY_API_KEY': '',
                                     'VERCEL_AI_GATEWAY_API_KEY': 'gateway-secret'}), \
             patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'), \
             patch.object(launch, 'verify_peer_paths', return_value={'ipv6': 'disabled'}):
            launch.launch(self.args('agents', 'vercel-ai-gateway'))
            text = next(launch.STATE.glob('*.json')).read_text()
            self.assertEqual(json.loads(text)['model'], 'reflection/beam-free')
            self.assertNotIn('gateway-secret', text + repr(self.calls))

    def test_preflight_failure_cleans_manifest(self):
        args = self.args()
        args.skip_preflight = False
        with patch.object(launch, 'docker', self.docker), \
             patch.object(launch, 'preflight', side_effect=RuntimeError('probe failure')):
            with self.assertRaisesRegex(RuntimeError, 'probe failure'):
                launch.launch(args)
            self.assertEqual(list(launch.STATE.iterdir()), [])
            self.assertFalse(any(c[0] == 'create' for c in self.calls))

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
            self.assertEqual(list(launch.STATE.iterdir()), [])
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


if __name__ == '__main__':
    unittest.main()
