# Entity spawn / despawn (0x3017 begin / 0x3019 data / 0x3018 end, 0x3015 / 0x3016)

Status: layout decoded from the client parser and its byte-array unit tests
(`client/src/net/entity_spawn.rs`), calibrated against live vSRO 1.188 captures.
The player and NPC records (incl. the interaction-option list) are
capture-verified; the monster rarity tail and the dropped-item drop-source tail
are cross-referenced with skrillax's `silkroad-protocol` and corrected against
`packet_dump/0x3015.log` (2026-07-24); the gate-building record is verified
against 40 identical captures of the Jangan dimensional gate. Fields the parser
skips without interpreting are marked *(unverified)* — their names are inferred,
not observed.

All five opcodes are **S→C** (`0x3xxx`). Direction is confirmed by the macro's
section comment and the range convention in `docs/protocol/opcodes.md`.

The framing structs live in `packets/src/agent/ingame.rs`
(`GroupEntitySpawnBegin`/`Data`/`End`, `SingleEntitySpawn`,
`SingleEntityDespawn`); the shared wire blocks (`SpawnPosition`,
`EntityMovement`, `EntityState`, `ActiveBuff`) live in
`packets/src/agent/character_data.rs`. The record payload is carried **unparsed**
at dispatch and decoded in `client/src/net/entity_spawn.rs`
(`parse_group_spawn`) with the bounds-checked reader in
`client/src/net/reader.rs`, because a record's shape depends on the client's
itemdata/characterdata tables (the same reason `CharacterDataBody` (0x3013) is a
raw passthrough — see [`net-character-data-0x3013.md`](net-character-data-0x3013.md)).
The batch consumer is `on_group_spawn` / `on_single_spawn` in
`client/src/scenes/game_scene.rs`.

## Sources

- **Repo code (ground truth):** `client/src/net/entity_spawn.rs` (the parser +
  its inline `#[cfg(test)]` byte fixtures), `client/src/net/reader.rs` (the
  position / movement / character-state block readers),
  `packets/src/agent/ingame.rs` (framing structs and their round-trip tests),
  `packets/src/agent/character_data.rs` (`SpawnPosition`, `EntityMovement`,
  `EntityState`, `ActiveBuff`), `client/src/plugins/net/entities.rs`
  (`MonsterRarity` class map).
- **Public cross-references**: skrillax
  `silkroad-protocol` (kumpelblase2) — `GroupSpawn`, `ItemSpawnData`,
  `EntityRarity` — and the SilkroadDoc wiki *Agent packets* page. These target
  vSRO 1.188, our version. Non-repo layouts are reference only; the byte offsets
  below are authoritative for the openroad parser.

All integers are little-endian. Strings are `u16` length + bytes.

## Batch framing (0x3017 / 0x3019 / 0x3018) and single spawn/despawn (0x3015 / 0x3016)

After the self-spawn stream, the agent server streams every *other* nearby
entity (remote players, NPCs, monsters, item drops) as a batch:

```text
0x3017  begin   kind:u8, count:u16          spawn vs despawn + record count
0x3019  data    <count records>             carried unparsed; decoded client-side
0x3018  end     (empty)                     the batch is complete
```

> **0x3018 is the empty End marker and 0x3019 carries the data** — the opposite
> of what some server docs list, confirmed against a live capture (see the macro
> comment in `packets/src/lib.rs`).

| Opcode | Name | Body | Notes |
|---|---|---|---|
| `0x3017` | GroupEntitySpawnBegin | `kind:u8, count:u16` | `kind` = `GROUP_SPAWN` (1) or `GROUP_DESPAWN` (2). Trailing bytes after `count` are ignored on decode (some servers append unknown fields). |
| `0x3019` | GroupEntitySpawnData | `count` × record | Spawn → one full entity record per entity (layout below). Despawn → `count` × bare `u32` unique id. |
| `0x3018` | GroupEntitySpawnEnd | *(empty)* | Resets the client's pending-batch state. |
| `0x3015` | SingleEntitySpawn | exactly one 0x3019 **spawn** record | A single entity entering view outside a batch (monster respawn, a player walking into range). Decoded with `parse_group_spawn(raw, spawning=true, count=1)`. |
| `0x3016` | SingleEntityDespawn | `unique_id:u32` | A single entity leaving view. |

