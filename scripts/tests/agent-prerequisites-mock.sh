#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
printf '%s\n' '#!/usr/bin/env bash' 'echo "unexpected Docker call" >&2; exit 99' > "$tmp/bin/docker"
chmod +x "$tmp/bin/docker"
export PATH="$tmp/bin:$PATH"
for provider in llama unknown; do
  if CCC_AGENT_PROVIDER="$provider" CCC_HOSTED_OPT_IN=1 OPENCODE_GO_API_KEY=x EVIDENCE_DIR="$tmp/evidence" "$root/scripts/e2e-docker-agent.sh" >"$tmp/out" 2>&1; then exit 1; fi
  grep -q 'unsupported' "$tmp/out"
  if grep -q 'unexpected Docker call' "$tmp/out"; then exit 1; fi
  [[ ! -e "$tmp/evidence" ]]
done
if CCC_AGENT_PROVIDER=opencode-go CCC_HOSTED_OPT_IN=0 OPENCODE_GO_API_KEY=x "$root/scripts/e2e-docker-agent.sh" >"$tmp/out" 2>&1; then exit 1; fi
grep -q CCC_HOSTED_OPT_IN "$tmp/out"
if CCC_AGENT_PROVIDER=opencode-go CCC_HOSTED_OPT_IN=1 OPENCODE_GO_API_KEY='' "$root/scripts/e2e-docker-agent.sh" >"$tmp/out" 2>&1; then exit 1; fi
grep -q OPENCODE_GO_API_KEY "$tmp/out"
if grep -q 'unexpected Docker call' "$tmp/out"; then exit 1; fi
echo 'PASS agent prerequisites fail before Docker'
