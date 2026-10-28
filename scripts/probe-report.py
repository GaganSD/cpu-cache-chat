#!/usr/bin/env python3
"""Validate typed physical probe evidence; comparison happens only on the host."""
import json
import sys
from pathlib import Path


def report(path):
    data = json.loads(Path(path).read_text(encoding='utf-8'))
    if data['schema_version'] != 1:
        raise ValueError('unsupported probe schema')
    return data


def receiver(data, outcome):
    if data['role'] != 'B' or data['requested_trials'] != 1 or data['attempted_trials'] != 1 or data['completed_trials'] != 1:
        raise ValueError('incomplete receiver report')
    trial, = data['trials']
    if trial['outcome'] != outcome or trial['error'] is not None:
        raise ValueError('unexpected probe outcome')
    return trial


def main(args):
    if args[0] == 'silent' and len(args) == 2:
        receiver(report(args[1]), 'silent_no_frame')
        print('PASS: silent control observed no frame')
    elif args[0] == 'compare' and len(args) == 3:
        a, b = report(args[1]), report(args[2])
        trial = receiver(b, 'decoded')
        if a['role'] != 'A' or a['mode'] != b['mode'] or a['requested_bits'] != b['requested_bits']:
            raise ValueError('sender and receiver reports do not match')
        expected = bytes.fromhex(a['payload_hex'])
        actual = bytes.fromhex(trial['decoded_hex'])
        if not expected or len(expected) * 8 != a['requested_bits'] or a['sent_bytes'] != len(expected) or trial['decoded_bytes'] != len(actual) or trial['crc_valid'] is not True:
            raise ValueError('invalid payload or CRC')
        errors = sum((x ^ y).bit_count() for x, y in zip(expected, actual)) + 8 * abs(len(expected) - len(actual))
        bits = 8 * max(len(expected), len(actual))
        print(f'Post-decode comparison: bit errors={errors}/{bits}; not raw carrier BER')
        if errors:
            raise ValueError('payload mismatch')
    else:
        raise ValueError('usage: probe-report.py silent REPORT | compare SENDER RECEIVER')


if __name__ == '__main__':
    try:
        main(sys.argv[1:])
    except (IndexError, KeyError, TypeError, ValueError, OSError) as exc:
        print(f'FAIL: invalid probe evidence: {exc}', file=sys.stderr)
        sys.exit(1)
