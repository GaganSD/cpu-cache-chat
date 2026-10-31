# Container packaging

Use Docker Engine on Linux or OrbStack on an Apple-silicon Mac.

```sh
./docker/build.sh        # Rust chat/game binaries
./docker/build-agent.sh  # Separate Node/Pi agent image
```

Builds are native to the Docker host and use pinned base-image digests and lockfiles. They download dependencies, not model weights. Source-revision labels are useful bookkeeping, not proof that an image matches a checkout.

`scripts/run-pair.sh` starts manual peers with CPU contention, 50 ms slots, and `CPUSET=0`. Change `CPUSET` if that Docker guest CPU isn't available, or set `CHANNEL=cache` for the separate cache workload. The [launcher](../docs/launcher.md) is the easier interactive entry point.

Manual containers have no network, private namespaces, no shared writable mounts, a read-only root, a non-root user, dropped capabilities, and resource limits. `scripts/inspect-isolation.sh` checks configuration; it doesn't prove live isolation.

Hosted agents need the agent image, provider credentials, and `CCC_HOSTED_OPT_IN=1`. Their separate bridges allow general egress, and the provider receives board prompts. See [agent setup](../agent/README.md).

No script here changes host security settings or starts Docker. Containers share a Linux kernel; they aren't VMs.

[Try it](../README.md#try-it) · [Reproduce experiments](../docs/reproducing.md) · [Findings](../docs/measurements.md)
