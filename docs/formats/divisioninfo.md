# divisioninfo.txt

The shard list: a locale id and, per division, its gateway hosts.

Derived from openroad's parser (`DivisionInfo::parse`,
`client/src/plugins/config/division.rs`). Upstream reference:
`SilkroadDoc.wiki/DivisionInfo` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/DivisionInfo

## Layout

Every string is a `u32` byte length, the bytes, then a **NUL terminator that is
part of the record** — the parser rejects the file if that byte is not zero,
because every later offset would otherwise be wrong.

| size | type | field | notes |
|---|---|---|---|
| 1 | u8 | `ContentID` | locale |
| 1 | u8 | `DivisionCount` | |

Then `DivisionCount` records:

| size | type | field |
|---|---|---|
| 4 + n + 1 | u32 + bytes + NUL | `Division.Name` |
| 1 | u8 | `GatewayCount` |
| (4 + n + 1) × count | u32 + bytes + NUL | `Gateway` host |

A gateway entry is a hostname or an IP; openroad resolves it via DNS at connect
time. The effective address is the first gateway of the first division, joined
with the port from `gateport.txt`.

## Byte verification (openroad, 2026-08-12)

`Media/divisioninfo.txt` is **binary despite the `.txt` extension**; the layout
above parses it 35/35 bytes, EOF-exact:

```
16 01 05000000 "DIV01" 00 01 11000000 "filter.example.com" 00
^  ^  ^                ^  ^  ^        ^                   ^
|  |  |                |  |  |        |                   NUL terminator
|  |  |                |  |  |        gateway address (ASCII, may be a hostname)
|  |  |                |  |  u32 AddrLen = 17
|  |  |                |  u8 GatewayCount = 1
|  |  |                u8 NUL
|  |  u32 NameLen = 5 + "DIV01"
|  u8 DivisionCount = 1
u8 ContentID = 22
```

The `//'0'` comment in the wiki sources is a typo — the separator is byte
`0x00`, not ASCII `'0'` (0x30).

**Byte 0 of this file is the `content_id` the login packets carry.** It used to
be hardcoded as `22` at five call sites; since 2026-08-12 (#300) all five read it
from here through `DivisionInfo` (`client/src/plugins/config/division.rs`), which
`main()` and the headless net-check each insert as a resource before the app
ticks — the gateway connect fires on the first frame, so the asset server would
deliver this file too late. It is read synchronously via
`Archive::read_file_bytes`, resolving Media.pk2 the same way `SroAssetPlugin`
does (`SRO_PK2_PATH` → `SRO_PATH` → `<cwd>/assets`).

Parse shape: `u8 content_id | u8 division_count | { u32 len, name, 0x00,
u8 gateway_count, { u32 len, host, 0x00 } }` — each length excludes its `0x00`
terminator. A missing or malformed file falls back to content id 22 with a
warning, so a tree without PK2s keeps working (notably the net-check against a
local stub, which never builds an asset server).

`config.yaml`'s `network_settings.gateway_address` is now an optional
**override**: omit it and the address becomes `divisions[0].gateways[0]` joined
with `gateport.txt`, resolved via DNS at connect.

**Still UNKNOWN:** whether this `ContentID` byte is the same enum as the login
packets' `content_id: u8`. Name, width and value all agree, and 22 is what the
five call sites already sent, so wiring it changes no bytes on the wire for this
build — but confirming the identity needs another locale's PK2.