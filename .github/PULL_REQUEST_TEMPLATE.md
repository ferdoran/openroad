## Summary

<!-- What does this change do, and why? Link the ticket: Closes #NN / Part of EP-XX. -->

## Evidence

<!-- How do you know it is correct? File paths, packet_dump lines (redacted),
     corpus-probe hit rates, docs/formats or docs/net-*.md citations, screenshots
     of the relevant scene where visual. -->

## Definition of Done

- [ ] `cargo fmt --all` — no diff
- [ ] `make warnings` green (warning policy: no warnings except unread SRO data fields)
- [ ] `cargo test --workspace` green
- [ ] `cargo build --package client` succeeds
- [ ] Visual/gameplay change verified in a scene (`make run <scene>`) — or n/a
- [ ] Docs updated where behavior/formats/protocol changed (`docs/`, `docs/formats/`, `docs/net-*.md`)
- [ ] No SRO assets added, linked, or committed; no credentials in dumps/docs
- [ ] External source used? Cite it and its license class (`CONTRIBUTING.md` / `AGENTS.md`)
