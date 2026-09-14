---
name: RE ticket
about: Decode an unknown (opcode, format field, constant, table) ahead of implementation
labels: re
---

## Unknown

<!-- What exactly is undecoded, and which epic/ticket it blocks. -->

## Sources to consult (in order)

- [ ] `packet_dump/<opcode>.log` diffing across samples — or corpus probe over own PK2s
- [ ] Public docs (facts only — see CONTRIBUTING.md for what each source class permits)
- [ ] Quarantined static analysis of a client you own — only if the above are insufficient, and the
      evidence stays local (CONTRIBUTING.md § "What this repository does not publish")

## Verification

<!-- How the hypothesis gets falsified: corpus hit rate, fresh capture, in-scene check. -->

## Deliverable

<!-- Where the result lands: the code and its comments, docs/formats/*.md, or the opcode ledger. -->

> Security: SRO-scene binaries are treated as trojaned — never execute or download them; static analysis only; never target live official servers.
