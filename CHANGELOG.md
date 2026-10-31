# Changelog

## Unreleased

- Add `game --plain --events-stdout` to copy game events to stdout.
- Use the stable `CCC_EVENT <json>` prefix for those events.
- Allow stdout events together with `--events-path`.
- Stop with a fatal error if the bounded event queue overflows.
- Add reproducible software builds and manual artifact builds.
- Document isolation settings, historical measurements, and hosted inference limits.

Physical measurements do not cover every change to the current wire format.
