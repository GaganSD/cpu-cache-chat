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


class ProbeReportTests(unittest.TestCase):
    def test_typed_reports(self):
        sender = {'schema_version': 1, 'role': 'A', 'mode': 'Cpu', 'requested_bits': 32,
                  'sent_bytes': 4, 'payload_hex': '01020304'}
        trial = {'outcome': 'decoded', 'error': None, 'decoded_hex': '01020304',
                 'decoded_bytes': 4, 'crc_valid': True}
        receiver = {'schema_version': 1, 'role': 'B', 'mode': 'Cpu', 'requested_bits': 32,
                    'requested_trials': 1, 'attempted_trials': 1, 'completed_trials': 1, 'trials': [trial]}
        with tempfile.TemporaryDirectory() as temp, redirect_stdout(StringIO()):
            a, b = Path(temp) / 'a', Path(temp) / 'b'
            a.write_text(json.dumps(sender))
            b.write_text(json.dumps(receiver))
            probe.main(['compare', str(a), str(b)])
            trial['decoded_hex'] = '01020305'
            b.write_text(json.dumps(receiver))
            with self.assertRaises(ValueError):
                probe.main(['compare', str(a), str(b)])
            trial.update(outcome='silent_no_frame', decoded_hex=None, decoded_bytes=None, crc_valid=None)
            b.write_text(json.dumps(receiver))
            probe.main(['silent', str(b)])
            receiver['completed_trials'] = 0
            b.write_text(json.dumps(receiver))
            with self.assertRaises(ValueError):
                probe.main(['silent', str(b)])


if __name__ == '__main__':
    unittest.main()
