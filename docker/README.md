# Container packaging

Use Docker Engine on Linux or OrbStack on macOS. Build natively for the Linux architecture: ARM64 is the first target for Apple silicon and ARM Linux; x86-64 Linux can use the same Dockerfile. The image contains the Rust release binary and agent source stub only. It does not install Pi, Node, llama.cpp, model weights, or secrets. Agent-backed games need separate future provisioning; no model is downloaded here.

```sh
./docker/build.sh
CHANNEL=cache ./scripts/run-pair.sh  # or CHANNEL=cpu; MODE=game is also available
./scripts/inspect-isolation.sh
```

Attach in separate terminals with `docker attach ccc-a` and `docker attach ccc-b`. Detach with Ctrl-p Ctrl-q; stop and remove with `docker rm -f ccc-a ccc-b`. Existing containers are not replaced. `IMAGE` selects another image tag. Set `CPUSET`, for example `CPUSET=0,1`, to apply an optional Docker CPU set to both players; without it the runtime chooses CPUs. On OrbStack, Linux guest CPUs are virtual CPUs and this does not pin host physical cores. Hardware sharing and timing are not guaranteed by a CPU set.

For separate cache and CPU measurements, stop the interactive pair first to avoid extra load, then run:

```sh
./scripts/probe-pair.sh
# Optional: CPUSET=0,1 TRIALS=1 BITS=64 SLOT_MS=2 ./scripts/probe-pair.sh
```

The script launches independent sender and receiver containers for each channel and prints their actual output. A two-second receiver head start is only a startup heuristic, not synchronization. The current probe performs a single receive attempt despite reporting the `trials` parameter; its receiver cannot verify the sender's payload and reports `usable: false` without an expected payload. A successful process exit is not evidence of a reliable channel. Repeat and evaluate measurements and controls separately; cache and CPU results are distinct.

These are containers sharing the Linux kernel, not microVMs. On macOS that kernel runs in OrbStack. The launch uses no network, private IPC, separate writable filesystems, no Docker socket or shared writable mounts, no privileged mode, and no host relay. The inspector checks the runtime configuration, including overlapping writable bind or volume sources, but does not prove that all possible covert paths are absent or that a physical channel works. Image layers are shared read-only. `docker info` must succeed; these scripts do not start or reconfigure OrbStack.
