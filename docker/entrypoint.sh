#!/bin/sh
set -eu
# No peer network is assumed. Loopback stays available inside the container.
exec /usr/local/bin/cpu-cache-chat "$@"
