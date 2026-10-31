# Container packaging

Use Docker Engine on Linux or OrbStack on an Apple-silicon Mac.

## Build images

Build the manual chat and game image:

```sh
./docker/build.sh
```

Build the separate Node/Pi agent image:

```sh
./docker/build-agent.sh
```

Builds use the Docker host's native architecture.
The Dockerfiles pin base-image digests and use dependency lockfiles.
They download dependencies, not model weights.

Source-revision labels record build inputs.
They do not prove that an image matches a checkout.

## Start peers

The [launcher](../docs/launcher.md) provides the interactive commands.

The lower-level `scripts/run-pair.sh` defaults to:

| Setting | Value |
| --- | --- |
| Channel | CPU contention |
| Slot duration | 50 ms |
| CPU assignment | `CPUSET=0` |

Change `CPUSET` if Docker does not allow guest CPU 0.
Set `CHANNEL=cache` to select the cache workload.

## Manual containers

Manual containers use:

- No network.
- Private namespaces.
- No shared writable mounts.
- A read-only root filesystem.
- A non-root user.
- Dropped capabilities.
- Resource limits.

`scripts/inspect-isolation.sh` checks these settings.
It does not prove live isolation.

## Hosted agents

Hosted agents require:

- The agent image.
- Provider credentials.
- `CCC_HOSTED_OPT_IN=1`.

Each peer uses a separate bridge with general egress.
The provider receives board prompts.
See [agent setup](../agent/README.md).

The scripts do not change host security settings or start Docker.
Containers share a Linux kernel.
They are not virtual machines.

[Getting Started](../README.md#getting-started) · [Reproduce experiments](../docs/reproducing.md) · [Findings](../docs/measurements.md)
