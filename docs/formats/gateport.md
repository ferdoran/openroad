# gateport.txt

The gateway port, as ASCII decimal.

Derived from openroad's parser (`parse_gateport`,
`client/src/plugins/config/division.rs`). Upstream reference:
`SilkroadDoc.wiki/Gateport` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/Gateport

## Layout

| size | type | field | notes |
|---|---|---|---|
| 8 | ASCII | `Gateport` | leading decimal digits, NUL-padded |

The parser takes the leading ASCII digits and stops at the first non-digit, so
the NUL padding needs no special case. A file with no leading digit yields no
port, and the configured address is used instead.

Wired 2026-08-12 (#300). The gateway address is
`divisioninfo.divisions[0].gateways[0] + ":" + gateport` —
`filter.example.com:4001` for this build — used whenever `config.yaml`'s
`network_settings.gateway_address` is omitted. That field stays the override,
and `config.example.yaml` still ships the local-stub value, so copying the
example does not redirect anyone's traffic. Independent corroboration of 4001: a
local, uncommitted `config.yaml` carried the same port against its own gateway
host. An absent or digit-less file leaves `gateway_port` as `None`, which falls
back to the configured address.