The client resolves `count`/`kind` from the preceding `GroupEntitySpawnBegin`
(stored in a `batch` local); a `GroupEntitySpawnData` with no preceding begin is
logged and dropped.

## Record dispatch — the leading `ref_id`

Every spawn record begins with a `ref_id:u32`. Its type is **not** on the wire —
it is resolved from the id against the loaded tables (`RefResolver::resolve`),
and the resolved type selects the per-record parser:

| Resolves via | Type | Record parser |
|---|---|---|
| itemdata, `is_equipment()` | Item (equipment) | `parse_item` (equipment arm) |
| itemdata, `is_gold()` | Item (gold) | `parse_item` (gold arm) |
| itemdata, other | Item (other drop) | `parse_item` (plain arm) |
| characterdata, `is_player()` | Player | `parse_player` |
| characterdata, `is_monster()` | Monster | `parse_character(is_monster=true)` |
| characterdata, other | NPC | `parse_character(is_monster=false)` |
| teleportbuilding.txt gate ref | Structure | `parse_structure` |
| not found anywhere | Unknown | skip that record if its width is derivable, else **stop the batch** |

Because records are variable-length and back-to-back, a mis-parse cannot recover
the next record boundary. On an unknown ref id or a short read the parser stops
the batch and returns the records decoded so far (fail-safe, mirroring
`character_data.rs`), logging the offending ref id, its resolved type, the record
start/stop offsets, and a hexdump of the remaining bytes so a live capture pins
the divergence.

**#426 — an unclassifiable ref no longer costs the whole batch.** A live server
sends refs this client's tables do not have (9251, 9252, 36030 appear in
`npcpos.txt` but in no `characterdata_*.txt` of either of the user's Media.pk2 —
the ids jump 8984 -> 9264 and 34067 -> 36031), and every entity behind such a
record was lost with it. The record's *layout* is still unknown, but its *width*
is decidable from the payload: assume an `n`-byte body, parse the records that
must follow, and keep `n` only if they all parse and land exactly on the payload's
last byte — a group-spawn body holds whole records and nothing else (verified over
all 265 payloads in `packet_dump/0x3019.log`: each is consumed to its last byte by
its begin count). Accepted only when exactly one `n` does; otherwise the batch
aborts as before. The skipped ref is reported in `GroupSpawnParse::unresolved`
and warned about — it has no model in this client either way, so it is skipped,
never guessed into an entity. Fixture: the 98-byte 2026-08-11 09:43:45.339Z
payload (`01 02 00` begin), records `9252` (unknown, 49 B) + `3861`
NPC_CH_EVENT_KISAENG1 (49 B).

## Shared wire blocks

These three blocks recur inside the character-bearing records and are read by
`Reader` in `client/src/net/reader.rs`.

**Position** (16 bytes, `SpawnPosition`):

| Offset | Field | Type |
|---|---|---|
| +0x00 | region | u16 |
| +0x02 | x | f32 |
| +0x06 | y | f32 |
| +0x0A | z | f32 |
| +0x0E | heading | u16 |

**Movement** (variable, `EntityMovement`; the client skips it — it does not drive
remote motion yet):

```text
has_dest:u8, move_type:u8
  has_dest != 0 : dest_region:u16, [dest_region > 0: dest_x:u16, dest_y:u16, dest_z:u16]
  has_dest == 0 : source:u8, angle:u16          (source 0 = spinning, 1 = sky/key-walk)
```

