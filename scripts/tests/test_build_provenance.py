import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BuildProvenance(unittest.TestCase):
    def test_agent_image_override_does_not_replace_base_tag(self):
        with tempfile.TemporaryDirectory(prefix='ccc-build-') as directory:
            path = Path(directory)
            (path / 'docker').write_text('''#!/usr/bin/env python3
import json, os, sys
with open(os.environ['DOCKER_CALLS'], 'a') as stream:
    stream.write(json.dumps(sys.argv[1:]) + '\\n')
if sys.argv[1:3] == ['image', 'inspect']:
    print('sha256:' + 'a' * 64 if '.Id' in sys.argv[4] else 'fixture-revision')
''')
            (path / 'docker').chmod(0o755)
            env = dict(os.environ, PATH=str(path) + os.pathsep + os.environ['PATH'],
                       DOCKER_CALLS=str(path / 'calls'), IMAGE='custom:agent', BASE_IMAGE='custom:base')
            subprocess.run(['bash', str(ROOT / 'docker/build-agent.sh')], env=env, check=True)
            calls = [json.loads(line) for line in (path / 'calls').read_text().splitlines()]
            builds = [call for call in calls if call[0] == 'build']
            self.assertEqual(len(builds), 2)
            self.assertEqual(builds[0][builds[0].index('-t') + 1], 'custom:base')
            self.assertEqual(builds[1][builds[1].index('-t') + 1], 'custom:agent')
            self.assertIn('CCC_IMAGE=custom:base', builds[1])
            self.assertIn('VCS_REF=fixture-revision', builds[1])
            self.assertTrue(any(arg.startswith('VCS_REF=') for arg in builds[0]))


if __name__ == '__main__':
    unittest.main()
