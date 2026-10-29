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


def compare(sender_path, receiver_path):
    a, b = report(sender_path), report(receiver_path)
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


def manifest(directory):
    fields = {}
    for line in (Path(directory) / 'manifest.txt').read_text(encoding='utf-8').splitlines():
        key, sep, value = line.partition('=')
        if sep:
            fields[key] = value
    if fields.get('schema_version') != '1':
        raise ValueError('unsupported control bundle manifest schema')
    if fields.get('result') != 'pass':
        raise ValueError('control bundle manifest is not a recorded pass')
    for key in ('created_utc', 'image', 'image_id', 'channel', 'slot_ms', 'cpuset', 'load_note', 'config_sha256'):
        if not fields.get(key):
            raise ValueError(f'control bundle manifest missing {key}')
    return fields


def main(args):
    if args[0] == 'silent' and len(args) == 2:
        receiver(report(args[1]), 'silent_no_frame')
        print('PASS: silent control observed no frame')
    elif args[0] == 'compare' and len(args) == 3:
        compare(args[1], args[2])
    elif args[0] == 'bundle' and len(args) == 2:
        manifest(args[1])
        receiver(report(Path(args[1]) / 'silent.out'), 'silent_no_frame')
        compare(Path(args[1]) / 'probe-a.out', Path(args[1]) / 'probe-b.out')
        print('PASS: control bundle evidence valid; single qualification run, not a reliability guarantee')
    else:
        raise ValueError('usage: probe-report.py silent REPORT | compare SENDER RECEIVER | bundle DIR')


if __name__ == '__main__':
    try:
        main(sys.argv[1:])
    except (IndexError, KeyError, TypeError, ValueError, OSError) as exc:
        print(f'FAIL: invalid probe evidence: {exc}', file=sys.stderr)
        sys.exit(1)
