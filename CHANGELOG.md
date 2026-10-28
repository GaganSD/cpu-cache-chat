# Changelog

## Unreleased

- Add `game --plain --events-stdout`, mirroring canonical game evidence events as stable `CCC_EVENT <json>` stdout lines (combinable with `--events-path`; overflow of the bounded critical queue is a typed fatal).

- Prepare reproducible software builds and manual release artifacts; physical support for the current wire format remains unvalidated.
- Clarify isolation, historical measurements and hosted inference limitations.
