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
             patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'):
            launch.launch(self.args('agents', 'opencode-go'))
            text = next(launch.STATE.glob('*.json')).read_text()
            self.assertNotIn('secret-value', text + repr(self.calls))
            self.assertEqual(json.loads(text)['model'], 'deepseek-v4.1-flash')
            self.assertEqual(len([c for c in self.calls if c[:2] == ('network', 'create')]), 2)

    def test_gateway_alias_and_model(self):
        with patch.dict(os.environ, {'CCC_HOSTED_OPT_IN': '1', 'AI_GATEWAY_API_KEY': '',
                                     'VERCEL_AI_GATEWAY_API_KEY': 'gateway-secret'}), \
             patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'):
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
                    return ('sender payload test: [01, 02, 03, 04]', '')
                return ('received bytes (hex): 01020304', '')
            def poll(self):
                return 0
            def wait(self):
                return 0
        with patch.object(launch.subprocess, 'Popen', Process), \
             patch.object(launch.time, 'sleep'), patch.object(launch, 'cleanup') as cleanup:
            launch.preflight('a' * 32, 'sha256:' + 'a' * 64, 'cache', 0)
            cleanup.assert_called_once_with('a' * 32)

    def test_invalid_stop(self):
        with self.assertRaisesRegex(RuntimeError, 'invalid run ID'):
            launch.stop('../bad')


if __name__ == '__main__':
    unittest.main()