**Character-state** (variable, `EntityState`): life/motion/body states, the three
movement speeds, and a buff list.

```text
life_state:u8, unk:u8, motion_state:u8, body_state:u8,
walk_speed:f32, run_speed:f32, hwan_speed:f32,
buff_count:u8, buff_count × { ref_skill_id:u32, duration:u32 }
```

## Spawn records by kind

Offsets are from the record's own start. A record is fixed-width only up to its
first variable block (marked ► below); everything after runs sequentially.

### Player (`parse_player`)

```text
ref_id:u32
scale:u8, hwan_level:u8, pvp_cape:u8, auto_invest_xp:u8, base_inventory_size:u8   (unverified names)
► inventory (equipped gear):  count:u8, count × { ref_id:u32, [item is equipment: opt_level:u8] }
avatar_slots:u8
► avatar items:               count:u8, count × { ref_id:u32, [item is equipment: opt_level:u8] }
has_mask:u8
unique_id:u32
position (16 B)
movement (variable)
character-state (variable)
name:string
job_type:u8, job_level:u8, pk_state:u8, riding:u8, in_combat:u8, [riding: riding_uid:u32], scroll:u8, interact:u8, unk:u8   (names unverified; the conditional riding_uid is [S] from xBot PacketParser.cs:744-747, promoted by F7)
guild: name:string, guild_id:u32, member_nick:string, 14 B (crest_rev:u32, union_id:u32, union_crest_rev:u32, is_friendly:u8, siege_authority:u8)   (UNVERIFIED — see below)
equipment_cooldown:u8, pk_flag:u8 (0xFF)
```

