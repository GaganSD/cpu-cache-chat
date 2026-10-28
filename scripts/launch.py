#!/usr/bin/env python3
"""Owner-scoped Docker demo launcher; never forwards peer data."""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
STATE = Path(os.environ.get('CCC_RUN_DIR', Path.home() / '.cpu-cache-chat' / 'runs'))
LABEL = 'org.cpu-cache-chat.run'
PRESET = 'reference-50ms'
SLOT = 50
EVENTS_PATH = '/tmp/game.events.jsonl'
TRANSCRIPT_PATH = '/tmp/agent-transcript.log'
# The agent adapter bounds the raw transcript itself; reaching the bound means truncation.
TRANSCRIPT_QUOTA = 16 * 1024 * 1024
RUN_QUOTA = 64 * 1024 * 1024
MAX_RETAINED_RUNS = 8
RETENTION_BUDGET = 8 * RUN_QUOTA
ARTIFACT_HEADER = ('REVIEW BEFORE SHARING: best-effort redaction only; '
                   'NOT guaranteed secret-free.\n')
SECRET_NAME = re.compile(r'(?i)(api[_-]?key|token|secret|password|credential)')
SECRET_VALUE = (
    (re.compile(r'(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{4,}'), r'\1[REDACTED]'),
    (re.compile(r'(?i)(\b(?:api[_-]?key|api[_-]?secret|access[_-]?token|auth[_-]?token|'
                r'refresh[_-]?token|secret[_-]?key|password|authorization)\b["\']?\s*'
                r'[:=]\s*["\']?)[^\s"\',;}]+'), r'\1[REDACTED]'),
)


