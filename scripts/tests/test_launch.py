#!/usr/bin/env python3
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

path = Path(__file__).resolve().parents[1] / 'launch.py'
spec = importlib.util.spec_from_file_location('launch', path)
launch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launch)

EVENTS_A = (b'{"type":"session","version":1,"session":7,"role":"A"}\n'
            b'{"type":"final","board":"XOXOXOXOX","ply":9,"outcome":"draw"}\n'
            b'{"type":"stopped","successful":true,"evidence_complete":true}\n')
EVENTS_B = EVENTS_A.replace(b'"role":"A"', b'"role":"B"')
MOVE = b'{"type":"move","ply":0,"square":5,"mark":"X","sequence":1,"acceptance":"local_ack"}'


def mirror(events, prefix=b''):
    return prefix + b''.join(launch.EVENT_PREFIX.encode() + line + b'\n'
                             for line in events.split(b'\n') if line)


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

    def write_manifest(self, run, mode):
        path = launch.STATE / (run + '.json')
        path.write_text(json.dumps({'run_id': run, 'label': launch.LABEL, 'mode': mode,
                                    'status': 'running', 'image_id': 'sha256:' + 'a' * 64,
                                    'cpu': 0}))
        return path

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
            creates = [c for c in self.calls if c[0] == 'create']
            self.assertEqual(len(creates), 2)
            for c in creates:
                self.assertIn('--read-only', c)
                self.assertIn('--cgroupns', c)
                self.assertIn('private', c)
                self.assertIn('max-size=4m', c)
                self.assertIn('max-file=2', c)
                self.assertIn('sha256:' + 'a' * 64, c)
                self.assertIn('none', c)
                self.assertIn('--plain', c)
                # Chat is plain-only: no game events, no game verifier.
                self.assertNotIn('--events-path', c)
                self.assertNotIn('--events-stdout', c)
            launch.stop(data['run_id'])
            data = json.loads(files[0].read_text())
            self.assertEqual(data['status'], 'stopped')
            self.assertIsNotNone(data['end'])
            self.assertNotIn('incomplete', data)
            self.assertTrue(any(f'{launch.LABEL}={data["run_id"]}' in x for c in self.calls for x in c))
            # Stopping again is idempotent and retains the manifest.
            launch.stop(data['run_id'])
            self.assertTrue(files[0].exists())

    def test_game_launch_flags(self):
        with patch.object(launch, 'docker', self.docker), patch.object(launch.subprocess, 'run'):
            launch.launch(self.args('game'))
            creates = [c for c in self.calls if c[0] == 'create']
            self.assertEqual(len(creates), 2)
            for c in creates:
                self.assertIn('--plain', c)
                self.assertIn('--events-stdout', c)
                self.assertIn('--events-path', c)
                self.assertIn('/tmp/game.events.jsonl', c)
                self.assertNotIn('--agent', c)

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
                self.assertIn('--events-stdout', c)
                self.assertIn('--agent', c)

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
        return self.write_manifest(run, 'agents')

    def peer_docker(self, containers=('cid-a', 'cid-b'), networks=('net-1',), wait_result='0',
                    events=(EVENTS_A, EVENTS_B), transcripts=(None, b'raw model transcript\n')):
        def fake(*args, **kwargs):
            self.calls.append(args)
            if args[:2] == ('ps', '-aq'):
                return '\n'.join(containers)
            if args[:2] == ('network', 'ls'):
                return '\n'.join(networks)
            if args[0] == 'wait':
                if isinstance(wait_result, Exception):
                    raise wait_result
                return wait_result
            if args[0] == 'inspect':
                return 'ccc-' + args[-1]
            if args[0] == 'cp':
                ident = args[1].split(':')[0]
                index = containers.index(ident)
                if 'game.events.jsonl' in args[1] and events[index] is not None:
                    Path(args[-1]).write_bytes(events[index])
                    return ''
                if 'agent-transcript.log' in args[1] and transcripts[index] is not None:
                    Path(args[-1]).write_bytes(transcripts[index])
                    return ''
                return None
            return 'ok'
        return fake

    def test_stop_orders_capture_and_records_verdict(self):
        run = 'b' * 32
        self.hosted_manifest(run)
        secret_log = b'session Bearer tok-abcdef123 api_key=hunter2secret live-secret-value\n'
        logs = {'cid-a': (mirror(EVENTS_A, secret_log), b'debug err\n', False),
                'cid-b': (mirror(EVENTS_B), b'', False)}
        verdict = (0, '{"ok": true, "session": 7, "outcome": "draw", "agents_verified": true}')
        events_by_ident = {'cid-a': EVENTS_A, 'cid-b': EVENTS_B}
        def fake_capture(*args, **kwargs):
            self.calls.append(args)
            if args[:1] == ('exec',):
                return 0, events_by_ident[args[1]].decode()
            return verdict
        with patch.dict(os.environ, {'OPENCODE_GO_API_KEY': 'live-secret-value'}), \
             patch.object(launch, 'docker', self.peer_docker()), \
             patch.object(launch, 'read_container_logs', side_effect=lambda ident, **kw: logs[ident]), \
             patch.object(launch, 'docker_capture', fake_capture):
            launch.stop(run)
        data = json.loads((launch.STATE / (run + '.json')).read_text())
        self.assertEqual(data['status'], 'stopped')
        self.assertEqual(data['game_verdict']['ok'], True)
        self.assertEqual(data['game_verdict']['via'], 'observer-container')
        rec_a = data['evidence']['ccc-cid-a']
        self.assertEqual(rec_a['events'], 'captured-live')
        self.assertEqual(rec_a['events_stdout'], 'captured')
        self.assertEqual(rec_a['exit'], 0)
        self.assertEqual(rec_a['logs'], 'captured')
        self.assertEqual(rec_a['transcript'], 'missing')
        self.assertEqual(data['evidence']['ccc-cid-b']['transcript'], 'captured')
        self.assertTrue(data['incomplete'])
        self.assertTrue(any('transcript missing' in r for r in data['incomplete_reasons']))
        self.assertFalse(any('verification' in r for r in data['incomplete_reasons']))
        # Canonical event files are byte-exact, headerless, strict JSONL.
        for name, events in (('ccc-cid-a', EVENTS_A), ('ccc-cid-b', EVENTS_B)):
            for suffix in ('.jsonl', '.stdout.jsonl'):
                path = launch.STATE / run / 'events' / f'{name}{suffix}'
                self.assertEqual(path.read_bytes(), events)
                self.assertFalse(path.read_text().startswith('REVIEW BEFORE SHARING'))
                for line in path.read_text().splitlines():
                    json.loads(line)
            self.assertNotIn('live-secret-value', path.read_text())
        # Text artifacts keep the warning header and are redacted.
        out = (launch.STATE / run / 'logs' / 'ccc-cid-a.stdout').read_text()
        self.assertTrue(out.startswith('REVIEW BEFORE SHARING'))
        for secret in ('hunter2secret', 'live-secret-value', 'tok-abcdef123'):
            self.assertNotIn(secret, out)
        self.assertIn('[REDACTED]', out)
        # Missing artifacts are recorded, never invented.
        self.assertFalse((launch.STATE / run / 'transcripts' / 'ccc-cid-a.log').exists())
        self.assertTrue((launch.STATE / run / 'transcripts' / 'ccc-cid-b.log').exists())
        # Ordering: transcript copy while peers live, SIGINT, live events
        # capture during the grace window, bounded wait, container removal,
        # then network removal. A stopped peer's tmpfs is gone, so the events
        # file must be read before exit, never by post-exit cp.
        index = lambda pred: [i for i, c in enumerate(self.calls) if pred(c)]
        tx_cp = index(lambda c: c[0] == 'cp' and 'agent-transcript.log' in c[1])
        ev_exec = index(lambda c: c[:1] == ('exec',) and any('game.events.jsonl' in x for x in c))
        kills = index(lambda c: c[:2] == ('kill', '--signal'))
        waits = index(lambda c: c[0] == 'wait')
        rms = index(lambda c: c[:2] == ('rm', '-f'))
        netrms = index(lambda c: c[:2] == ('network', 'rm'))
        self.assertTrue(all((tx_cp, ev_exec, kills, waits, rms, netrms)))
        self.assertLess(max(tx_cp), min(kills))
        self.assertLess(max(kills), min(ev_exec))
        self.assertLess(max(ev_exec), min(waits))
        self.assertLess(max(waits), min(rms))
        self.assertLess(max(rms), min(netrms))
        self.assertFalse(index(lambda c: c[0] == 'cp' and 'game.events.jsonl' in c[1]))
        self.assertTrue(all(c[2] == 'SIGINT' for c in self.calls if c[:2] == ('kill', '--signal')))

    def test_missing_final_or_stopped_marks_incomplete(self):
        run = '1' * 32
        path = self.write_manifest(run, 'game')
        no_stopped = EVENTS_A.replace(
            b'{"type":"stopped","successful":true,"evidence_complete":true}', MOVE)
        bad_stop = EVENTS_A.replace(b'"successful":true', b'"successful":false')
        events_by_ident = {'cid-a': no_stopped, 'cid-b': bad_stop}
        def fake_capture(*args, **kwargs):
            if args[:1] == ('exec',):
                return 0, events_by_ident[args[1]].decode()
            raise AssertionError('verifier must not run without canonical events')
        with patch.object(launch, 'docker', self.peer_docker(events=(no_stopped, bad_stop),
                                                             transcripts=(None, None))), \
             patch.object(launch, 'read_container_logs', return_value=(b'', b'', False)), \
             patch.object(launch, 'WAIT_TIMEOUT', 0.2), \
             patch.object(launch, 'docker_capture', fake_capture):
            launch.stop(run)
        data = json.loads(path.read_text())
        self.assertEqual(data['evidence']['ccc-cid-a']['events'],
                         'incomplete: missing stopped record')
        self.assertEqual(data['evidence']['ccc-cid-b']['events'],
                         'incomplete: stopped record reports unsuccessful exit')
        self.assertTrue(data['incomplete'])
        self.assertTrue(any('missing stopped record' in r for r in data['incomplete_reasons']))
        self.assertTrue(any('unsuccessful exit' in r for r in data['incomplete_reasons']))
        self.assertTrue(any('verification skipped' in r for r in data['incomplete_reasons']))
        # No verdict is invented and malformed evidence is never stored as canonical.
        self.assertNotIn('game_verdict', data)
        self.assertFalse((launch.STATE / run / 'events' / 'ccc-cid-a.jsonl').exists())

    def test_observed_exit_failure_marks_incomplete(self):
        run = '2' * 32
        path = self.write_manifest(run, 'game')
        def fake_capture(*args, **kwargs):
            if args[:1] == ('exec',):
                return 0, EVENTS_A.decode()
            raise AssertionError('verifier must not run for a single peer')
        with patch.object(launch, 'docker', self.peer_docker(containers=('cid-a',),
                                                             networks=(), wait_result='1',
                                                             events=(EVENTS_A,),
                                                             transcripts=(None,))), \
             patch.object(launch, 'read_container_logs',
                          return_value=(mirror(EVENTS_A), b'', False)), \
             patch.object(launch, 'docker_capture', fake_capture):
            launch.stop(run)
        data = json.loads(path.read_text())
        record = data['evidence']['ccc-cid-a']
        self.assertEqual(record['exit'], 1)
        self.assertEqual(record['events'], 'captured-live')
        self.assertTrue(any('peer exit 1' in r for r in data['incomplete_reasons']))

    def test_unresponsive_peer_escalates_and_never_invents(self):
        run = '3' * 32
        path = self.write_manifest(run, 'game')
        fake = self.peer_docker(containers=('cid-a',), networks=(),
                                wait_result=RuntimeError('timed out'),
                                events=(None,), transcripts=(None,))
        with patch.object(launch, 'docker', fake), \
             patch.object(launch, 'read_container_logs',
                          side_effect=RuntimeError('log capture failed')), \
             patch.object(launch, 'docker_capture', return_value=(1, '')):
            launch.stop(run)
        data = json.loads(path.read_text())
        record = data['evidence']['ccc-cid-a']
        self.assertEqual(record['exit'], 'forced')
        self.assertEqual(record['events'],
                         'unavailable: peer exited before a complete live capture')
        self.assertTrue(record['logs'].startswith('capture-error'))
        reasons = data['incomplete_reasons']
        self.assertTrue(any('forcibly removed' in r for r in reasons))
        self.assertTrue(any('no complete canonical events' in r for r in reasons))
        self.assertNotIn('game_verdict', data)
        self.assertTrue(any(c[:2] == ('rm', '-f') and 'cid-a' in c for c in self.calls))
        self.assertFalse((launch.STATE / run / 'events' / 'ccc-cid-a.jsonl').exists())

    def test_truncated_transcript_marks_incomplete(self):
        run = 'c' * 32
        self.hosted_manifest(run)
        def fake(*args, **kwargs):
            self.calls.append(args)
            if args[:2] == ('ps', '-aq'):
                return 'cid-a'
            if args[:2] == ('network', 'ls'):
                return ''
            if args[0] == 'wait':
                return '0'
            if args[0] == 'inspect':
                return 'ccc-' + args[-1]
            if args[0] == 'cp':
                if 'agent-transcript.log' in args[1]:
                    Path(args[-1]).write_bytes(b'x' * launch.TRANSCRIPT_QUOTA)
                    return ''
                return None
            return 'ok'
        with patch.object(launch, 'docker', fake), \
             patch.object(launch, 'read_container_logs', return_value=(b'', b'', False)), \
             patch.object(launch, 'docker_capture', return_value=(1, '')), \
             patch.object(launch, 'TRANSCRIPT_QUOTA', 8):
            launch.stop(run)
        data = json.loads((launch.STATE / (run + '.json')).read_text())
        self.assertEqual(data['evidence']['ccc-cid-a']['transcript'], 'truncated')
        self.assertTrue(any('transcript truncated' in r for r in data['incomplete_reasons']))

    def test_validate_events_gate(self):
        self.assertIsNone(launch.validate_events(EVENTS_A))
        self.assertIn('empty', launch.validate_events(b''))
        self.assertIn('terminating newline', launch.validate_events(EVENTS_A.rstrip(b'\n')))
        self.assertIn('not valid JSON', launch.validate_events(b'{oops}\n'))
        self.assertIn('session', launch.validate_events(
            EVENTS_A.replace(b'"type":"session"', b'"type":"move"', 1)))
        no_final = EVENTS_A.replace(
            b'{"type":"final","board":"XOXOXOXOX","ply":9,"outcome":"draw"}\n', b'')
        self.assertIn('final', launch.validate_events(no_final))
        no_stopped = EVENTS_A.replace(
            b'{"type":"stopped","successful":true,"evidence_complete":true}', MOVE)
        self.assertIn('stopped', launch.validate_events(no_stopped))
        self.assertIn('unsuccessful', launch.validate_events(
            EVENTS_A.replace(b'"successful":true', b'"successful":false')))
        self.assertIn('incomplete evidence', launch.validate_events(
            EVENTS_A.replace(b'"evidence_complete":true', b'"evidence_complete":false')))
        after_stopped = EVENTS_A + MOVE + b'\n'
        self.assertIn('final', launch.validate_events(after_stopped))
        self.assertIn('byte limit', launch.validate_events(EVENTS_A + b' ' * launch.EVENTS_MAX_BYTES))

    def test_extract_stdout_events_lossless(self):
        out = b'noise\n' + mirror(EVENTS_A)
        self.assertEqual(launch.extract_stdout_events(out), EVENTS_A)
        trailing = out + b'CCC_EVENT not-json'
        self.assertEqual(launch.extract_stdout_events(trailing), EVENTS_A + b'not-json\n')
        self.assertIsNone(launch.extract_stdout_events(b'no prefixed lines\n'))
        self.assertIsNone(launch.extract_stdout_events(b''))
        # Peers attach via a TTY, whose pty layer terminates lines with CRLF;
        # one trailing carriage return per line is stripped so the mirror is
        # byte-identical to the events file.
        crlf = mirror(EVENTS_A).replace(b'\n', b'\r\n')
        self.assertEqual(launch.extract_stdout_events(crlf), EVENTS_A)
        self.assertEqual(launch.extract_stdout_events(b'noise\r\n' + crlf), EVENTS_A)

    def test_log_streaming_reads_split_streams(self):
        script = ('import sys; sys.stdout.write("out"); sys.stdout.flush(); '
                  'sys.stderr.write("err"); sys.stderr.flush()')
        with patch.object(launch.subprocess, 'Popen', self.fake_popen(script)):
            out, err, truncated = launch.read_container_logs('cid', limit=4096, timeout=30)
        self.assertFalse(truncated)
        self.assertEqual(out, b'out')
        self.assertEqual(err, b'err')

    def test_log_streaming_cap_terminates_capture_cli(self):
        script = 'import sys; sys.stdout.write("x" * 1000000); sys.stdout.flush()'
        with patch.object(launch.subprocess, 'Popen', self.fake_popen(script)):
            out, _err, truncated = launch.read_container_logs('cid', limit=4096, timeout=30)
        self.assertTrue(truncated)
        self.assertLessEqual(len(out), 4096)

    def test_log_streaming_timeout_terminates_capture_cli(self):
        script = 'import time; time.sleep(30)'
        with patch.object(launch.subprocess, 'Popen', self.fake_popen(script)):
            with self.assertRaisesRegex(RuntimeError, 'timed out'):
                launch.read_container_logs('cid', timeout=0.5)

    def fake_popen(self, script):
        real_popen = subprocess.Popen
        def fake(_cmd, **kwargs):
            return real_popen([sys.executable, '-c', script], stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE)
        return fake

    def test_observer_hardening_and_verdict(self):
        calls = []
        def fake_capture(*args, **kwargs):
            calls.append(args)
            return 0, '{"ok": true, "session": 7, "outcome": "draw", "agents_verified": true}'
        with patch.object(launch, 'docker_capture', fake_capture):
            verdict, problem = launch.verify_game_events(
                'a' * 32, 'sha256:' + 'a' * 64, {'ccc-x-a': EVENTS_A, 'ccc-x-b': EVENTS_B}, True, 0)
        self.assertIsNone(problem)
        self.assertEqual(verdict['via'], 'observer-container')
        args = calls[0]
        self.assertEqual(args[0], 'run')
        for flag in ('--rm', '--network', 'none', '--read-only', '--require-agents',
                     '--cgroupns', '/usr/local/bin/ccc-verify-game', f'{launch.LABEL}=' + 'a' * 32):
            self.assertIn(flag, args)
        binds = [a for a in args if a.startswith('type=bind')]
        self.assertEqual(len(binds), 2)
        for bind in binds:
            self.assertTrue(bind.endswith('readonly'))
        with patch.object(launch, 'docker_capture', fake_capture):
            launch.verify_game_events('a' * 32, 'img', {'a': EVENTS_A, 'b': EVENTS_B}, False, 0)
        self.assertNotIn('--require-agents', calls[-1])

    def test_verifier_rejection_records_failure(self):
        def fake_capture(*args, **kwargs):
            return 1, '{"ok": false, "error": "peer boards or move histories disagree"}'
        with patch.object(launch, 'docker_capture', fake_capture):
            verdict, problem = launch.verify_game_events(
                'a' * 32, 'img', {'a': EVENTS_A, 'b': EVENTS_B}, False, 0)
        self.assertEqual(verdict['ok'], False)
        self.assertIn('peer boards', problem)

    def test_host_binary_fallback_and_unverified_absence(self):
        def fake_capture(*args, **kwargs):
            return 1, ''
        with patch.object(launch, 'docker_capture', fake_capture), \
             patch.object(launch.shutil, 'which', return_value=None):
            verdict, problem = launch.verify_game_events(
                'a' * 32, 'img', {'a': EVENTS_A, 'b': EVENTS_B}, False, 0)
        self.assertIsNone(verdict)
        self.assertIn('no parseable verdict', problem)
        result = subprocess.CompletedProcess(args=[], returncode=0,
                                             stdout='{"ok": true, "outcome": "draw"}')
        with patch.object(launch, 'docker_capture', fake_capture), \
             patch.object(launch.shutil, 'which', return_value='/usr/local/bin/ccc-verify-game'), \
             patch.object(launch.subprocess, 'run', return_value=result):
            verdict, problem = launch.verify_game_events(
                'a' * 32, 'img', {'a': EVENTS_A, 'b': EVENTS_B}, False, 0)
        self.assertIsNone(problem)
        self.assertEqual(verdict['via'], 'host-binary')

    def test_redacted_artifacts_count_against_quota(self):
        run = '4' * 32
        path = self.write_manifest(run, 'chat')
        fake = self.peer_docker(containers=('cid-a',), networks=(), events=(None,),
                                transcripts=(None,))
        with patch.object(launch, 'docker', fake), \
             patch.object(launch, 'read_container_logs',
                          return_value=(b'Bearer abcdef123456 secret\n', b'', False)), \
             patch.object(launch, 'RUN_QUOTA', 64):
            launch.stop(run)
        data = json.loads(path.read_text())
        self.assertTrue(data['incomplete'])
        self.assertTrue(any('quota' in r for r in data['incomplete_reasons']))
        total = sum(p.stat().st_size for p in (launch.STATE / run).rglob('*') if p.is_file())
        self.assertLessEqual(total, 64)
        out = (launch.STATE / run / 'logs' / 'ccc-cid-a.stdout').read_text()
        self.assertNotIn('abcdef123456', out)

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

    def test_run_store_jsonl_is_exact_headerless_all_or_nothing(self):
        with patch.object(launch, 'RUN_QUOTA', 256):
            store = launch.RunStore('6' * 32)
            self.assertTrue(store.write_jsonl('events/peer.jsonl', EVENTS_A))
            path = store.dir / 'events' / 'peer.jsonl'
            self.assertEqual(path.read_bytes(), EVENTS_A)
            self.assertFalse(path.read_text().startswith('REVIEW BEFORE SHARING'))
            for line in path.read_text().splitlines():
                json.loads(line)
            self.assertFalse(store.write_jsonl('events/big.jsonl', b'x' * 300))
            self.assertFalse((store.dir / 'events' / 'big.jsonl').exists())
            self.assertTrue(store.incomplete)

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