**The guild block has never been observed.** `packet_dump/0x3019.log` contains
no player spawn record at all — no name string and no player ref id appears in
any of its 182 payloads (nor in `0x3015.log`), so the guild read has never run
against real bytes. Its layout comes from go-sro's `WriteGuild` — the server
these dumps were captured against — corroborated field-for-field by the vSRO
client-side parser, which also names the 14-byte tail (the earlier "3×u32
crest/union revs" label was wrong: it is crest-rev, union-id, union-crest-rev).
Tagged `[S]`, not `[V]`.

The block is **not** conditional on guild membership: a guildless player sends a
zero-length name plus the same zeroed tail. There is a conditional in this region,
but it is **job mode** — a player wearing job equipment omits everything after the
guild name (vSRO `hasJobMode()`, go-sro's spec `if(Inventory.ContainsJobEquipment == false)`).
We do not implement that branch: go-sro never emits it, so it cannot be tested
locally.

Fixed head:

| Offset | Field | Type | Notes |
|---|---|---|---|
| +0x00 | ref_id | u32 | characterdata player ref |
| +0x04 | scale | u8 | *(unverified)* |
| +0x05 | hwan_level | u8 | *(unverified)* |
| +0x06 | pvp_cape | u8 | *(unverified)* |
| +0x07 | auto_invest_xp | u8 | *(unverified)* |
| +0x08 | base_inventory_size | u8 | *(unverified)* |
| +0x09 | inventory count | u8 | ► variable from here |

The two item lists are **equipment ref ids only** — the client keeps them (as
`SpawnKind::Player { equipment }`) to attach the avatar's visible gear. An item's
per-entry width depends on itemdata: an *equipment* item carries a trailing
`opt_level:u8`, a non-equipment item does not. This itemdata dependency is why
the whole payload is decoded client-side rather than in the `packets` crate.

### NPC (`parse_character`, `is_monster = false`)

| Offset | Field | Type | Notes |
|---|---|---|---|
| +0x00 | ref_id | u32 | characterdata NPC ref |
| +0x04 | unique_id | u32 | server entity id |
| +0x08 | position | 16 B | region +0x08 … heading +0x16 |
| +0x18 | movement | variable | ► |
| — | character-state | variable | |
| — | interact_tag | u8 | 0 = no interaction |
| — | [tag ≠ 0] option_count | u8 | number of option ids |
| — | [tag ≠ 0] option_count × option_id | u8 each | talk/store/storage/teleport dialog entries |

The interaction list (`tag byte, then u8 count + that many 1-byte option ids`) is
capture-verified (tag = 2, count = 4, then 4 option bytes). Its length **must** be
consumed exactly or the following record desyncs. The client stores the ids on
`NpcTalkOptions` but treats them as advisory only — playtests showed they are not
a reliable dialog-option list (city guards advertise trade-ish bits), so the
dialog derives its real options from the shop/teleport/speech tables.

### Monster (`parse_character`, `is_monster = true`)

Identical to the NPC record, plus a trailing per-instance rarity byte:

| Offset | Field | Type | Notes |
|---|---|---|---|
| … | *(NPC record layout above)* | | |
| — | spawn_rarity | u8 | per-instance rarity class (see below) |

### Item drop (`parse_item`)

```text
ref_id:u32
[equipment item: opt_level:u8]  |  [gold item: amount:u32]  |  [other: nothing]
unique_id:u32
position (16 B)
owner_flag:u8, [owner_flag != 0: owner_jid:u32]
rarity:u8
drop_source:u8, dropper_uid:u32
```

| Offset | Field | Type | Notes |
|---|---|---|---|
| +0x00 | ref_id | u32 | itemdata ref |
| +0x04 | opt_level **or** amount | u8 / u32 | present only for equipment (`u8`) or gold (`u32`); absent otherwise. ► |
| — | unique_id | u32 | |
| — | position | 16 B | no movement/state block for a drop |
| — | owner_flag | u8 | 0 = free-for-all pickup |
| — | [owner_flag ≠ 0] owner_jid | u32 | reserving player's job/char id |
| — | rarity | u8 | item rarity |
| — | drop_source | u8 | how it dropped |
| — | dropper_uid | u32 | uid of the entity that dropped it |

Only gold's `amount` is surfaced (`SpawnKind::Item { amount }`); the drop-source
tail (`rarity, drop_source, dropper_uid`) is present on **every** dropped item,
gold included — confirmed against `packet_dump/0x3015.log` (2026-07-24). The
owner block is skrillax's `ItemSpawnData` shape (owner as a flagged optional).

### Structure — teleport gate building (`parse_structure`)

A fixed 36-byte record for a gate building (teleportbuilding.txt ref, e.g. the
Jangan dimensional gate ref 2094), which has no characterdata row and would
otherwise abort the batch as `Unknown`.

| Offset | Field | Type | Notes |
|---|---|---|---|
| +0x00 | ref_id | u32 | teleportbuilding.txt gate ref |
| +0x04 | unique_id | u32 | |
| +0x08 | position | 16 B | matches the teleportbuilding.txt row exactly |
| +0x18 | tail | 12 B | **layout unknown**, skipped whole (see open questions) |

## Despawn payload

For a `GROUP_DESPAWN` batch the 0x3019 body is simply `count` × `u32` unique id;
the client despawns each (never the local player). A `SingleEntityDespawn`
(0x3016) is the single-record form: one `unique_id:u32`.

## Rarity / champion classes

The monster `spawn_rarity` byte (and characterdata's rarity column as fallback)
maps to `MonsterRarity` in `client/src/plugins/net/entities.rs`, per skrillax's
`EntityRarity`:

| Value | Class | Client effect |
|---|---|---|
| 0 | normal | — |
| 1 | champion | ×1.5 scale, swaps to `_champ.bmt` recolor |
| 3 | unique | world-boss marker (`UniqueMonster`), dedicated characterdata row |
| 4 | giant | ×5.0 scale |
| 5 | titan | ×1.6 scale |
| 6 | elite | ×1.4 scale |
| 7 | strong | ×1.25 scale |
| 8 | unique2 | — |

Bit `0x10` (masked off before the class lookup) marks a **party mob**. The spawn
system logs a mismatch when the packet byte and the characterdata column disagree
but keys the badge off characterdata.

## PvP / state flags

- **Player `pk_flag`** — the trailing `0xFF` byte closing the player record;
  observed constant.
- **Player `pk_state` / `pvp_cape`** — inside the skipped blocks; names inferred,
  not decoded (see open questions).
- **Item `owner_flag` / `owner_jid`** — pickup reservation (0 = free-for-all).

## Open questions

- **Player skipped fields.** The 5-byte head (`scale, hwan_level, pvp_cape,
  auto_invest_xp, base_inventory_size`) and the 8-byte block after the name
  (`job_type, job_level, pk_state, transport, in_combat, scroll, interact, unk`)
  are consumed by width only; the field names are inferred and unverified.
- **Player mount/transport.** `parse_player` now models the conditional: a set
  riding flag inserts `riding_uid:u32` before the scroll byte, mirroring
  CHARACTER_DATA's (0x3013) `transport_flag`/`transport_id` pair. [S] from
  xBot (`PacketParser.cs:744-747`, corroborating anapse over skrillax's plain
  bool) — no mounted-remote-player capture yet; F7 promotes it to [V]. The old
  flat 8-byte skip desynced the batch by 4 bytes for any mounted player.
- **COS records.** COS refs (characterdata `1/2/3/tid4`) now parse with their
  own record: the shared NPC head plus a tid4-selected owner tail (horses none;
  pets `Name`; all non-horse `OwnerName, job, [pvp unless pick pet],
  [owner_obj_id for guild guards], owner_uid`). [S] from xBot
  (`PacketParser.cs:791-807`); promoted by a COS-spawn capture (F2/F8).
  Previously these fell through to the plain NPC parse, leaving the tail
  unconsumed and desyncing every later record in the batch.
- **Structure tail.** The gate record's 12-byte tail
  (`01 00 00 01 00 00 00 00 00 00 00 00` in every capture) has no known layout —
  no movement/state block fits in 12 bytes — and is skipped whole. Non-Jangan
  gates would surface any variant via the batch-abort diagnostics.
- **Interaction tag values.** Only `tag = 0` (none) and `tag = 2` (talk) are
  observed; other tag values, and the meaning of the individual option ids, are
  unverified. The client already treats the option ids as advisory.
- **Monster rarity tail vs status note.** `parse_item`'s own comment records a
  2026-07-24 capture verification of the drop tail, while the module-header
  comment still lists the monster rarity tail and dropped-item branch as
  "pending a capture" — the header note is likely stale relative to the function.
  Worth reconciling on the next capture pass.

## Sample hex (synthetic / redacted)

Drawn from the in-repo unit-test fixtures and world constants only — no account
or session data.

```text
0x3017  01 03 00                              begin: kind=SPAWN(1), count=3
0x3018  (empty)                               end marker
0x3016  28 62 05 00                           single despawn: unique_id 352808

0x3019  (one Structure record — Jangan dimensional gate, ref 2094, captured
         40× as its own count-1 batch; a fixed world object, safe to quote)
        2e 08 00 00                           ref_id 2094
        0c 00 00 00                           unique_id 12
        a8 61                                 region 25000
        00 c0 9c 44                           x 1254.0
        00 00 c0 c0                           y -6.0
        00 c0 ab 44                           z 1374.0
        00 00                                 heading 0
        01 00 00 01 00 00 00 00 00 00 00 00   12-byte tail (layout unknown)
```

Player, NPC, monster and item-drop record bytes are exercised in
`client/src/net/entity_spawn.rs`'s `#[cfg(test)]` module via a little-endian
`Body` builder (`parses_player_with_mixed_equipment`, `parses_npc_and_monster`,
`parses_talkable_npc_with_options`, `parses_gold_drop`, `parses_despawn_list`);
see that module for full synthetic record bytes.