def docker(*args, timeout=30, check=True):
    try:
        p = subprocess.run(['docker', *args], text=True, stdout=subprocess.PIPE,
                           stderr=subprocess.DEVNULL, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise RuntimeError(f'Docker unavailable or timed out: {type(e).__name__}') from e
    if check and p.returncode:
        raise RuntimeError(f'Docker {args[0]} failed (exit {p.returncode})')
    return p.stdout.strip() if p.returncode == 0 else None


def owned(kind, run):
    command = ['ps', '-aq'] if kind == 'container' else ['network', 'ls', '-q']
    ids = docker(*command, '--filter', f'label={LABEL}={run}') or ''
    return ids.splitlines()


def cleanup(run):
    # The random run ID and label filter, never names, define ownership.
    errors = []
    for kind in ('container', 'network'):
        for ident in owned(kind, run):
            try:
                docker('rm', '-f', ident) if kind == 'container' else docker('network', 'rm', ident)
            except RuntimeError as e:
                errors.append(str(e))
    if errors:
        raise RuntimeError('; '.join(errors))


def image_id(image):
    value = docker('image', 'inspect', '--format', '{{.Id}}', image)
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', value or ''):
        raise RuntimeError('image inspection did not return an immutable image ID')
    return value


def guest_cpu(cpu):
    n = docker('info', '--format', '{{.NCPU}}')
    if not n or not n.isdigit() or cpu >= int(n):
        raise RuntimeError(f'CPU {cpu} unavailable in Docker guest (NCPU={n})')


def manifest_path(run):
    if not re.fullmatch(r'[0-9a-f]{32}', run):
        raise RuntimeError('invalid run ID')
    return STATE / (run + '.json')


def save_manifest(path, data):
    with path.open('w', encoding='utf-8') as f:
        os.chmod(path, 0o600)
        json.dump(data, f)
        f.write('\n')


def redact(text):
    """Best-effort secret removal; artifacts remain review-before-sharing."""
    for key, value in os.environ.items():
        if value and len(value) >= 6 and SECRET_NAME.search(key):
            text = text.replace(value, '[REDACTED]')
    for pattern, repl in SECRET_VALUE:
        text = pattern.sub(repl, text)
    return text


def retention_check():
    """Refuse new launches beyond the count/byte budget; never deletes anything."""
    if not STATE.is_dir():
        return
    runs = [p.stem for p in STATE.glob('*.json') if re.fullmatch(r'[0-9a-f]{32}', p.stem)]
    total = 0
    for run in runs:
        total += (STATE / (run + '.json')).stat().st_size
        directory = STATE / run
        if directory.is_dir():
            total += sum(p.stat().st_size for p in directory.rglob('*') if p.is_file())
    if len(runs) >= MAX_RETAINED_RUNS or total >= RETENTION_BUDGET:
        raise RuntimeError(
            f'retention budget reached ({len(runs)}/{MAX_RETAINED_RUNS} runs, '
            f'{total}/{RETENTION_BUDGET} bytes); remove retained runs under {STATE} manually')


def image_provenance(image):
    """Best-effort image source labels and binary provenance; never inspects Env."""
    provenance = {}
    try:
        raw = docker('image', 'inspect', '--format', '{{json .Config.Labels}}', image)
        labels = json.loads(raw) if raw else None
        if isinstance(labels, dict):
            for key, field in (('org.opencontainers.image.source', 'image_source'),
                               ('org.opencontainers.image.revision', 'image_revision')):
                if isinstance(labels.get(key), str):
                    provenance[field] = labels[key]
    except (RuntimeError, ValueError):
        pass
    try:
        out = docker('run', '--rm', '--network', 'none', '--read-only',
                     '--user', '65532:65532', image, '--version', timeout=60, check=False)
        if out:
            provenance['binary_version'] = out[:256]
    except RuntimeError:
        pass
    return provenance


class RunStore:
    """Bounded writer for one retained run directory; never deletes anything."""

    def __init__(self, run):
        self.dir = STATE / run
        self.used = 0
        self.incomplete = []

    def write(self, rel, text):
        data = (ARTIFACT_HEADER + text).encode('utf-8', 'replace')
        budget = RUN_QUOTA - self.used
        if budget <= 0:
            self.incomplete.append(f'{rel}: per-run {RUN_QUOTA}-byte quota exhausted')
            return False
        full = len(data) <= budget
        if not full:
            data = data[:budget]
            self.incomplete.append(f'{rel}: truncated to fit per-run {RUN_QUOTA}-byte quota')
        path = self.dir / rel
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.write_bytes(data)
        os.chmod(path, 0o600)
        self.used += len(data)
        return full


def read_container_logs(ident):
    """Split stdout/stderr; TTY containers merge both streams into stdout."""
    try:
        p = subprocess.run(['docker', 'logs', ident], stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise RuntimeError(f'log capture failed: {type(e).__name__}') from e
    if p.returncode:
        raise RuntimeError('log capture failed')
    return p.stdout.decode('utf-8', 'replace'), p.stderr.decode('utf-8', 'replace')


def read_remote_file(ident, remote):
    """docker cp one in-container file; None when absent (no invented record)."""
    with tempfile.TemporaryDirectory() as tmp:
        target = Path(tmp) / 'artifact'
        if docker('cp', f'{ident}:{remote}', str(target), check=False) is None or not target.is_file():
            return None
        # Bounded by the container's 64 MiB tmpfs.
        return target.read_bytes()


def capture(run, store, hosted):
    """Capture split logs and session files from owned containers before removal."""
    evidence = {}
    for ident in owned('container', run):
        safe = ident
        try:
            name = docker('inspect', '--format', '{{.Name}}', ident)
            if name:
                safe = re.sub(r'[^A-Za-z0-9_.-]+', '_', name.lstrip('/'))
            out, err = read_container_logs(ident)
            store.write(f'logs/{safe}.stdout', redact(out))
            store.write(f'logs/{safe}.stderr', redact(err))
            record = {'logs': 'captured'}
            events = read_remote_file(ident, EVENTS_PATH)
            if events is None:
                record['events'] = 'missing'
            else:
                store.write(f'events/{safe}.jsonl', redact(events.decode('utf-8', 'replace')))
                record['events'] = 'captured'
            if hosted:
                raw = read_remote_file(ident, TRANSCRIPT_PATH)
                if raw is None:
                    record['transcript'] = 'missing'
                else:
                    store.write(f'transcripts/{safe}.log', redact(raw.decode('utf-8', 'replace')))
                    record['transcript'] = ('truncated' if len(raw) >= TRANSCRIPT_QUOTA
                                            else 'captured')
            evidence[safe] = record
        except RuntimeError as e:
            evidence[safe] = {'logs': f'capture-error: {e}'}
    return evidence


def incomplete_reasons(store, evidence, hosted):
    reasons = list(store.incomplete)
    for name, record in evidence.items():
        for key, value in sorted(record.items()):
            if str(value).startswith('capture-error'):
                reasons.append(f'{name}: {value}')
        if record.get('events') != 'captured':
            reasons.append(f'{name}: events {record.get("events", "not captured")}')
        if hosted and record.get('transcript') != 'captured':
            reasons.append(f'{name}: transcript {record.get("transcript", "not captured")}')
    return reasons


def stop(run):
    path = manifest_path(run)
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as e:
        raise RuntimeError('run manifest unavailable or invalid') from e
    if data.get('run_id') != run or data.get('label') != LABEL:
        raise RuntimeError('run manifest ownership mismatch')
    hosted = data.get('mode') == 'agents'
    remaining = owned('container', run) + owned('network', run)
    if not remaining and data.get('status') in ('stopped', 'failed'):
        print(f'Run {run} already {data["status"]}; artifacts retained under {STATE / run}')
        return
    store = RunStore(run)
    evidence = capture(run, store, hosted)
    cleanup(run)
    merged = data.get('evidence') or {}
    merged.update(evidence)
    data['evidence'] = merged
    if data.get('status') != 'failed':
        data['status'] = 'stopped'
    data['end'] = int(time.time())
    reasons = incomplete_reasons(store, evidence, hosted)
    if reasons:
        data['incomplete'] = True
        data['incomplete_reasons'] = sorted(set(data.get('incomplete_reasons', []) + reasons))
    save_manifest(path, data)
    print(f'Stopped run {run}; artifacts retained under {store.dir}')
    print('Artifacts: best-effort redaction; REVIEW BEFORE SHARING; not guaranteed secret-free.')


def common(run, cpu):
    return ['--label', f'{LABEL}={run}', '--init', '--ipc', 'private', '--read-only',
            '--tmpfs', '/tmp:rw,nosuid,size=64m', '--security-opt', 'no-new-privileges',
            '--cap-drop', 'ALL', '--user', '65532:65532', '--memory', '768m',
            '--pids-limit', '128', '--cpuset-cpus', str(cpu)]


def parse_probe_reports(sender, receiver):
    """Validate versioned, receiver-blind probe reports after both processes exit."""
    try:
        a, b = json.loads(sender), json.loads(receiver)
        if a['schema_version'] != 1 or b['schema_version'] != 1 or a['role'].upper() != 'A':
            raise ValueError('unsupported report')
        if a['requested_bits'] != 32 or b['requested_bits'] != 32:
            raise ValueError('unexpected bit count')
        if b['completed_trials'] != 1 or b['attempted_trials'] != 1 or b['requested_trials'] != 1:
            raise ValueError('incomplete trials')
        trial, = b['trials']
        if trial['outcome'] != 'decoded' or trial['crc_valid'] is not True or trial['decoded_bytes'] != 4:
            raise ValueError('invalid trial')
        expected = bytes.fromhex(a['payload_hex'])
        actual = bytes.fromhex(trial.get('decoded_bytes_hex', trial.get('decoded_hex', '')))
        if len(expected) != 4 or expected != actual:
            raise ValueError('payload mismatch')
    except (KeyError, TypeError, ValueError, AttributeError) as e:
        raise RuntimeError('physical preflight report invalid or payload mismatch') from e


LISTENER = "require('net').createServer(s=>s.end()).listen(43719,'0.0.0.0')"
CONTROL = """const s=require('net').connect({host:process.argv[1],port:43719});
let done=false; function finish(x){if(!done){done=true; console.log(x); s.destroy();}}
s.setTimeout(4000,()=>finish('ETIMEDOUT')); s.on('connect',()=>finish('CONNECTED'));
s.on('error',e=>finish(e.code||'ERROR'));"""


def verify_peer_paths(run, image, cpu, nets):
    """Test only disposable listeners; all controls are removed even on failure."""
    created = []
    try:
        for net in nets:
            raw = docker('network', 'inspect', net)
            info = json.loads(raw)
            if len(info) != 1 or info[0].get('EnableIPv6') is not False:
                raise RuntimeError('IPv6 enabled or unknown; live verification refused')
        addresses = []
        for role, net in zip(('a', 'b'), nets):
            name = f'ccc-{run}-listener-{role}'
            docker('create', '--name', name, '--network', net, *common(run, cpu),
                   '--entrypoint', 'node', image, '-e', LISTENER)
            created.append(name)
            docker('start', name)
            raw = docker('inspect', '--format', '{{json .NetworkSettings.Networks}}', name)
            address = json.loads(raw)[net]['IPAddress']
            if not address:
                raise RuntimeError('listener IP unavailable')
            addresses.append(address)
        def control(source, target, label):
            name = f'ccc-{run}-control-{label}'
            docker('create', '--name', name, '--network', nets[source], *common(run, cpu),
                   '--entrypoint', 'node', image, '-e', CONTROL, addresses[target])
            created.append(name)
            return docker('start', '-a', name, timeout=15)
        # Allow Node to bind; refusal remains an inconclusive failure.
        time.sleep(1)
        for i, role in enumerate(('a', 'b')):
            result = control(i, i, 'positive-' + role)
            if result != 'CONNECTED':
                raise RuntimeError(f'positive {role} listener control inconclusive: {result}')
        results = [control(0, 1, 'a-to-b'), control(1, 0, 'b-to-a')]
        if any(result not in ('ENETUNREACH', 'EHOSTUNREACH', 'ETIMEDOUT') for result in results):
            raise RuntimeError('cross-peer control not explicitly denied or timed out')
        return {'version': 1, 'ipv6': 'disabled', 'positive': ['connected', 'connected'],
                'a_to_b': results[0], 'b_to_a': results[1], 'scope': 'listener-ip-tcp'}
    finally:
        errors = []
        for name in reversed(created):
            try:
                docker('rm', '-f', name)
            except RuntimeError as e:
                errors.append(str(e))
        if errors:
            raise RuntimeError('disposable control cleanup incomplete: ' + '; '.join(errors))


def preflight(run, image, channel, cpu):
    # Receiver-blind comparison happens on the host only after both peers exit.
    base = ['docker', 'run', '--network', 'none', *common(run, cpu), image,
            'probe', '--channel', channel, '--slot-ms', str(SLOT), '--bits', '32']
    processes = []
    try:
        for role in ('b', 'a'):
            processes.append(subprocess.Popen([*base, '--role', role], stdout=subprocess.PIPE,
                                              stderr=subprocess.DEVNULL, text=True))
            if role == 'b':
                time.sleep(2)
        deadline = time.monotonic() + 300
        outputs = []
        for p in processes:
            out, _ = p.communicate(timeout=max(0.1, deadline - time.monotonic()))
            if p.returncode:
                raise RuntimeError('physical preflight process failed')
            outputs.append(out)
        parse_probe_reports(outputs[1], outputs[0])
        print('Preflight: matching receiver-blind 32-bit probe; not a reliability guarantee.')
    except subprocess.TimeoutExpired as e:
        raise RuntimeError('physical preflight timed out after 300 seconds') from e
    finally:
        for p in processes:
            if p.poll() is None:
                p.kill()
            p.wait()
        cleanup(run)


def launch(args):
    if args.cpu < 0:
        raise RuntimeError('CPU must be nonnegative')
    hosted = args.command == 'agents'
    envargs = []
    model = None
    if hosted:
        if os.environ.get('CCC_HOSTED_OPT_IN') != '1':
            raise RuntimeError('Set CCC_HOSTED_OPT_IN=1 to permit paid hosted inference and disclosure of board prompts')
        if args.provider == 'opencode-go':
            key = 'OPENCODE_GO_API_KEY'
            model = 'deepseek-v4.1-flash'
        else:
            key = 'AI_GATEWAY_API_KEY'
            if not os.environ.get(key) and os.environ.get('VERCEL_AI_GATEWAY_API_KEY'):
                os.environ[key] = os.environ['VERCEL_AI_GATEWAY_API_KEY']
            model = 'reflection/beam-free'
        if not os.environ.get(key):
            raise RuntimeError(f'{key} is required')
        os.environ['CCC_AGENT_PROVIDER'] = args.provider
        os.environ['CCC_AGENT_MODEL'] = model
        envargs = ['--env', 'CCC_AGENT_PROVIDER', '--env', 'CCC_AGENT_MODEL', '--env', key,
                   '--env', f'CCC_AGENT_TRANSCRIPT_PATH={TRANSCRIPT_PATH}',
                   '--env', 'CCC_AGENT_DEBUG=1']
    retention_check()
    docker('info')
    guest_cpu(args.cpu)
    image = image_id(args.image or ('cpu-cache-chat:agent' if hosted else 'cpu-cache-chat:local'))
    run = secrets.token_hex(16)
    path = manifest_path(run)
    STATE.mkdir(mode=0o700, parents=True, exist_ok=True)
    manifest = {'run_id': run, 'label': LABEL, 'image_id': image, **image_provenance(image),
                'mode': args.command, 'channel': args.channel, 'preset': PRESET, 'slot_ms': SLOT,
                'cpu': args.cpu, 'provider': args.provider if hosted else None, 'model': model,
                'status': 'starting', 'start': int(time.time()), 'end': None,
                'probe': 'skipped' if args.skip_preflight else 'pending',
                'events_path': EVENTS_PATH, 'transcript_path': TRANSCRIPT_PATH if hosted else None}
    with path.open('x', encoding='utf-8') as f:
        os.chmod(path, 0o600)
        json.dump(manifest, f)
        f.write('\n')
    def interrupted(signum, _frame):
        raise RuntimeError(f'interrupted by signal {signum}')
    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    try:
        # Starting a bounded disposable container verifies the guest daemon's
        # cpuset/cgroup restrictions as well as its advertised CPU count.
        docker('run', '--rm', '--network', 'none', *common(run, args.cpu),
               '--entrypoint', '/bin/sh', image, '-c', 'exit 0', timeout=30)
        if args.skip_preflight:
            print('WARNING: physical preflight explicitly skipped; hardware channel unverified.')
        else:
            preflight(run, image, args.channel, args.cpu)
            manifest['probe'] = 'passed'
            save_manifest(path, manifest)
        names = [f'ccc-{run}-{role}' for role in ('a', 'b')]
        nets = ['none', 'none']
        if hosted:
            print('WARNING: separate egress bridges are not destination firewalls.')
            nets = [f'ccc-{run}-net-{role}' for role in ('a', 'b')]
            for net in nets:
                docker('network', 'create', '--label', f'{LABEL}={run}', net)
            manifest['peer_path_test'] = verify_peer_paths(run, image, args.cpu, nets)
            save_manifest(path, manifest)
            print('Live controls: both same-network listeners reachable; A-to-B and B-to-A listener IP TCP paths denied or timed out; IPv6 disabled.')
        mode = 'game' if hosted else args.command
        for role, name, net in zip(('a', 'b'), names, nets):
            docker('create', '-it', '--name', name, '--network', net, *common(run, args.cpu),
                   *envargs, image, mode, '--role', role, '--channel', args.channel,
                   '--slot-ms', str(SLOT), '--events-path', EVENTS_PATH,
                   *(['--agent'] if hosted else []))
        check = [str(ROOT / 'scripts' / 'inspect-isolation.sh')]
        if hosted:
            check.append('--hosted')
        subprocess.run([*check, *names], check=True, timeout=30)
        manifest['isolation_config'] = 'passed-configuration-only'
        for name in names:
            docker('start', name)
        manifest['status'] = 'running'
        save_manifest(path, manifest)
        print(f'Run ID: {run}; preset: {PRESET}; channel: {args.channel}; image: {image}')
        print(f'Attach in separate terminals: docker attach {names[0]}   and   docker attach {names[1]}')
        print('Detach without stopping: Ctrl-p Ctrl-q')
        print(f'Stop: scripts/demo.sh stop --run-id {run}')
        print(f'On stop, bounded evidence is retained under {STATE / run} (review before sharing; not guaranteed secret-free).')
        print('50 ms slots: a 19-byte control frame takes about 20 seconds; retries and games can take minutes. No delivery guarantee.')
    except BaseException as e:
        try:
            store = RunStore(run)
            evidence = capture(run, store, hosted)
            cleanup(run)
            if evidence:
                manifest['evidence'] = evidence
            manifest['status'] = 'failed'
            manifest['end'] = int(time.time())
            manifest['error'] = redact(str(e))[:500]
            reasons = incomplete_reasons(store, evidence, hosted)
            if reasons:
                manifest['incomplete'] = True
                manifest['incomplete_reasons'] = reasons
            save_manifest(path, manifest)
            print(f'Run failed; manifest and any captured artifacts retained under {STATE}.', file=sys.stderr)
        except Exception as cleanup_error:
            print(f'Cleanup incomplete; retain manifest and retry stop: {cleanup_error}', file=sys.stderr)
        raise


def doctor():
    info = json.loads(docker('info', '--format', '{{json .}}'))
    arch = info.get('Architecture', 'unknown')
    memory = info.get('MemTotal', 0)
    cpus = info.get('NCPU', 0)
    print(f'Docker guest: architecture={arch}, memory={memory} bytes, CPUs={cpus}')
    if arch not in ('arm64', 'aarch64', 'x86_64', 'amd64'):
        print('WARNING: unsupported or unknown guest architecture')
    if not isinstance(memory, int) or memory < 2 * 768 * 1024 * 1024:
        print('WARNING: guest memory is below the two-container limit (1.5 GiB); allow additional overhead')
    for image, command in (('cpu-cache-chat:local', './docker/build.sh'),
                           ('cpu-cache-chat:agent', './docker/build-agent.sh')):
        if docker('image', 'inspect', image, check=False) is None:
            print(f'Missing image {image}; build: {command}')
        else:
            print(f'Image available: {image}')
    for provider, keys in (('opencode-go', ('OPENCODE_GO_API_KEY',)),
                           ('vercel-ai-gateway', ('AI_GATEWAY_API_KEY', 'VERCEL_AI_GATEWAY_API_KEY'))):
        print(f'Provider {provider}: credential ' + ('present' if any(os.environ.get(k) for k in keys) else 'missing (' + ' or '.join(keys) + ')'))
    print('Hosted runs also require CCC_HOSTED_OPT_IN=1; credentials and provider reachability are not verified.')
    print('Physical channel and live network policy are not tested by doctor.')


def main():
    parser = argparse.ArgumentParser(description='Isolated physical-channel demo launcher')
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('doctor')
    stop_parser = sub.add_parser('stop')
    stop_parser.add_argument('--run-id', required=True)
    for mode in ('chat', 'game', 'agents'):
        p = sub.add_parser(mode)
        p.add_argument('--channel', choices=('cpu', 'cache'), required=True)
        p.add_argument('--cpu', type=int, default=0, help='Docker guest CPU index (default 0)')
        p.add_argument('--image', help='local image name; resolved to immutable ID')
        p.add_argument('--skip-preflight', action='store_true', help='explicitly skip bounded physical probe')
        if mode == 'agents':
            p.add_argument('--provider', choices=('opencode-go', 'vercel-ai-gateway'), required=True)
    args = parser.parse_args()
    if args.command == 'doctor':
        doctor()
    elif args.command == 'stop':
        stop(args.run_id)
    else:
        launch(args)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError) as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        sys.exit(1)
