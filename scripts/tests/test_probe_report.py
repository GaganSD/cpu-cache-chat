import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO

spec = importlib.util.spec_from_file_location('probe_report', Path(__file__).resolve().parents[1] / 'probe-report.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)

SENDER = {'schema_version': 1, 'role': 'A', 'mode': 'Cpu', 'requested_bits': 32,
          'sent_bytes': 4, 'payload_hex': '01020304'}
DECODED_TRIAL = {'outcome': 'decoded', 'error': None, 'decoded_hex': '01020304',
                 'decoded_bytes': 4, 'crc_valid': True}
SILENT_TRIAL = {'outcome': 'silent_no_frame', 'error': None, 'decoded_hex': None,
                'decoded_bytes': None, 'crc_valid': None}


def receiver(trial):
    return {'schema_version': 1, 'role': 'B', 'mode': 'Cpu', 'requested_bits': 32,
            'requested_trials': 1, 'attempted_trials': 1, 'completed_trials': 1, 'trials': [trial]}


MANIFEST = {
    'schema_version': '1', 'created_utc': '2026-10-29T00:00:00Z', 'image': 'cpu-cache-chat:local',
    'image_id': 'sha256:abc', 'channel': 'cpu', 'slot_ms': '50', 'cpuset': '0',
    'silent_seconds': '61', 'load_note': 'test', 'config_sha256': 'deadbeef',
    'result': 'pass', 'note': 'single qualification run; not a reliability guarantee',
}


class ProbeReportTests(unittest.TestCase):
    def test_typed_reports(self):
        sender = dict(SENDER)
        trial = dict(DECODED_TRIAL)
        receiver_report = receiver(trial)
        with tempfile.TemporaryDirectory() as temp, redirect_stdout(StringIO()):
            a, b = Path(temp) / 'a', Path(temp) / 'b'
            a.write_text(json.dumps(sender))
            b.write_text(json.dumps(receiver_report))
            probe.main(['compare', str(a), str(b)])
            trial['decoded_hex'] = '01020305'
            b.write_text(json.dumps(receiver_report))
            with self.assertRaises(ValueError):
                probe.main(['compare', str(a), str(b)])
            trial.update(outcome='silent_no_frame', decoded_hex=None, decoded_bytes=None, crc_valid=None)
            b.write_text(json.dumps(receiver_report))
            probe.main(['silent', str(b)])
            receiver_report['completed_trials'] = 0
            b.write_text(json.dumps(receiver_report))
            with self.assertRaises(ValueError):
                probe.main(['silent', str(b)])

    def write_bundle(self, directory, manifest_fields=None, trial=None, sender=None):
        d = Path(directory)
        fields = dict(MANIFEST if manifest_fields is None else manifest_fields)
        (d / 'manifest.txt').write_text(''.join(f'{k}={v}\n' for k, v in fields.items()))
        (d / 'silent.out').write_text(json.dumps(receiver(dict(SILENT_TRIAL))))
        (d / 'probe-a.out').write_text(json.dumps(sender or dict(SENDER)))
        (d / 'probe-b.out').write_text(json.dumps(receiver(trial or dict(DECODED_TRIAL))))

    def test_bundle_validation(self):
        with tempfile.TemporaryDirectory() as temp, redirect_stdout(StringIO()):
            self.write_bundle(temp)
            probe.main(['bundle', temp])
            # A manifest that is not a recorded pass invalidates the bundle.
            bad = dict(MANIFEST, result='fail')
            self.write_bundle(temp, manifest_fields=bad)
            with self.assertRaises(ValueError):
                probe.main(['bundle', temp])
            # Missing identity fields invalidate the bundle.
            bad = dict(MANIFEST)
            del bad['config_sha256']
            self.write_bundle(temp, manifest_fields=bad)
            with self.assertRaises(ValueError):
                probe.main(['bundle', temp])
            # Stored probe evidence is re-validated, not trusted.
            bad_trial = dict(DECODED_TRIAL, decoded_hex='01020305')
            self.write_bundle(temp, trial=bad_trial)
            with self.assertRaises(ValueError):
                probe.main(['bundle', temp])
            # A missing manifest invalidates the bundle.
            (Path(temp) / 'manifest.txt').unlink()
            with self.assertRaises(OSError):
                probe.main(['bundle', temp])


if __name__ == '__main__':
    unittest.main()
