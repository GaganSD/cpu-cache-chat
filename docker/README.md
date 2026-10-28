# Container packaging

Docker Engine on Linux or OrbStack on macOS is required. `./docker/build.sh` builds the manual image; `./docker/build-agent.sh` builds the separate Node/Pi agent image. These commands build locally and may download dependencies. No build or physical validation is implied by this document.

`scripts/run-pair.sh` starts manual peers with the explicit CPU-contention 50 ms preset and CPUSET=0 by default. Set `CPUSET` to a Linux guest CPU available to Docker: CPU 0 may not be permitted, and a guest vCPU is not a host physical core. Set `CHANNEL=cache` explicitly for the distinct experimental cache workload. Attach commands are printed by the script. Physical compatibility and reliability are unvalidated on new hosts.

`scripts/inspect-isolation.sh container-a container-b` checks configuration, not live isolation or channel operation. It requires nonroot, private namespaces, network none, dropped capabilities, no-new-privileges, a read-only root filesystem, no host/volume mounts, no devices or exposed ports, and resource bounds. Its hosted option checks disjoint dedicated networks but cannot establish that routing is blocked. Live policy remains unverified.

Hosted agents require `CCC_HOSTED_OPT_IN=1`, `OPENCODE_GO_API_KEY`, and the agent image. The separate bridge networks permit general egress; they are not inference-only firewalls. The provider receives board prompts and charges may apply. Offline agents are not provisioned and fail before network creation. The current agent harness cannot verify board equality or live isolation and will not report an overall PASS.

No script here configures host security settings or starts Docker. Containers share a Linux kernel; they are not VMs. Historical 50 ms results are not a guarantee. See `plan.md` for unresolved release and hardware gates.

See the [main quickstart](../README.md). `./docker/build.sh` builds the native base image; `./docker/build-agent.sh` additionally installs Node dependencies with `npm ci`. No model weights or secrets are included. Hosted agents need external inference access and credentials at runtime; bridge networking is not an inference-only firewall.

The Dockerfiles pin verified multi-platform base-image index digests. Reverify and update them under [the release checklist](../docs/release.md), and supply source-revision build arguments. The default revision label is `unknown`, not proof of source identity; neither digest resolution nor labels constitute a validated build.
